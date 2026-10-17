use std::sync::LazyLock;

const POLY: u64 = 0xbfe6b8a5bf378d83;
const WINDOW_SIZE: usize = 48;
const BREAK_VALUE: u32 = 0x0013;

/// seafile's official CDC block sizing.
///
/// `common/fs-mgr.h` defines `CDC_AVERAGE_BLOCK_SIZE` = 8 MiB, `CDC_MIN_BLOCK_SIZE`
/// = 6 MiB and `CDC_MAX_BLOCK_SIZE` = 10 MiB, and `seaf_fs_manager_index_blocks()`
/// always chunks with them (seafile commit `31dcd928`, 2017).
///
/// The sizes are **fixed and independent of file size**: the legacy size-dependent
/// `calculate_chunk_size()` is only the pre-6.1 fallback, never the indexing path.
/// Using it here would change every block boundary and `seafile` object id.
pub const SEAFILE_AVG_BLOCK: usize = 8 * 1024 * 1024;
pub const SEAFILE_MIN_BLOCK: usize = 6 * 1024 * 1024;
pub const SEAFILE_MAX_BLOCK: usize = 10 * 1024 * 1024;

/// Block-size triple for [`file_chunk_cdc_with`] / [`Chunker::with_sizes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkSizes {
    pub avg: usize,
    pub min: usize,
    pub max: usize,
}

impl ChunkSizes {
    /// The triple every seafile client and server has used since 2017.
    pub const SEAFILE: ChunkSizes = ChunkSizes {
        avg: SEAFILE_AVG_BLOCK,
        min: SEAFILE_MIN_BLOCK,
        max: SEAFILE_MAX_BLOCK,
    };
}

/// seafile-compatible Rabin fingerprint.
///
/// Matches `common/cdc/rabin-checksum.c`: polynomial `0xbfe6b8a5bf378d83`,
/// 48-byte window, GF(2) arithmetic with precomputed T/U tables.
struct RabinState {
    window: [u8; WINDOW_SIZE],
    pos: usize,
    count: usize,
    fingerprint: u64,
    t: [u64; 256],
    u: [u64; 256],
    shift: usize,
}

impl RabinState {
    /// Find last set bit (MSB index, 1-based) — matches seafile's fls64.
    fn fls64(v: u64) -> usize {
        if v == 0 {
            return 0;
        }
        64 - v.leading_zeros() as usize
    }

    /// GF(2) polynomial multiplication: (x * y) → (high, low).
    fn polymult(x: u64, y: u64) -> (u64, u64) {
        let mut ph: u64 = 0;
        let mut pl: u64 = 0;
        if x & 1 != 0 {
            pl = y;
        }
        for i in 1..64 {
            if x & (1u64 << i) != 0 {
                ph ^= y >> (64 - i);
                pl ^= y << i;
            }
        }
        (ph, pl)
    }

    /// GF(2) polynomial modulo: (nh, nl) mod d.
    fn polymod(nh: u64, nl: u64, d: u64) -> u64 {
        let k = Self::fls64(d) - 1;
        let d_shifted = d << (63 - k);
        let mut nh = nh;
        let mut nl = nl;
        if nh != 0 {
            if nh & (1u64 << 63) != 0 {
                nh ^= d_shifted;
            }
            for i in (0..=62).rev() {
                if nh & (1u64 << i) != 0 {
                    nh ^= d_shifted >> (63 - i);
                    nl ^= d_shifted << (i + 1);
                }
            }
        }
        for i in (k..=63).rev() {
            if nl & (1u64 << i) != 0 {
                nl ^= d_shifted >> (63 - i);
            }
        }
        nl
    }

    /// GF(2) multiply-then-mod: (x * y) mod d.
    fn polymmult(x: u64, y: u64, d: u64) -> u64 {
        let (h, l) = Self::polymult(x, y);
        Self::polymod(h, l, d)
    }

    fn new() -> Self {
        // Copy the 4 KiB precomputed tables instead of recomputing them per chunk.
        let (shift, t, u) = *RABIN_TABLES;
        Self {
            window: [0; WINDOW_SIZE],
            pos: 0,
            count: 0,
            fingerprint: 0,
            t,
            u,
            shift,
        }
    }

    /// Core update: `(p * x^8 + m) mod poly`, using the T table.
    fn append8(&self, p: u64, m: u8) -> u64 {
        let idx = (p >> self.shift) as usize;
        ((p << 8) | m as u64) ^ self.t[idx]
    }

    /// Seed from the first WINDOW_SIZE bytes (seafile's `rabin_checksum`).
    fn init(&mut self, data: &[u8]) {
        self.fingerprint = 0;
        self.count = 0;
        self.pos = 0;

        if data.len() >= WINDOW_SIZE {
            for (i, &byte) in data.iter().enumerate().take(WINDOW_SIZE) {
                self.fingerprint = self.append8(self.fingerprint, byte) & 0xFFFF_FFFF;
                self.window[i] = byte;
            }
            self.count = WINDOW_SIZE;
            self.pos = WINDOW_SIZE % WINDOW_SIZE;
        }
    }

    /// Slide the window by one byte (seafile's `rabin_rolling_checksum`).
    fn update(&mut self, byte: u8) {
        let out = self.window[self.pos];
        self.fingerprint =
            self.append8(self.fingerprint ^ self.u[out as usize], byte) & 0xFFFF_FFFF;
        self.window[self.pos] = byte;
        self.pos = (self.pos + 1) % WINDOW_SIZE;
        self.count += 1;
    }

    fn get_fingerprint(&self) -> u32 {
        self.fingerprint as u32
    }
}

/// Precomputed Rabin lookup tables for `POLY`/`WINDOW_SIZE`, computed once and
/// shared by every `RabinState::new()`. Recomputing them per chunk cost ~512
/// `polymult`/`polymod` operations per chunk.
static RABIN_TABLES: LazyLock<(usize, [u64; 256], [u64; 256])> = LazyLock::new(|| {
    let poly = POLY;
    let xshift = RabinState::fls64(poly) - 1;
    let shift = xshift - 8;

    // Precompute T[256]: T[j] = (j * x^62 mod poly) | (j << 62)
    let t1 = RabinState::polymod(0, 1u64 << xshift, poly);
    let mut t = [0u64; 256];
    for j in 0..256u64 {
        t[j as usize] = RabinState::polymmult(j, t1, poly) | (j << xshift);
    }

    // Precompute U[256]: U[i] = i * x^(8*WINDOW_SIZE) mod poly
    let mut sizeshift: u64 = 1;
    for _ in 1..WINDOW_SIZE {
        let idx = (sizeshift >> shift) as usize;
        sizeshift = (sizeshift << 8) ^ t[idx];
    }
    let mut u = [0u64; 256];
    for i in 0..256u64 {
        u[i as usize] = RabinState::polymmult(i, sizeshift, poly);
    }

    (shift, t, u)
});

/// Content-defined chunking matching seafile's `file_chunk_cdc` exactly, using its
/// official fixed block sizes ([`ChunkSizes::SEAFILE`]).
///
/// Same buffer management as the C code: skip `min - WINDOW_SIZE` bytes, seed the
/// fingerprint at `min - 1`, then roll until a break point or max size.
pub fn file_chunk_cdc(data: &[u8]) -> Vec<(usize, usize)> {
    file_chunk_cdc_with(data, ChunkSizes::SEAFILE)
}

/// [`file_chunk_cdc`] with an explicit block-size triple. Production always uses
/// [`ChunkSizes::SEAFILE`]; the parameter lets tests exercise multi-block boundaries
/// on small fixtures.
pub fn file_chunk_cdc_with(data: &[u8], sizes: ChunkSizes) -> Vec<(usize, usize)> {
    let file_size = data.len();
    if file_size == 0 {
        return vec![];
    }
    let ChunkSizes { avg, min, max } = sizes;
    let mask = (avg as u32).wrapping_sub(1);
    let target = BREAK_VALUE & mask;

    let mut chunks = Vec::new();
    let mut chunk_start = 0usize;

    while chunk_start < file_size {
        // If remaining data <= min, emit the rest as final chunk
        if file_size - chunk_start <= min {
            chunks.push((chunk_start, file_size - chunk_start));
            break;
        }

        // Compute initial fingerprint at position `min - 1` from scratch (like C's finger)
        let scan_start = chunk_start + min - 1;
        // `min` far exceeds the Rabin window, so the last window before `scan_start` exists.
        debug_assert!(
            scan_start >= WINDOW_SIZE - 1,
            "scan_start too small for window"
        );
        let mut state = RabinState::new();
        state.init(&data[scan_start - (WINDOW_SIZE - 1)..]);
        let mut fp = state.get_fingerprint();

        // Scan forward looking for break
        let max_pos = chunk_start + max;
        let end = file_size;
        let mut pos = scan_start;

        loop {
            if (fp & mask) == target {
                // Break found — emit chunk including this byte
                chunks.push((chunk_start, pos - chunk_start + 1));
                chunk_start = pos + 1;
                break;
            }

            if pos + 1 >= max_pos || pos + 1 >= end {
                // Max size or end of file — emit max-size chunk
                let chunk_end = chunk_start + max;
                if chunk_end <= file_size {
                    chunks.push((chunk_start, max));
                } else {
                    chunks.push((chunk_start, file_size - chunk_start));
                }
                chunk_start += max;
                break;
            }

            // Move to next byte
            pos += 1;
            state.update(data[pos]);
            fp = state.get_fingerprint();
        }
    }

    chunks
}

/// Content-defined chunker that consumes bytes incrementally and emits chunks, so
/// the caller never buffers the whole file in memory.
///
/// Boundaries are **identical** to [`file_chunk_cdc`]: the same windowing, break and
/// max-size state machine, with each chunk's bytes buffered and the fingerprint
/// rolled forward. Memory is O(max chunk size) regardless of file size.
pub struct Chunker {
    file_size: usize,
    min: usize,
    max: usize,
    mask: u32,
    target: u32,
    /// Bytes accumulated for the current chunk (always `data[chunk_start..=pos]`).
    chunk_buf: Vec<u8>,
    /// Global offset of the current chunk's first byte.
    chunk_start: usize,
    /// Rabin state, meaningful once the chunk has reached `min` bytes.
    state: RabinState,
    /// `false` until the chunk has its first `min` bytes (no break checks), `true` once
    /// the rolling fingerprint advances byte by byte.
    rolling: bool,
}

impl Chunker {
    /// Streaming CDC with seafile's fixed block sizes ([`ChunkSizes::SEAFILE`]).
    /// `file_size` drives only the end-of-file test; the block sizes do **not** depend
    /// on it, so this agrees with [`file_chunk_cdc`] for any size. For an unknown size
    /// pass `0` and let [`Chunker::finish`] produce the trailing chunk.
    pub fn new(file_size: usize) -> Self {
        Self::with_sizes(file_size, ChunkSizes::SEAFILE)
    }

    /// [`Chunker::new`] with an explicit triple (tests; see [`file_chunk_cdc_with`]).
    pub fn with_sizes(file_size: usize, sizes: ChunkSizes) -> Self {
        let ChunkSizes { avg, min, max } = sizes;
        let mask = (avg as u32).wrapping_sub(1);
        Self {
            file_size,
            min,
            max,
            mask,
            target: BREAK_VALUE & mask,
            chunk_buf: Vec::with_capacity(max),
            chunk_start: 0,
            state: RabinState::new(),
            rolling: false,
        }
    }

    /// Feed a slice, returning any chunks that become complete. Chunk boundaries come
    /// from the content, not the feed sizes. The prefix phase is a bulk memcpy; the
    /// rolling phase scans byte by byte.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut i = 0usize;
        // Local mirror of the chunk length, so the loop avoids a capacity check per byte.
        let mut chunk_len = self.chunk_buf.len();

        while i < bytes.len() {
            if !self.rolling {
                // Prefix phase: accumulate up to `min` bytes in one memcpy.
                while !self.rolling && i < bytes.len() {
                    let take = (self.min - chunk_len).min(bytes.len() - i);
                    self.chunk_buf.extend_from_slice(&bytes[i..i + take]);
                    chunk_len += take;
                    i += take;
                    if chunk_len == self.min {
                        self.finish_min_chunk(&mut out);
                        // Emitted (buffer empty) or rolling flipped on (length stays `min`).
                        chunk_len = self.chunk_buf.len();
                    }
                }
            } else {
                // Rolling phase: the fingerprint covers the last byte in `chunk_buf`; `bytes[i]`
                // is the next byte.
                while self.rolling && i < bytes.len() {
                    let b = bytes[i];
                    let pos = self.chunk_start + chunk_len - 1;
                    let fp = self.state.get_fingerprint();
                    let next_pos = pos + 1;

                    // Break point at `pos` — emit this chunk (without `b`).
                    if (fp & self.mask) == self.target {
                        out.push(self.take_chunk());
                        self.rolling = false;
                        self.chunk_start = next_pos;
                        self.chunk_buf.push(b);
                        chunk_len = 1;
                        i += 1;
                        continue;
                    }

                    // Max size or end of file. `file_size == 0` means unknown (the A2 multipart path
                    // has no field size), so the end test is suppressed; `finish()` ends the tail.
                    if next_pos >= self.chunk_start + self.max
                        || (self.file_size > 0 && next_pos >= self.file_size)
                    {
                        out.push(self.take_chunk());
                        self.rolling = false;
                        self.chunk_start = next_pos;
                        self.chunk_buf.push(b);
                        chunk_len = 1;
                        i += 1;
                        continue;
                    }

                    // Roll `b` into the window.
                    self.state.update(b);
                    self.chunk_buf.push(b);
                    chunk_len += 1;
                    i += 1;
                }
            }
        }
        out
    }

    /// Finish the stream and return the final (possibly empty) chunk.
    pub fn finish(self) -> Vec<u8> {
        self.chunk_buf
    }

    /// Emit the chunk, leaving a buffer pre-sized to `max` for the next one.
    fn take_chunk(&mut self) -> Vec<u8> {
        std::mem::replace(&mut self.chunk_buf, Vec::with_capacity(self.max))
    }

    /// Seed the fingerprint once the chunk reaches `min` bytes (matching the whole-
    /// buffer `scan_start`), then decide whether it ends here.
    fn finish_min_chunk(&mut self, out: &mut Vec<Vec<u8>>) {
        debug_assert!(
            self.chunk_buf.len() >= WINDOW_SIZE,
            "chunk must exceed the window before fingerprint init"
        );
        self.state
            .init(&self.chunk_buf[self.chunk_buf.len() - WINDOW_SIZE..]);
        let fp = self.state.get_fingerprint();
        let pos = self.chunk_start + self.chunk_buf.len() - 1;
        let next_pos = pos + 1;

        // Break point at `pos` — chunk is `min` bytes.
        if (fp & self.mask) == self.target {
            out.push(self.take_chunk());
            self.chunk_start = next_pos;
            self.rolling = false;
            return;
        }

        // Max size or end of file. `file_size == 0` (A2 multipart) suppresses the end
        // test; `finish()` ends the tail.
        if next_pos >= self.chunk_start + self.max
            || (self.file_size > 0 && next_pos >= self.file_size)
        {
            out.push(self.take_chunk());
            self.chunk_start = next_pos;
            self.rolling = false;
            return;
        }

        self.rolling = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small triple for boundary/equivalence tests: the production 8/6/10 MiB sizes
    /// need ~30 MiB fixtures. The production triple is locked by
    /// [`test_default_block_sizes_match_seafile`].
    const TEST_SIZES: ChunkSizes = ChunkSizes {
        avg: 1024 * 1024,
        min: 256 * 1024,
        max: 4 * 1024 * 1024,
    };

    /// Deterministic pseudo-random bytes (no rand dependency), matching the generator
    /// of the C reference test.
    fn lcg_bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    /// Feed `data` in `feed_len` slices and collect the chunks.
    fn stream_blocks(data: &[u8], feed_len: usize, sizes: ChunkSizes) -> Vec<Vec<u8>> {
        if data.is_empty() {
            return Vec::new();
        }
        let mut ch = Chunker::with_sizes(data.len(), sizes);
        let mut blocks = Vec::new();
        for chunk in data.chunks(feed_len) {
            blocks.extend(ch.feed(chunk));
        }
        let last = ch.finish();
        if !last.is_empty() {
            blocks.push(last);
        }
        blocks
    }

    /// The streaming chunker must produce boundaries *identical* to the whole-buffer
    /// chunker for the same content, under many feed slice sizes.
    fn assert_chunkers_agree(data: &[u8], sizes: ChunkSizes) {
        let expected: Vec<usize> = file_chunk_cdc_with(data, sizes)
            .iter()
            .map(|(_, s)| *s)
            .collect();
        for feed_len in [1usize, 7, 64, 999, 4096] {
            let blocks = stream_blocks(data, feed_len, sizes);
            let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
            assert_eq!(sizes, expected, "feed_len={feed_len} len={}", data.len());
            assert_eq!(
                blocks.concat(),
                data,
                "feed_len={feed_len} len={}",
                data.len()
            );
        }
    }

    #[test]
    fn test_chunker_matches_file_chunk_cdc() {
        assert_chunkers_agree(&[], TEST_SIZES);
        assert_chunkers_agree(&[0u8; 1], TEST_SIZES);
        assert_chunkers_agree(&[0u8; 1000], TEST_SIZES);
        assert_chunkers_agree(&[0xFFu8; 500], TEST_SIZES);
        assert_chunkers_agree(
            &(0..256 * 1024).map(|i| i as u8).collect::<Vec<_>>(),
            TEST_SIZES,
        );
        assert_chunkers_agree(&[0u8; 256 * 1024 + 100], TEST_SIZES);
        assert_chunkers_agree(
            &(0..300_000)
                .map(|i| if i % 2 == 0 { 0xAA } else { 0x55 })
                .collect::<Vec<_>>(),
            TEST_SIZES,
        );
        assert_chunkers_agree(&[0xFFu8; 1_000_000], TEST_SIZES);
        let data: Vec<u8> = (0..3_000_000)
            .map(|i: usize| (i.wrapping_mul(31) ^ (i >> 2) ^ (i.wrapping_mul(7))) as u8)
            .collect();
        assert_chunkers_agree(&data, TEST_SIZES);
    }

    /// The A2 multipart path uses `Chunker::new(0)` (field size unknown) while the
    /// whole-buffer path uses `file_chunk_cdc`; both must agree on boundaries.
    #[test]
    fn test_chunker_new_zero_matches_file_chunk_cdc() {
        fn feed_zero(data: &[u8], feed_len: usize, sizes: ChunkSizes) -> Vec<Vec<u8>> {
            if data.is_empty() {
                return Vec::new();
            }
            let mut ch = Chunker::with_sizes(0, sizes);
            let mut blocks = Vec::new();
            for chunk in data.chunks(feed_len) {
                blocks.extend(ch.feed(chunk));
            }
            let last = ch.finish();
            if !last.is_empty() {
                blocks.push(last);
            }
            blocks
        }

        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("sub-min", vec![0u8; 1000]),
            ("exact-min", vec![0xABu8; 256 * 1024]),
            ("min-plus-1", vec![0xCDu8; 256 * 1024 + 1]),
            ("few-MB", lcg_bytes(3 * 1024 * 1024 + 123, 99)),
            ("random-800k", lcg_bytes(800_000, 12345)),
        ];

        for (label, data) in &cases {
            let expected: Vec<usize> = file_chunk_cdc_with(data, TEST_SIZES)
                .iter()
                .map(|(_, s)| *s)
                .collect();
            for feed_len in [1usize, 3, 17, 64, 4096] {
                let blocks = feed_zero(data, feed_len, TEST_SIZES);
                let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
                assert_eq!(
                    sizes,
                    expected,
                    "Chunker::new(0) mismatch: case={label} feed_len={feed_len} len={}",
                    data.len()
                );
                assert!(
                    blocks.concat() == data.as_slice(),
                    "Chunker::new(0) concat mismatch: case={label} feed_len={feed_len}"
                );
            }
        }
    }

    /// Feed sizes straddling `min` must not shift boundaries; one large feed must emit
    /// multiple chunks in a single call.
    #[test]
    fn test_chunker_feed_len_across_min_boundary() {
        let data: Vec<u8> = (0..2 * 1024 * 1024)
            .map(|i: usize| (i.wrapping_mul(41) ^ (i >> 3)) as u8)
            .collect();
        let expected: Vec<usize> = file_chunk_cdc_with(&data, TEST_SIZES)
            .iter()
            .map(|(_, s)| *s)
            .collect();
        for feed_len in [256 * 1024 - 1, 256 * 1024, 256 * 1024 + 1, 5 * 1024 * 1024] {
            let blocks = stream_blocks(&data, feed_len, TEST_SIZES);
            let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
            assert_eq!(sizes, expected, "feed_len={feed_len}");
            assert_eq!(blocks.concat(), data, "feed_len={feed_len}");
        }
    }

    /// Known size `max + min`: the first chunk is forced out by max size, the second
    /// by the end-of-file check in `finish_min_chunk`.
    #[test]
    fn test_chunker_max_min_known_size() {
        let data = vec![0u8; 4 * 1024 * 1024 + 256 * 1024];
        let expected: Vec<usize> = file_chunk_cdc_with(&data, TEST_SIZES)
            .iter()
            .map(|(_, s)| *s)
            .collect();
        let blocks = stream_blocks(&data, 1024, TEST_SIZES);
        let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
        assert_eq!(sizes, expected);
        assert_eq!(blocks.concat(), data);
    }

    /// Unknown size (`new(0)`) over break-free zero content forces the max-size branch.
    #[test]
    fn test_chunker_zero_unknown_size_over_max() {
        let data = vec![0u8; 4 * 1024 * 1024 + 512 * 1024];
        let expected: Vec<usize> = file_chunk_cdc_with(&data, TEST_SIZES)
            .iter()
            .map(|(_, s)| *s)
            .collect();
        for feed_len in [64usize, 1024, 4 * 1024 * 1024, 5 * 1024 * 1024] {
            let mut ch = Chunker::with_sizes(0, TEST_SIZES);
            let mut blocks = Vec::new();
            for slice in data.chunks(feed_len) {
                blocks.extend(ch.feed(slice));
            }
            let last = ch.finish();
            if !last.is_empty() {
                blocks.push(last);
            }
            let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
            assert_eq!(sizes, expected, "feed_len={feed_len}");
            assert_eq!(blocks.concat(), data, "feed_len={feed_len}");
        }
    }

    /// The streaming chunker must use seafile's fixed sizes regardless of `file_size`.
    #[test]
    fn test_chunker_size_independent() {
        let a = Chunker::new(0);
        let b = Chunker::new(2 * 1024 * 1024); // 2 MiB
        let c = Chunker::new(3 * 1024 * 1024 * 1024); // 3 GiB
        assert_eq!(a.min, SEAFILE_MIN_BLOCK);
        assert_eq!(a.max, SEAFILE_MAX_BLOCK);
        assert_eq!(a.min, b.min, "min must not depend on file_size");
        assert_eq!(a.max, b.max, "max must not depend on file_size");
        assert_eq!(a.min, c.min, "min must not depend on file_size");
        assert_eq!(a.max, c.max, "max must not depend on file_size");
    }

    /// Must equal seafile's `CDC_*_BLOCK_SIZE` constants: 8 / 6 / 10 MiB.
    #[test]
    fn test_default_block_sizes_match_seafile() {
        assert_eq!(ChunkSizes::SEAFILE.avg, 8 * 1024 * 1024);
        assert_eq!(ChunkSizes::SEAFILE.min, 6 * 1024 * 1024);
        assert_eq!(ChunkSizes::SEAFILE.max, 10 * 1024 * 1024);
    }

    /// Golden test: boundaries for 40 MiB of deterministic `lcg_bytes` (seed 1) must
    /// match byte for byte those of **unmodified upstream C** (`common/cdc/rabin-
    /// checksum.c`) with 8/6/10 MiB. This is the compatibility contract: a client
    /// chunking the same bytes derives the same `seafile` object id.
    #[test]
    fn test_default_boundaries_match_seafile_c_reference() {
        const EXPECTED: &[usize] = &[6_431_339, 10_485_760, 10_485_760, 10_485_760, 4_054_421];
        let data = lcg_bytes(40 * 1024 * 1024, 1);

        let chunks = file_chunk_cdc(&data);
        let sizes: Vec<usize> = chunks.iter().map(|(_, s)| *s).collect();
        assert_eq!(sizes, EXPECTED);

        // Offsets must be the running prefix sums.
        let mut offset = 0usize;
        for (chunk_offset, size) in &chunks {
            assert_eq!(*chunk_offset, offset);
            offset += size;
        }
        assert_eq!(offset, data.len());

        // The streaming chunker (the actual upload path) must agree.
        for feed_len in [4096usize, 65_536, 5 * 1024 * 1024] {
            let blocks = stream_blocks(&data, feed_len, ChunkSizes::SEAFILE);
            let sizes: Vec<usize> = blocks.iter().map(|b| b.len()).collect();
            assert_eq!(sizes, EXPECTED, "feed_len={feed_len}");
            assert_eq!(blocks.concat(), data, "feed_len={feed_len}");
        }
    }

    #[test]
    fn test_cdc_small_file() {
        let data = vec![0u8; 1024 * 100];
        let chunks = file_chunk_cdc(&data);
        assert!(!chunks.is_empty());
        let total: usize = chunks.iter().map(|(_, s)| s).sum();
        assert_eq!(total, data.len());
    }

    /// Fingerprint must match a pure-Rust port of seafile's rabin-checksum.c line for
    /// line, independently of `RabinState`.
    #[test]
    fn test_fingerprint_matches_seafile() {
        // Reference: port of seafile's rabin-checksum.c.
        fn fls64_ref(v: u64) -> usize {
            if v == 0 {
                return 0;
            }
            64 - v.leading_zeros() as usize
        }

        fn polymod_ref(nh: u64, nl: u64, d: u64) -> u64 {
            let k = fls64_ref(d) - 1;
            let d_shifted = d << (63 - k);
            let mut nh = nh;
            let mut nl = nl;
            if nh != 0 {
                if nh & (1u64 << 63) != 0 {
                    nh ^= d_shifted;
                }
                for i in (0..=62).rev() {
                    if nh & (1u64 << i) != 0 {
                        nh ^= d_shifted >> (63 - i);
                        nl ^= d_shifted << (i + 1);
                    }
                }
            }
            for i in (k..=63).rev() {
                if nl & (1u64 << i) != 0 {
                    nl ^= d_shifted >> (63 - i);
                }
            }
            nl
        }

        fn polymmult_ref(x: u64, y: u64, d: u64) -> u64 {
            let (mut ph, mut pl) = (0u64, 0u64);
            if x & 1 != 0 {
                pl = y;
            }
            for i in 1..64 {
                if x & (1u64 << i) != 0 {
                    ph ^= y >> (64 - i);
                    pl ^= y << i;
                }
            }
            polymod_ref(ph, pl, d)
        }

        let poly: u64 = 0xbfe6b8a5bf378d83;
        let xshift = fls64_ref(poly) - 1;
        let shift = xshift - 8;

        let t1 = polymod_ref(0, 1u64 << xshift, poly);
        let mut t_ref = [0u64; 256];
        for j in 0..256u64 {
            t_ref[j as usize] = polymmult_ref(j, t1, poly) | (j << xshift);
        }

        let mut sizeshift: u64 = 1;
        for _ in 1..48 {
            let idx = (sizeshift >> shift) as usize;
            sizeshift = (sizeshift << 8) ^ t_ref[idx];
        }
        let mut u_ref = [0u64; 256];
        for i in 0..256u64 {
            u_ref[i as usize] = polymmult_ref(i, sizeshift, poly);
        }

        let append8_ref = |p: u64, m: u8| -> u64 {
            let idx = (p >> shift) as usize;
            ((p << 8) | m as u64) ^ t_ref[idx]
        };

        let rabin_checksum_ref = |buf: &[u8]| -> u64 {
            let mut fp: u64 = 0;
            for &b in buf.iter().take(48) {
                fp = append8_ref(fp, b) & 0xFFFF_FFFF;
            }
            fp
        };

        let rolling_ref =
            |csum: u64, c1: u8, c2: u8| -> u64 { append8_ref(csum ^ u_ref[c1 as usize], c2) };

        // Test vectors:
        let data_zeros = vec![0u8; 100];
        let expected_init = rabin_checksum_ref(&data_zeros);

        let mut state = RabinState::new();
        state.init(&data_zeros);
        assert_eq!(
            state.fingerprint, expected_init,
            "fingerprint mismatch for zeros init: got {:016x}, expected {:016x}",
            state.fingerprint, expected_init
        );

        let mut expected_fp = expected_init;
        expected_fp = rolling_ref(expected_fp, 0, 0x42);
        state.update(0x42);
        assert_eq!(
            state.fingerprint, expected_fp,
            "fingerprint mismatch after rolling update"
        );

        let data_ff = vec![0xFFu8; 100];
        let expected_ff_init = rabin_checksum_ref(&data_ff);
        let mut state2 = RabinState::new();
        state2.init(&data_ff);
        assert_eq!(
            state2.fingerprint, expected_ff_init,
            "fingerprint mismatch for 0xFF init"
        );

        let data_alt: Vec<u8> = (0..100)
            .map(|i| if i % 2 == 0 { 0xAA } else { 0x55 })
            .collect();
        let expected_alt_init = rabin_checksum_ref(&data_alt);
        let mut state3 = RabinState::new();
        state3.init(&data_alt);
        assert_eq!(
            state3.fingerprint, expected_alt_init,
            "fingerprint mismatch for alternating init"
        );

        let state = RabinState::new();
        for i in 0..256 {
            assert_eq!(state.t[i], t_ref[i], "T[{}] mismatch", i);
            assert_eq!(state.u[i], u_ref[i], "U[{}] mismatch", i);
        }
    }

    /// Verify CDC determinism: same content, multiple runs → same chunks.
    #[test]
    fn test_cdc_determinism() {
        let data: Vec<u8> = (0..(1024 * 1024))
            .map(|i: usize| (i.wrapping_mul(7) ^ (i >> 3)) as u8)
            .collect();
        let chunks1 = file_chunk_cdc(&data);
        let chunks2 = file_chunk_cdc(&data);
        assert_eq!(chunks1, chunks2, "CDC must be deterministic");
    }

    /// Verify all chunk sizes sum to the original file size.
    #[test]
    fn test_cdc_total_size() {
        let data: Vec<u8> = (0..(512 * 1024))
            .map(|i: usize| (i.wrapping_mul(13)) as u8)
            .collect();
        let chunks = file_chunk_cdc(&data);
        let total: usize = chunks.iter().map(|(_, s)| s).sum();
        assert_eq!(total, data.len(), "chunks must cover entire file");
    }

    /// Golden proof that the fingerprint matches the reference at every byte position,
    /// so break-point decisions are identical.
    #[test]
    fn test_fingerprint_stream_matches_reference() {
        // Reference implementation ported from seafile rabin-checksum.c
        fn fls64_ref(v: u64) -> usize {
            if v == 0 {
                0
            } else {
                64 - v.leading_zeros() as usize
            }
        }
        fn polymod_ref(nh: u64, nl: u64, d: u64) -> u64 {
            let k = fls64_ref(d) - 1;
            let ds = d << (63 - k);
            let (mut nh, mut nl) = (nh, nl);
            if nh != 0 {
                if nh & (1u64 << 63) != 0 {
                    nh ^= ds;
                }
                for i in (0..=62).rev() {
                    if nh & (1u64 << i) != 0 {
                        nh ^= ds >> (63 - i);
                        nl ^= ds << (i + 1);
                    }
                }
            }
            for i in (k..=63).rev() {
                if nl & (1u64 << i) != 0 {
                    nl ^= ds >> (63 - i);
                }
            }
            nl
        }
        fn polymmult_ref(x: u64, y: u64, d: u64) -> u64 {
            let (mut ph, mut pl) = (0u64, 0u64);
            if x & 1 != 0 {
                pl = y;
            }
            for i in 1..64 {
                if x & (1u64 << i) != 0 {
                    ph ^= y >> (64 - i);
                    pl ^= y << i;
                }
            }
            polymod_ref(ph, pl, d)
        }
        let poly: u64 = 0xbfe6b8a5bf378d83;
        let xshift = fls64_ref(poly) - 1;
        let shift = xshift - 8;
        let t1 = polymod_ref(0, 1u64 << xshift, poly);
        let mut t_ref = [0u64; 256];
        for j in 0..256u64 {
            t_ref[j as usize] = polymmult_ref(j, t1, poly) | (j << xshift);
        }
        let mut sizeshift: u64 = 1;
        for _ in 1..WINDOW_SIZE {
            let idx = (sizeshift >> shift) as usize;
            sizeshift = (sizeshift << 8) ^ t_ref[idx];
        }
        let mut u_ref = [0u64; 256];
        for i in 0..256u64 {
            u_ref[i as usize] = polymmult_ref(i, sizeshift, poly);
        }

        let append8_ref =
            |p: u64, m: u8| -> u64 { ((p << 8) | m as u64) ^ t_ref[(p >> shift) as usize] };
        let mut ref_fp: u64 = 0;
        let mut ref_window = [0u8; WINDOW_SIZE];
        let mut ref_pos: usize = WINDOW_SIZE % WINDOW_SIZE;

        let mut prod = RabinState::new();

        let data: Vec<u8> = (0..(500 * 1024))
            .map(|i: usize| (i.wrapping_mul(37) ^ (i >> 5) ^ (i.wrapping_mul(13))) as u8)
            .collect();

        prod.init(&data);
        for i in 0..WINDOW_SIZE {
            ref_fp = append8_ref(ref_fp, data[i]) & 0xFFFF_FFFF;
            ref_window[i] = data[i];
        }
        assert_eq!(
            prod.fingerprint, ref_fp,
            "fingerprint mismatch after init (first {} bytes)",
            WINDOW_SIZE
        );

        // Rolling: compare fingerprint at EVERY byte position
        let mut mismatch_count = 0;
        let mut first_mismatch = None;
        for (pos, &byte) in data.iter().enumerate().skip(WINDOW_SIZE) {
            let out = ref_window[ref_pos];
            ref_fp = append8_ref(ref_fp ^ u_ref[out as usize], byte) & 0xFFFF_FFFF;
            ref_window[ref_pos] = byte;
            ref_pos = (ref_pos + 1) % WINDOW_SIZE;

            prod.update(byte);

            if prod.fingerprint != ref_fp {
                mismatch_count += 1;
                if first_mismatch.is_none() {
                    first_mismatch = Some(pos);
                }
            }
        }

        if mismatch_count > 0 {
            panic!(
                "fingerprint stream diverged at {} positions (first at byte {}). \
                 CDC chunk boundaries WILL NOT match seafile.",
                mismatch_count,
                first_mismatch.unwrap()
            );
        }

        // Also check the u32 truncation seafile uses for break-point decisions.
        let prod_fp_u32 = prod.get_fingerprint();
        let ref_fp_u32 = ref_fp as u32;
        assert_eq!(
            prod_fp_u32, ref_fp_u32,
            "final fingerprint u32 mismatch: prod={:08x} ref={:08x}",
            prod_fp_u32, ref_fp_u32
        );

        println!(
            "Verified {} byte positions — all fingerprints match",
            data.len() - WINDOW_SIZE
        );
    }

    /// Verify CDC boundary conditions: min block size is respected.
    #[test]
    fn test_cdc_min_block_respected() {
        let data = vec![0u8; TEST_SIZES.min + 100]; // just above min
        let chunks = file_chunk_cdc_with(&data, TEST_SIZES);
        // With uniform zero data there may be no break points: every non-final chunk must
        // be >= min or == max.
        for (i, &(_offset, size)) in chunks.iter().enumerate() {
            if i < chunks.len() - 1 {
                assert!(
                    size >= TEST_SIZES.min || size == TEST_SIZES.max,
                    "non-last chunk {} size {} violated bounds",
                    i,
                    size
                );
            }
        }
    }
}
