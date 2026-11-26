use crate::repository::Repositories;

use base::common::FsFileData;
use base::error::AppError;
use futures::{Stream, StreamExt};
use infra::crypto::random_key::decrypt_block;
use infra::storage::DynBlockStorage;

pub struct Downloader;

/// Decrypt a block on the blocking pool: AES-CBC is CPU-bound and would stall
/// the async worker.
async fn decrypt_block_offload(
    data: Vec<u8>,
    key: &[u8],
    iv: &[u8],
) -> Result<Vec<u8>, std::io::Error> {
    let key = key.to_vec();
    let iv = iv.to_vec();
    // `decrypt_block` returns a non-`Send` `Box<dyn Error>`; map it to `String` in here.
    tokio::task::spawn_blocking(move || decrypt_block(&data, &key, &iv).map_err(|e| e.to_string()))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .map_err(std::io::Error::other)
}

impl Downloader {
    /// Read at most `max_bytes` of a file, for previews that must not load it whole.
    pub async fn download_file_limited(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
        dec_key: Option<(&[u8], &[u8])>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        let (file_data, block_ids) = Self::download_file_stream(repos, repo_id, path).await?;
        Self::read_blocks_limited(
            repo_id,
            block_store,
            &block_ids,
            file_data.size,
            dec_key,
            max_bytes,
        )
        .await
    }

    /// Read at most `max_bytes` from known block IDs, skipping the tree walk.
    pub async fn read_file_limited_from_blocks(
        repo_id: &str,
        block_store: &DynBlockStorage,
        block_ids: &[String],
        size: i64,
        dec_key: Option<(&[u8], &[u8])>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        Self::read_blocks_limited(repo_id, block_store, block_ids, size, dec_key, max_bytes).await
    }

    /// The one truncating block reader behind both preview paths.
    ///
    /// A partial read must stop at `max_bytes` and never load the rest of the
    /// file: callers use it for thumbnails and previews, and the two copies had to
    /// keep the encrypted-block and truncation semantics in step by hand.
    async fn read_blocks_limited(
        repo_id: &str,
        block_store: &DynBlockStorage,
        block_ids: &[String],
        size: i64,
        dec_key: Option<(&[u8], &[u8])>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        // Clamp before the cast: a negative `size` would wrap to `usize::MAX` and overflow.
        let mut out = Vec::with_capacity(size.max(0).min(max_bytes as i64) as usize);
        for block_id in block_ids {
            if out.len() >= max_bytes {
                break;
            }
            let block_data = block_store
                .read_block(repo_id, block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block_offload(block_data, key, iv)
                    .await
                    .map_err(|e| AppError::internal(e.to_string()))?
            } else {
                block_data
            };
            let remaining = max_bytes - out.len();
            let take = remaining.min(block_data.len());
            out.extend_from_slice(&block_data[..take]);
            if take < block_data.len() {
                break;
            }
        }
        Ok(out)
    }

    /// Resolve a file's block IDs without reading their content.
    pub async fn resolve_blocks(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        Self::download_file_stream(repos, repo_id, path).await
    }

    pub async fn download_file_stream(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        let head = crate::fs::core::repo_head(repos, repo_id).await?;
        if head.head_commit_id.is_none() {
            return Err(AppError::NotFound("repo has no commits".into()));
        }
        Self::resolve_blocks_from_root(repos, repo_id, &head.root_id, path).await
    }

    /// Resolve block IDs from an already-resolved root fs_id; callers like WebDAV reuse
    /// their head here instead of re-querying.
    pub async fn resolve_blocks_from_root(
        repos: &Repositories,
        repo_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        let fs_id = crate::fs::core::resolve_fs_id(repos, repo_id, root_id, path).await?;

        let file_data =
            crate::fs::core::file_ops::FileOps::read_file_fs_object(repos, repo_id, &fs_id).await?;

        Ok((file_data.clone(), file_data.block_ids))
    }
}

/// Stream the given blocks one at a time as the response body.
///
/// Two blocks in flight (not four) bound a single download's block buffers to
/// roughly 16–20 MiB at Seafile's 8–10 MiB block sizes.
pub fn stream_blocks(
    repo_id: String,
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let repo_id = repo_id.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&repo_id, &block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => decrypt_block_offload(data, k, iv).await?,
                None => data,
            };
            Ok(bytes::Bytes::from(data))
        }
    }))
    .buffered(2)
}

/// Parse a single-range `Range` header (`bytes=start-end`, `bytes=start-`, `bytes=-suffix`)
/// against a known total size. `None` for malformed, multiple-range or unsatisfiable
/// input — the caller then serves the full body with status 200 instead of 206.
pub fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    if total == 0 {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    let start_s = start_s.trim();
    let end_s = end_s.trim();

    if start_s.is_empty() {
        let n: u64 = end_s.parse().ok()?;
        if n == 0 {
            return None;
        }
        return Some((total.saturating_sub(n), total - 1));
    }

    let start: u64 = start_s.parse().ok()?;
    if start >= total {
        return None; // unsatisfiable — serve full body.
    }
    let end = if end_s.is_empty() {
        total - 1
    } else {
        end_s.parse::<u64>().ok()?.min(total - 1)
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

/// Stream only the byte range `[start, end]` (inclusive) of a block-stored file.
/// Block offsets are tracked cumulatively, so no block-size metadata is required.
pub fn range_stream(
    repo_id: String,
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
    start: u64,
    end: u64,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    let iter = block_ids.into_iter();
    futures::stream::unfold((iter, 0u64, false), move |(mut iter, mut pos, done)| {
        let store = block_store.clone();
        let repo_id = repo_id.clone();
        let key = enc_key.clone();
        async move {
            if done {
                return None;
            }

            // Fast-forward past blocks before `start` only when the reported sizes are
            // known to be logical byte offsets: the response is sliced by them and
            // nothing is read while skipping, so a size that is even slightly off shifts
            // every byte after it (an at-rest decorator mis-sized header-less legacy
            // blocks and segmented downloads dropped bytes at the seam).
            //
            // - E2EE reports ciphertext lengths (padding included), not logical offsets,
            //   so `enc_key` disables the skip.
            // - A store that cannot guarantee `block_size == read_block().len()` must not
            //   be trusted to skip either.
            let mut expected_block_len: Option<u64> = None;
            if key.is_none() && store.logical_sizes_are_exact() && pos < start {
                loop {
                    // Peek the next id without cloning the iterator: cloning `IntoIter`
                    // copies every remaining entry, which is O(N²) for a large prefix skip.
                    let next_id = match iter.as_slice().first().cloned() {
                        Some(id) => id,
                        None => return Some((Ok(bytes::Bytes::new()), (iter, pos, true))),
                    };
                    let size = match store.block_size(&repo_id, &next_id).await {
                        Ok(s) if s >= 0 => s as u64,
                        _ => break, // fall through to the read path
                    };
                    if pos + size > start {
                        // The size that stopped the skip is this block's start offset.
                        expected_block_len = Some(size);
                        break; // next block intersects the range
                    }
                    pos += size;
                    iter.next(); // consume the skipped block
                }
            }

            let block_id = match iter.next() {
                Some(id) => id,
                None => return Some((Ok(bytes::Bytes::new()), (iter, pos, true))),
            };
            let data = match store.read_block(&repo_id, &block_id).await {
                Ok(d) => d,
                Err(e) => {
                    return Some((Err(std::io::Error::other(e.to_string())), (iter, pos, done)));
                }
            };
            let data = match &key {
                Some((k, iv)) => match decrypt_block_offload(data, k, iv).await {
                    Ok(d) => d,
                    Err(e) => {
                        return Some((Err(e), (iter, pos, done)));
                    }
                },
                None => data,
            };
            let data = bytes::Bytes::from(data);

            // The prefix skip derived this block's start offset from its reported size;
            // a length mismatch means every byte of the response would be shifted, so
            // fail instead of serving a silently mangled range.
            if let Some(expected) = expected_block_len
                && data.len() as u64 != expected
            {
                return Some((
                    Err(std::io::Error::other(format!(
                        "block {block_id} is {} bytes but its reported size was {expected}",
                        data.len()
                    ))),
                    (iter, pos, true),
                ));
            }

            let len = data.len() as u64;
            let block_start = pos;
            let block_end = block_start + len;
            pos = block_end;

            if block_start > end {
                return Some((Ok(bytes::Bytes::new()), (iter, pos, true)));
            }
            // Intersection of [block_start, block_end) with [start, end].
            let rel_start = start.saturating_sub(block_start);
            let rel_end = (end + 1).saturating_sub(block_start);
            if rel_start >= len {
                return Some((Ok(bytes::Bytes::new()), (iter, pos, done)));
            }
            let take = rel_end.min(len) - rel_start.min(len);
            if take == 0 {
                return Some((Ok(bytes::Bytes::new()), (iter, pos, true)));
            }
            let chunk = data.slice(rel_start as usize..(rel_start + take) as usize);
            let finished = block_end > end;
            Some((Ok(chunk), (iter, pos, done || finished)))
        }
    })
}
