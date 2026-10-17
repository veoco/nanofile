//! Business logic service layer — all domain-level operations, organized by domain.
//!
//! Services depend on repository traits and domain types; they never import axum or HTTP concerns.

pub mod activity;
pub mod admin;
pub mod api_key;
pub mod auth;
pub mod credential;
pub mod fs;
pub mod index;
pub mod mail;
pub mod repo;
pub mod sharing;
pub mod sync;
pub mod user;
