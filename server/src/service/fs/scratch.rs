//! Scratch files the confined media worker may read.
//!
//! ffmpeg has to seek its input, so bytes the server already has are streamed into
//! one file under `storage.temp_dir` and exactly that path is granted to the media
//! child. The name is unique per request, so two concurrent decodes cannot truncate
//! each other, and the guard removes the file on every path — a failed decode must
//! not leave the user's bytes behind.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use base::error::AppError;
use infra::storage::DynBlockStorage;

/// A scratch file that is removed when it goes out of scope.
pub(crate) struct ScratchFile {
    path: PathBuf,
}

impl ScratchFile {
    /// Where the bytes were written.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The path as the grant and the request have to agree on it.
    ///
    /// On macOS a Seatbelt `(literal …)` is written for the resolved file, so the
    /// child must open that same path; elsewhere the path is used as written.
    pub(crate) fn granted_path(&self) -> PathBuf {
        crate::sandbox::jobs::media::granted_source(&self.path)
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Stream at most `cap` bytes of a file's blocks into a fresh scratch file.
///
/// `Ok(None)` on a write or stream error (nothing usable was written), which the
/// caller treats as a file that cannot be decoded. The extension is part of the
/// name the helper sees, and some demuxers key on it.
pub(crate) async fn write_blocks(
    directory: &Path,
    stem: &str,
    extension: &str,
    repo_id: &str,
    block_ids: &[String],
    block_store: &DynBlockStorage,
    cap: u64,
) -> Result<Option<ScratchFile>, AppError> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| AppError::Internal(format!("create scratch dir failed: {e}")))?;
    // Unique per request: a name derived from the path alone lets two concurrent
    // misses for one file truncate each other's stream.
    let path = directory.join(format!("{stem}_{}.{extension}", uuid::Uuid::new_v4()));

    let written: Result<(), std::io::Error> = async {
        let mut out = tokio::fs::File::create(&path).await?;
        let mut stream = crate::fs::core::download::stream_blocks(
            repo_id.to_string(),
            block_ids.to_vec(),
            block_store.clone(),
            None,
        );
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = cap.saturating_sub(total);
            if remaining == 0 {
                break;
            }
            let take = remaining.min(chunk.len() as u64) as usize;
            out.write_all(&chunk[..take]).await?;
            total += take as u64;
            if take < chunk.len() {
                break;
            }
        }
        out.flush().await
    }
    .await;

    match written {
        Ok(()) => Ok(Some(ScratchFile { path })),
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            Ok(None)
        }
    }
}
