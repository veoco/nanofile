//! Business logic service layer — all domain-level operations.
//!
//! Organized by domain (auth, fs, repo, sharing, user, admin, activity).
//! Services depend on repository traits and domain types; they never
//! import axum or HTTP concerns directly.

pub mod activity;
pub mod admin;
pub mod auth;
pub mod fs;
pub mod repo;
pub mod sdoc;
pub mod sharing;
pub mod sync;
pub mod user;
