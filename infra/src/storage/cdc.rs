//! Content-defined chunk boundaries for file blocks.
//!
//! A rolling fingerprint over a 48-byte window decides where a chunk ends. The
//! scan starts at the minimum chunk size and stops at the first position whose
//! low bits match the target, or at the maximum size. The final chunk is whatever
//! remains at end of input, which is also how short files stay in one piece.
//!
//! [`Chunker`] performs the same scan incrementally. It retains only the current
//! candidate chunk, never the whole stream, so its buffer is bounded by the
//! configured chunk size. Complete chunks are returned by [`Chunker::feed`] and
//! the last chunk is returned by [`Chunker::finish`].

/// Average block size used by the production file format.
pub const SEAFILE_AVG_BLOCK: usize = 8 * 1024 * 1024;
/// Minimum block size used by the production file format.
pub const SEAFILE_MIN_BLOCK: usize = 6 * 1024 * 1024;
/// Maximum block size used by the production file format.
pub const SEAFILE_MAX_BLOCK: usize = 10 * 1024 * 1024;

/// Chunk-size parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkSizes {
    pub avg: usize,
    pub min: usize,
    pub max: usize,
}

impl ChunkSizes {
    /// The production parameters for file blocks.
    pub const SEAFILE: ChunkSizes = ChunkSizes {
        avg: SEAFILE_AVG_BLOCK,
        min: SEAFILE_MIN_BLOCK,
        max: SEAFILE_MAX_BLOCK,
    };
}

/// Polynomial over GF(2) used by the rolling fingerprint.
const POLYNOMIAL: u64 = 0xbfe6_b8a5_bf37_8d83;
/// Number of bytes covered by one fingerprint value.
const WINDOW_BYTES: usize = 48;
/// Fingerprint bits that select a boundary.
const BREAK_PATTERN: u32 = 0x0013;
/// Number of fingerprint bits discarded when indexing the byte table.
const TABLE_INDEX_SHIFT: u32 = 55;
/// Fingerprint values are kept in this many low bits while scanning.
const FINGERPRINT_MASK: u64 = 0xffff_ffff;

/// Lookup tables for advancing and retiring one byte of the rolling window.
struct FingerprintTables {
    /// Contribution of an incoming byte, indexed by the high fingerprint bits.
    incoming: [u64; 256],
    /// Contribution of an outgoing byte, indexed by its value.
    outgoing: [u64; 256],
}

/// 1-based position of the highest set bit, or zero for zero.
const fn msb_position(value: u64) -> u32 {
    64 - value.leading_zeros()
}

/// Carry-less multiplication of two 64-bit words into a 128-bit product.
const fn multiply_wide(x: u64, y: u64) -> (u64, u64) {
    let mut high = 0u64;
    let mut low = if x & 1 == 0 { 0 } else { y };
    let mut bit = 1u32;
    while bit < 64 {
        if x & (1u64 << bit) != 0 {
            high ^= y >> (64 - bit);
            low ^= y << bit;
        }
        bit += 1;
    }
    (high, low)
}

/// Reduce a 128-bit carry-less product modulo `divisor`.
const fn reduce_product(mut high: u64, mut low: u64, divisor: u64) -> u64 {
    let degree = msb_position(divisor) - 1;
    let scaled = divisor << (63 - degree);

    if high != 0 {
        if high & (1u64 << 63) != 0 {
            high ^= scaled;
        }
        let mut bit = 63u32;
        while bit > 0 {
            bit -= 1;
            if high & (1u64 << bit) != 0 {
                high ^= scaled >> (63 - bit);
                low ^= scaled << (bit + 1);
            }
        }
    }

    let mut bit = 64u32;
    while bit > degree {
        bit -= 1;
        if low & (1u64 << bit) != 0 {
            low ^= scaled >> (63 - bit);
        }
    }
    low
}

/// Multiply two field elements modulo `divisor`.
const fn multiply_mod(x: u64, y: u64, divisor: u64) -> u64 {
    let (high, low) = multiply_wide(x, y);
    reduce_product(high, low, divisor)
}

/// Build the incoming- and outgoing-byte tables.
const fn build_tables() -> FingerprintTables {
    let unit = reduce_product(0, 1u64 << 63, POLYNOMIAL);

    let mut incoming = [0u64; 256];
    let mut byte = 0usize;
    while byte < 256 {
        incoming[byte] = multiply_mod(byte as u64, unit, POLYNOMIAL) | ((byte as u64) << 63);
        byte += 1;
    }

    let mut window_contribution = 1u64;
    let mut step = 1usize;
    while step < WINDOW_BYTES {
        window_contribution = (window_contribution << 8)
            ^ incoming[(window_contribution >> TABLE_INDEX_SHIFT) as usize];
        step += 1;
    }

    let mut outgoing = [0u64; 256];
    byte = 0;
    while byte < 256 {
        outgoing[byte] = multiply_mod(byte as u64, window_contribution, POLYNOMIAL);
        byte += 1;
    }

    FingerprintTables { incoming, outgoing }
}

/// Tables are fixed by [`POLYNOMIAL`], so they are evaluated at compile time.
static TABLES: FingerprintTables = build_tables();

/// Advance the fingerprint with one incoming byte.
#[inline]
fn advance(fingerprint: u64, byte: u8) -> u64 {
    let shifted = (fingerprint << 8) | u64::from(byte);
    let index = (fingerprint >> TABLE_INDEX_SHIFT) as usize;
    shifted ^ TABLES.incoming[index]
}

/// Fingerprint of one complete 48-byte window.
fn window_fingerprint(window: &[u8]) -> u64 {
    debug_assert_eq!(window.len(), WINDOW_BYTES);
    let mut fingerprint = 0u64;
    for &byte in window {
        fingerprint = advance(fingerprint, byte) & FINGERPRINT_MASK;
    }
    fingerprint
}

/// Advance the fingerprint by one byte on each side of the window.
#[inline]
fn roll(fingerprint: u64, outgoing: u8, incoming: u8) -> u64 {
    advance(fingerprint ^ TABLES.outgoing[outgoing as usize], incoming) & FINGERPRINT_MASK
}

/// Compute the chunk offsets of `data` with the production sizes.
pub fn file_chunk_cdc(data: &[u8]) -> Vec<(usize, usize)> {
    file_chunk_cdc_with(data, ChunkSizes::SEAFILE)
}

/// Compute contiguous, non-empty `(offset, length)` chunks covering `data`.
pub fn file_chunk_cdc_with(data: &[u8], sizes: ChunkSizes) -> Vec<(usize, usize)> {
    debug_assert!(
        sizes.min >= WINDOW_BYTES,
        "minimum chunk size must hold a full rolling window"
    );

    let mask = (sizes.avg as u32).wrapping_sub(1);
    let target = BREAK_PATTERN & mask;
    let total = data.len();
    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < total {
        if total - start <= sizes.min {
            chunks.push((start, total - start));
            break;
        }

        let mut position = start + sizes.min - 1;
        let mut fingerprint = window_fingerprint(&data[position + 1 - WINDOW_BYTES..=position]);

        loop {
            if (fingerprint as u32) & mask == target {
                chunks.push((start, position - start + 1));
                start = position + 1;
                break;
            }

            if position + 1 >= start + sizes.max || position + 1 >= total {
                chunks.push((start, sizes.max.min(total - start)));
                start += sizes.max;
                break;
            }

            position += 1;
            fingerprint = roll(fingerprint, data[position - WINDOW_BYTES], data[position]);
        }
    }

    chunks
}

/// Incremental chunker with the same boundaries as [`file_chunk_cdc_with`].
///
/// The chunker buffers the current candidate chunk only. `file_size` is accepted
/// for API symmetry with whole-file callers; the boundary scan itself never needs
/// the total length, and the trailing chunk is always returned by [`Self::finish`].
pub struct Chunker {
    sizes: ChunkSizes,
    pending: Vec<u8>,
    fingerprint: u64,
    next_probe: usize,
    probing: bool,
}

impl Chunker {
    /// Create a chunker with the production sizes.
    pub fn new(file_size: usize) -> Self {
        Self::with_sizes(file_size, ChunkSizes::SEAFILE)
    }

    /// Create a chunker with explicit sizes.
    pub fn with_sizes(file_size: usize, sizes: ChunkSizes) -> Self {
        debug_assert!(
            sizes.min >= WINDOW_BYTES,
            "minimum chunk size must hold a full rolling window"
        );
        let _ = file_size;
        Self {
            sizes,
            pending: Vec::new(),
            fingerprint: 0,
            next_probe: 0,
            probing: false,
        }
    }

    /// Consume bytes and return every chunk that is now complete.
    pub fn feed(&mut self, mut bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut completed = Vec::new();
        // The scan starts only once there is at least one byte beyond the
        // minimum, matching the whole-input rule that a remainder of exactly
        // `min` is a tail rather than a scan. The extra byte also lets a
        // maximum below the minimum make progress.
        let buffer_limit = self.sizes.max.max(self.sizes.min.saturating_add(1));

        while !bytes.is_empty() {
            let room = buffer_limit.saturating_sub(self.pending.len());
            if room == 0 {
                self.scan(&mut completed);
                debug_assert!(
                    self.pending.len() < buffer_limit,
                    "a full pending chunk must yield a boundary"
                );
                if self.pending.len() >= buffer_limit {
                    // Degenerate sizes (for example max == 0); avoid an endless loop.
                    return completed;
                }
                continue;
            }

            let take = room.min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            self.scan(&mut completed);
        }

        completed
    }

    /// Return the final, possibly empty, chunk.
    pub fn finish(self) -> Vec<u8> {
        self.pending
    }

    /// Find boundaries in the buffered data and retire the completed chunks.
    fn scan(&mut self, completed: &mut Vec<Vec<u8>>) {
        let mask = (self.sizes.avg as u32).wrapping_sub(1);
        let target = BREAK_PATTERN & mask;

        loop {
            if !self.probing {
                if self.pending.len() <= self.sizes.min {
                    return;
                }
                let window_start = self.sizes.min - WINDOW_BYTES;
                self.fingerprint = window_fingerprint(&self.pending[window_start..self.sizes.min]);
                self.next_probe = self.sizes.min - 1;
                self.probing = true;
            }

            if self.next_probe >= self.pending.len() {
                return;
            }

            let position = self.next_probe;
            if (self.fingerprint as u32) & mask == target {
                self.emit(position + 1, completed);
                continue;
            }

            if position + 1 >= self.sizes.max {
                self.emit(self.sizes.max, completed);
                continue;
            }

            if position + 1 >= self.pending.len() {
                return;
            }

            self.fingerprint = roll(
                self.fingerprint,
                self.pending[position + 1 - WINDOW_BYTES],
                self.pending[position + 1],
            );
            self.next_probe = position + 1;
        }
    }

    /// Remove the first `length` bytes as one completed chunk and reset the scan.
    fn emit(&mut self, length: usize, completed: &mut Vec<Vec<u8>>) {
        let chunk: Vec<u8> = self.pending.drain(..length).collect();
        completed.push(chunk);
        self.fingerprint = 0;
        self.next_probe = 0;
        self.probing = false;
    }
}
