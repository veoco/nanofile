//! Sharing, groups, tags and activity.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod activity_test;
mod groups_compat_test;
mod history_settings_test;
mod notification_test;
mod share_link_test;
mod starred_test;
mod tags_test;
mod task_load_test;
