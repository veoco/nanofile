//! Dump CDC block boundaries for a file, for differential testing against the
//! upstream seafile C implementation.
//!
//! The upstream reference is `common/cdc/cdc.c` + `common/cdc/rabin-checksum.c`
//! run with `block_sz = 8 MiB`, `block_min_sz = 6 MiB`, `block_max_sz = 10 MiB`
//! (seafile's `CDC_AVERAGE_BLOCK_SIZE` / `CDC_MIN_BLOCK_SIZE` /
//! `CDC_MAX_BLOCK_SIZE`). Its block sizes, one per line, must be identical to
//! this program's output for the same input file — for both the whole-buffer
//! and the streaming path, and for any feed size. `test_default_boundaries_
//! match_seafile_c_reference` in `src/storage/cdc.rs` locks in one such vector.
//!
//! Usage:
//!   cdc_dump <file>              # whole-buffer file_chunk_cdc: one size per line
//!   cdc_dump <file> stream <n>   # streaming Chunker fed in `n`-byte slices

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read input");
    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("whole");

    match mode {
        "whole" => {
            for (_off, size) in infra::storage::cdc::file_chunk_cdc(&data) {
                println!("{size}");
            }
        }
        "stream" => {
            let feed: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(65536);
            let mut ch = infra::storage::cdc::Chunker::new(data.len());
            let mut sizes = Vec::new();
            for part in data.chunks(feed) {
                for blk in ch.feed(part) {
                    sizes.push(blk.len());
                }
            }
            let tail = ch.finish();
            if !tail.is_empty() {
                sizes.push(tail.len());
            }
            for s in sizes {
                println!("{s}");
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
