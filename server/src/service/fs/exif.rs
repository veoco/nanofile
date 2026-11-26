use std::sync::Arc;

use crate::fs::core::download::Downloader;
use crate::repository::Repositories;
use base::error::AppError;

pub struct ExifService {
    repos: Arc<Repositories>,
    block_store: infra::storage::DynBlockStorage,
}

impl ExifService {
    pub fn new(repos: Arc<Repositories>, block_store: infra::storage::DynBlockStorage) -> Self {
        Self { repos, block_store }
    }

    /// Extract EXIF metadata as a JSON object, or `null` when the file has none.
    pub async fn get_exif(&self, repo_id: &str, path: &str) -> Result<serde_json::Value, AppError> {
        // EXIF lives in the APP1 segment near the start of the image, so a
        // bounded read is enough and avoids loading huge files into memory.
        let content = Downloader::download_file_limited(
            &self.repos,
            repo_id,
            path,
            &self.block_store,
            None,
            8 * 1024 * 1024,
        )
        .await
        .map_err(|_| AppError::NotFound("file not found".into()))?;

        // The worker being unavailable is an environment problem, distinct from
        // "this image has no EXIF": a switch that is off or a host below the
        // minimum is an error the caller sees, not a silent empty answer.
        if let Err(why) = crate::sandbox::available(crate::sandbox::Profile::Images) {
            return Err(AppError::Internal(format!(
                "EXIF processing is unavailable: {why}"
            )));
        }

        // May run for the whole image timeout, so queue on the shared image gate;
        // the permit covers the wait below.
        let _permit = crate::sandbox::jobs::images::acquire_image_permit()
            .await
            .map_err(|e| AppError::Internal(format!("EXIF worker gate failed: {e}")))?;
        let exif_data =
            tokio::task::spawn_blocking(move || crate::sandbox::jobs::images::exif(&content))
                .await
                .map_err(|e| AppError::Internal(format!("EXIF worker panicked: {e}")))?;

        // A refusal from a running worker is answered as "no EXIF".
        match exif_data {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or(serde_json::Value::Null)),
            Err(_) => Ok(serde_json::Value::Null),
        }
    }
}
