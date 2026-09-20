//! The message an admin page shows for a failed action.
//!
//! One definition, so the admin pages cannot disagree about what is safe to put
//! in front of a user: our own client-facing errors are displayed verbatim,
//! while an internal failure keeps its detail in the log and shows a translated
//! generic message instead.

use crate::i18n::I18n;
use base::error::AppError;

/// The banner text for a failed admin action.
///
/// `BadRequest` is the variant these services use for a condition the caller
/// can act on (a duplicate email, a rejected value), so its message is meant to
/// be read. Everything else — a database error, a bug — is summarised.
pub fn action_error(t: &I18n, error: &AppError) -> String {
    match error {
        AppError::BadRequest(message) => message.clone(),
        _ => t.tr("admin.action_failed").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_facing_error_is_shown_verbatim() {
        let t = I18n::get(Some("en"));
        assert_eq!(
            action_error(t, &AppError::BadRequest("user already exists".into())),
            "user already exists"
        );
    }

    #[test]
    fn an_internal_failure_shows_the_generic_message() {
        let t = I18n::get(Some("en"));
        let message = action_error(t, &AppError::Internal("connection reset by peer".into()));
        assert_eq!(message, "The action could not be completed.");
        // The detail belongs in the log, not on the page.
        assert!(!message.contains("connection reset"));
    }

    #[test]
    fn the_generic_message_follows_the_reader_language() {
        assert_eq!(
            action_error(I18n::get(Some("zh")), &AppError::Internal("boom".into())),
            "操作未能完成。"
        );
    }
}
