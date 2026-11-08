//! Accounts, auth, SSO, credentials and sessions.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod account_test;
mod auth_test;
mod avatar_test;
mod credential_api_test;
mod credential_bulk_test;
mod credential_page_test;
mod credential_repository_test;
mod devices_test;
mod email_verification_test;
mod login_last_login_test;
mod pow_test;
mod revocation_test;
mod session_source_test;
mod setup_test;
mod sso_hardening_test;
mod sso_test;
