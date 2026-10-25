//! Behavioral contract for the content-defined chunker.
//!
//! These tests describe the required boundaries and streaming semantics. Golden
//! sizes are recorded as data; the test suite does not contain a second copy of
//! the chunking algorithm.

use infra::storage::cdc::{
    ChunkSizes, Chunker, SEAFILE_AVG_BLOCK, SEAFILE_MAX_BLOCK, SEAFILE_MIN_BLOCK, file_chunk_cdc,
    file_chunk_cdc_with,
};

/// Deterministic bytes used to pin the production boundary vector.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push((state >> 33) as u8);
    }
    out
}

fn sizes_of(chunks: &[(usize, usize)]) -> Vec<usize> {
    chunks.iter().map(|(_, len)| *len).collect()
}

fn assert_contiguous(chunks: &[(usize, usize)], total: usize) {
    let mut offset = 0;
    for &(start, len) in chunks {
        assert_eq!(start, offset, "chunk offsets must be contiguous");
        assert!(len > 0, "chunks must be non-empty");
        offset += len;
    }
    assert_eq!(offset, total, "chunks must cover the whole input");
}

fn assert_bounds(chunks: &[(usize, usize)], min: usize, max: usize) {
    for (i, &(_, len)) in chunks.iter().enumerate() {
        assert!(len <= max, "chunk {i} is {len} bytes, above max {max}");
        if i + 1 < chunks.len() {
            assert!(
                len >= min,
                "non-final chunk {i} is {len} bytes, below min {min}"
            );
        }
    }
}

fn collect_stream(
    data: &[u8],
    feed_len: usize,
    sizes: ChunkSizes,
    file_size: usize,
) -> Vec<Vec<u8>> {
    let mut chunker = if file_size == 0 {
        Chunker::with_sizes(0, sizes)
    } else {
        Chunker::with_sizes(file_size, sizes)
    };
    let mut blocks = Vec::new();
    for slice in data.chunks(feed_len.max(1)) {
        blocks.extend(chunker.feed(slice));
    }
    let tail = chunker.finish();
    if !tail.is_empty() {
        blocks.push(tail);
    }
    blocks
}

const SMALL: ChunkSizes = ChunkSizes {
    avg: 1024,
    min: 256,
    max: 4096,
};

#[test]
fn production_sizes_are_the_documented_constants() {
    assert_eq!(SEAFILE_AVG_BLOCK, 8 * 1024 * 1024);
    assert_eq!(SEAFILE_MIN_BLOCK, 6 * 1024 * 1024);
    assert_eq!(SEAFILE_MAX_BLOCK, 10 * 1024 * 1024);
    assert_eq!(ChunkSizes::SEAFILE.avg, SEAFILE_AVG_BLOCK);
    assert_eq!(ChunkSizes::SEAFILE.min, SEAFILE_MIN_BLOCK);
    assert_eq!(ChunkSizes::SEAFILE.max, SEAFILE_MAX_BLOCK);
}

#[test]
fn production_boundary_vector_is_stable() {
    const EXPECTED: &[usize] = &[6_431_339, 10_485_760, 10_485_760, 10_485_760, 4_054_421];
    let data = pseudo_random(40 * 1024 * 1024, 1);
    let chunks = file_chunk_cdc(&data);
    assert_eq!(sizes_of(&chunks), EXPECTED);
    assert_contiguous(&chunks, data.len());
    assert_bounds(&chunks, SEAFILE_MIN_BLOCK, SEAFILE_MAX_BLOCK);
}

#[test]
fn empty_input_has_no_chunks() {
    assert!(file_chunk_cdc(&[]).is_empty());
    assert!(file_chunk_cdc_with(&[], SMALL).is_empty());
    let mut chunker = Chunker::new(0);
    assert!(chunker.feed(&[]).is_empty());
    assert!(chunker.finish().is_empty());
}

#[test]
fn short_input_is_one_chunk() {
    for len in [1usize, 47, 48, SMALL.min - 1, SMALL.min, SMALL.min + 1] {
        let data = pseudo_random(len, 7);
        let chunks = file_chunk_cdc_with(&data, SMALL);
        assert_contiguous(&chunks, len);
        assert_eq!(chunks.len(), 1, "len {len}");
        assert_eq!(chunks[0], (0, len));
    }
}

#[test]
fn custom_sizes_cover_the_input() {
    for len in [SMALL.min + 2, SMALL.max, SMALL.max + 1, SMALL.max * 3 + 17] {
        let data = pseudo_random(len, 11);
        let chunks = file_chunk_cdc_with(&data, SMALL);
        assert_contiguous(&chunks, len);
        assert_bounds(&chunks, SMALL.min, SMALL.max);
    }
}

#[test]
fn zeros_and_ffs_cover_the_input() {
    for fill in [0x00u8, 0xFF] {
        let data = vec![fill; SMALL.max * 2 + 333];
        let chunks = file_chunk_cdc_with(&data, SMALL);
        assert_contiguous(&chunks, data.len());
        assert_bounds(&chunks, SMALL.min, SMALL.max);
    }
}

#[test]
fn streaming_matches_one_shot_for_every_feed_size() {
    let data = pseudo_random(2 * 1024 * 1024 + 12345, 42);
    let expected: Vec<Vec<u8>> = file_chunk_cdc_with(&data, SMALL)
        .into_iter()
        .map(|(start, len)| data[start..start + len].to_vec())
        .collect();

    for feed_len in [1usize, 4096, 65_536, 1024 * 1024, 5 * 1024 * 1024] {
        let got = collect_stream(&data, feed_len, SMALL, data.len());
        assert_eq!(got, expected, "feed_len={feed_len}");
    }
}

#[test]
fn streaming_unknown_size_matches_one_shot() {
    let data = pseudo_random(3 * SMALL.max + 99, 99);
    let expected: Vec<Vec<u8>> = file_chunk_cdc_with(&data, SMALL)
        .into_iter()
        .map(|(start, len)| data[start..start + len].to_vec())
        .collect();

    for feed_len in [1usize, 777, 8192, 65_536] {
        let got = collect_stream(&data, feed_len, SMALL, 0);
        assert_eq!(got, expected, "feed_len={feed_len}");
    }
}

#[test]
fn production_streaming_matches_one_shot_on_a_medium_file() {
    let data = pseudo_random(30 * 1024 * 1024 + 7, 5);
    let expected = sizes_of(&file_chunk_cdc(&data));
    for feed_len in [65_536usize, 1024 * 1024, 5 * 1024 * 1024] {
        let got: Vec<usize> = collect_stream(&data, feed_len, ChunkSizes::SEAFILE, data.len())
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(got, expected, "feed_len={feed_len}");
    }
}
