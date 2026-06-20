//! Service for notification-related operations.
//!
//! Nanofile doesn't have a notification system, so the count is always 0.

/// Returns the count of unseen notifications.
/// Always returns 0 as nanofile doesn't have a notification system.
pub fn get_unseen_messages() -> i32 {
    0
}
