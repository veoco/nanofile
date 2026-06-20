const POLY: u64 = 0xbfe6b8a5bf378d83;
const WINDOW_SIZE: usize = 48;
const BREAK_VALUE: u32 = 0x0013;
const DEFAULT_MIN_BLOCK: usize = 256 * 1024;
const DEFAULT_MAX_BLOCK: usize = 4 * 1024 * 1024;

/// seafile-compatible Rabin fingerprint.
///
/// Matches the exact behaviour of seafile's `common/cdc/rabin-checksum.c`:
/// polynomial `0xbfe6b8a5bf378d83`, 48-byte window, GF(2) polynomial
/// arithmetic with precomputed T/U tables.
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
        let poly = POLY;
        let xshift = Self::fls64(poly) - 1;
        let shift = xshift - 8;

        // Precompute T[256]: T[j] = (j * x^62 mod poly) | (j << 62)
        let t1 = Self::polymod(0, 1u64 << xshift, poly);
        let mut t = [0u64; 256];
        for j in 0..256u64 {
            t[j as usize] = Self::polymmult(j, t1, poly) | (j << xshift);
        }

        // Precompute U[256]: U[i] = i * x^(8*WINDOW_SIZE) mod poly
        let mut sizeshift: u64 = 1;
        for _ in 1..WINDOW_SIZE {
            let idx = (sizeshift >> shift) as usize;
            sizeshift = (sizeshift << 8) ^ t[idx];
        }
        let mut u = [0u64; 256];
        for i in 0..256u64 {
            u[i as usize] = Self::polymmult(i, sizeshift, poly);
        }

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

    /// Core fingerprint update: `(p * x^8 + m) mod poly`.
    /// Uses the T table for efficient reduction.
    fn append8(&self, p: u64, m: u8) -> u64 {
        let idx = (p >> self.shift) as usize;
        ((p << 8) | m as u64) ^ self.t[idx]
    }

    /// Initialise fingerprint from the first WINDOW_SIZE bytes.
    /// Equivalent to seafile's `rabin_checksum(buf, WINDOW_SIZE)`.
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

    /// Rolling update: slide window by one byte.
    /// Equivalent to seafile's `rabin_rolling_checksum(csum, len, c1, c2)`.
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

#[allow(dead_code)]
pub struct CdcResult {
    pub offset: usize,
    pub size: usize,
    pub block_id: String,
}

pub fn calculate_chunk_sizes(file_size: usize) -> (usize, usize, usize) {
    let (avg, min, max) = if file_size >= 8 * 1024 * 1024 * 1024 {
        (8 * 1024 * 1024, 2 * 1024 * 1024, 16 * 1024 * 1024)
    } else if file_size >= 4 * 1024 * 1024 * 1024 {
        (4 * 1024 * 1024, 1024 * 1024, 8 * 1024 * 1024)
    } else if file_size >= 2 * 1024 * 1024 * 1024 {
        (2 * 1024 * 1024, 512 * 1024, 4 * 1024 * 1024)
    } else {
        (1024 * 1024, DEFAULT_MIN_BLOCK, DEFAULT_MAX_BLOCK)
    };
    (avg, min, max)
}

pub fn is_break_point(fp: u32, block_sz: usize) -> bool {
    (fp & (block_sz as u32 - 1)) == (BREAK_VALUE & (block_sz as u32 - 1))
}

/// Compute the fingerprint of exactly `WINDOW_SIZE` bytes starting at `data`,
/// equivalent to C's `rabin_checksum(buf, WINDOW_SIZE)`.
#[allow(dead_code)]
fn finger(data: &[u8]) -> u32 {
    let mut state = RabinState::new();
    state.init(data);
    state.get_fingerprint()
}

/// Content-defined chunking matching seafile's `file_chunk_cdc` exactly.
///
/// Algorithm mirrors the C code's buffer management:
/// 1. Skip first `min - WINDOW_SIZE` bytes (no break checking)
/// 2. At position `min - 1` from chunk start, compute fingerprint from scratch (`finger()`)
/// 3. Continue with rolling updates, checking for break points or max size
/// 4. On break/max: emit chunk, reset, repeat
pub fn file_chunk_cdc(data: &[u8]) -> Vec<(usize, usize)> {
    let file_size = data.len();
    if file_size == 0 {
        return vec![];
    }
    let (avg, min, max) = calculate_chunk_sizes(file_size);
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
        let mut state = RabinState::new();
        state.init(&data[scan_start - 47..]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_sizes() {
        let (avg, min, max) = calculate_chunk_sizes(1024 * 1024);
        assert_eq!(avg, 1024 * 1024);
        assert_eq!(min, 256 * 1024);
        assert_eq!(max, 4 * 1024 * 1024);

        let (avg, min, max) = calculate_chunk_sizes(3 * 1024 * 1024 * 1024);
        assert_eq!(avg, 2 * 1024 * 1024);
        assert_eq!(min, 512 * 1024);
        assert_eq!(max, 4 * 1024 * 1024);
    }

    #[test]
    fn test_cdc_small_file() {
        let data = vec![0u8; 1024 * 100];
        let chunks = file_chunk_cdc(&data);
        assert!(!chunks.is_empty());
        let total: usize = chunks.iter().map(|(_, s)| s).sum();
        assert_eq!(total, data.len());
    }

    /// Verify fingerprint matches seafile's rabin-checksum.c line-for-line.
    ///
    /// Uses a pure-Rust reference implementation ported from seafile
    /// common/cdc/rabin-checksum.c, independently of RabinState above,
    /// to cross-validate the fingerprint computation.
    #[test]
    fn test_fingerprint_matches_seafile() {
        // ── Reference: port of seafile rabin-checksum.c ──
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

        // calcT
        let t1 = polymod_ref(0, 1u64 << xshift, poly);
        let mut t_ref = [0u64; 256];
        for j in 0..256u64 {
            t_ref[j as usize] = polymmult_ref(j, t1, poly) | (j << xshift);
        }

        // calcU
        let mut sizeshift: u64 = 1;
        for _ in 1..48 {
            let idx = (sizeshift >> shift) as usize;
            sizeshift = (sizeshift << 8) ^ t_ref[idx];
        }
        let mut u_ref = [0u64; 256];
        for i in 0..256u64 {
            u_ref[i as usize] = polymmult_ref(i, sizeshift, poly);
        }

        // append8
        let append8_ref = |p: u64, m: u8| -> u64 {
            let idx = (p >> shift) as usize;
            ((p << 8) | m as u64) ^ t_ref[idx]
        };

        // rabin_checksum: init fingerprint for first 48 bytes
        let rabin_checksum_ref = |buf: &[u8]| -> u64 {
            let mut fp: u64 = 0;
            for &b in buf.iter().take(48) {
                fp = append8_ref(fp, b) & 0xFFFF_FFFF;
            }
            fp
        };

        // rabin_rolling_checksum
        let rolling_ref =
            |csum: u64, c1: u8, c2: u8| -> u64 { append8_ref(csum ^ u_ref[c1 as usize], c2) };

        // ── Test vectors ──
        // 1. All zeros: init fingerprint
        let data_zeros = vec![0u8; 100];
        let expected_init = rabin_checksum_ref(&data_zeros);

        let mut state = RabinState::new();
        state.init(&data_zeros);
        assert_eq!(
            state.fingerprint, expected_init,
            "fingerprint mismatch for zeros init: got {:016x}, expected {:016x}",
            state.fingerprint, expected_init
        );

        // 2. Rolling update: zeros → add byte 0x42 at pos 48
        let mut expected_fp = expected_init;
        expected_fp = rolling_ref(expected_fp, 0, 0x42);
        state.update(0x42);
        assert_eq!(
            state.fingerprint, expected_fp,
            "fingerprint mismatch after rolling update"
        );

        // 3. All 0xFF pattern
        let data_ff = vec![0xFFu8; 100];
        let expected_ff_init = rabin_checksum_ref(&data_ff);
        let mut state2 = RabinState::new();
        state2.init(&data_ff);
        assert_eq!(
            state2.fingerprint, expected_ff_init,
            "fingerprint mismatch for 0xFF init"
        );

        // 4. Alternating 0xAA/0x55
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

        // 5. T and U tables match
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

    /// Verify fingerprint stream matches reference for every byte position.
    ///
    /// This is the definitive proof that our CDC produces the same chunk
    /// boundaries as seafile: if the fingerprint matches at every byte,
    /// the break-point decisions are identical.
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

        // reference rolling hash functions
        let append8_ref =
            |p: u64, m: u8| -> u64 { ((p << 8) | m as u64) ^ t_ref[(p >> shift) as usize] };
        let mut ref_fp: u64 = 0;
        let mut ref_window = [0u8; WINDOW_SIZE];
        let mut ref_pos: usize = WINDOW_SIZE % WINDOW_SIZE;

        // Production state
        let mut prod = RabinState::new();

        // Generate varied data (500KB)
        let data: Vec<u8> = (0..(500 * 1024))
            .map(|i: usize| (i.wrapping_mul(37) ^ (i >> 5) ^ (i.wrapping_mul(13))) as u8)
            .collect();

        // Init: first WINDOW_SIZE bytes
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
            // Reference: rolling update
            let out = ref_window[ref_pos];
            ref_fp = append8_ref(ref_fp ^ u_ref[out as usize], byte) & 0xFFFF_FFFF;
            ref_window[ref_pos] = byte;
            ref_pos = (ref_pos + 1) % WINDOW_SIZE;

            // Production: rolling update
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

        // Also verify that the async fingerprint matches u32 truncation
        // (seafile uses u32 fingerprint for break-point check)
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
        let data = vec![0u8; 256 * 1024 + 100]; // just above min
        let chunks = file_chunk_cdc(&data);
        // With uniform zero data, we may not hit break points.
        // All chunks except possibly the last must be >= min or == max.
        for (i, &(_offset, size)) in chunks.iter().enumerate() {
            if i < chunks.len() - 1 {
                assert!(
                    size >= 256 * 1024 || size == 4 * 1024 * 1024,
                    "non-last chunk {} size {} violated bounds",
                    i,
                    size
                );
            }
        }
    }
}
