//! API surface, error rendering, docs guards and compatibility.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod api_key_auth_test;
mod api_key_repository_test;
mod api_key_test;
mod api_key_ui_test;
mod api_v1_test;
mod api_v21_test;
mod business_logic_test;
mod capability_coverage_test;
mod client_compat_test;
mod csrf_integration_test;
mod desktop_endpoints_test;
mod docs_api_reference_test;
mod error_page_test;
mod error_test;
mod i18n_test;
mod metadata_test;
mod server_info_test;
mod ui_test;
mod web_access_test;
