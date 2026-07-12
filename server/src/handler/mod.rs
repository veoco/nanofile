//! HTTP handler layer — thin adapters between axum and the service layer.
//!
//! REST API handlers live as flat files under `handler/`, grouped by domain.
//! The sync protocol (`/seafhttp/`) and file serving (`/download-api/`, etc.)
//! each have their own subdirectories due to their distinct auth patterns.

pub mod account;
pub mod activities;
pub mod async_batch;
pub mod avatar;
pub mod batch;
pub mod chunked_upload;
pub mod client_login;
pub mod device_wipe;
pub mod devices;
pub mod dir;
pub mod exif;
pub mod file;
pub mod fileops;
pub mod groups;
pub mod history;
pub mod invitations;
pub mod links;
pub mod login;
pub mod metadata;
pub mod notification;
pub mod notifications;
pub mod password;
pub mod reindex;
pub mod repos;
pub mod search;
pub mod server_info;
pub mod share;
pub mod smart_link;
pub mod sso;
pub mod starred;
pub mod sync;
pub mod thumbnail;
pub mod trash;
pub mod two_factor;
pub mod upload_link;
pub mod user_avatar;
pub mod users;
pub mod web;
pub mod wiki;
