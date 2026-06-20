//! # nanofile-domain
//!
//! Pure domain types for nanofile. No axum dependency unless the `with-axum`
//! feature is enabled (for `IntoResponse` on `AppError`).

pub mod common;
pub mod error;
pub mod sanitize;

pub use error::AppError;
