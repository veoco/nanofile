//! The jobs a confined child can run.
//!
//! Each owns its request and reply framing; the transport
//! ([`crate::sandbox::worker`]) carries bytes and one tag, so one child binary serves
//! every pipeline and no job can read another's reply. All of them turn user bytes
//! into server state on the far side of the process boundary.

mod exif;
pub mod faces;
pub mod images;
pub mod media;
pub mod ocr;

use std::sync::{Arc, OnceLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// One per-profile concurrency ceiling.
///
/// Each job had its own `OnceLock<Arc<Semaphore>>` + `acquire_owned` + error
/// format copy. The ceilings stay per job (OCR must not hold an image permit), but
/// the mechanism, the first-use initialisation and the error wording are shared.
pub(crate) struct JobGate {
    permits: OnceLock<Arc<Semaphore>>,
    max: usize,
    name: &'static str,
}

impl JobGate {
    pub(crate) const fn new(max: usize, name: &'static str) -> Self {
        Self {
            permits: OnceLock::new(),
            max,
            name,
        }
    }

    /// Acquire a permit covering one child of this profile; callers queue rather
    /// than fail.
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, String> {
        let permits = self
            .permits
            .get_or_init(|| Arc::new(Semaphore::new(self.max)))
            .clone();
        permits
            .acquire_owned()
            .await
            .map_err(|error| format!("{} concurrency gate failed: {error}", self.name))
    }
}

/// One token out of a failure message: the probe detail is comma-separated and
/// whitespace-free, so the parent's `item_verdict` can split on it.
pub(crate) fn token(why: &str) -> String {
    why.chars()
        .map(|character| {
            if character.is_whitespace() || character == ',' {
                '_'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::token;

    #[test]
    fn a_token_has_no_commas_or_whitespace() {
        assert_eq!(
            token("model missing, retry later"),
            "model_missing__retry_later"
        );
        assert_eq!(token("ok"), "ok");
    }
}
