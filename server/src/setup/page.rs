//! The pages the setup listener renders.
//!
//! Three small templates rather than one with flags: the form, the page that
//! says the setup is finished (so the browser can be refreshed or reopened), and
//! the notice every other entry point gets. All three extend `auth_base.html`,
//! which is the anonymous shell the sign-in and password-reset pages use — the
//! setup page is read before anybody has an account, so it belongs to exactly
//! that family.

use askama::Template;

use crate::i18n::I18n;
use crate::static_assets::TemplateUrls;

/// The first-run form.
#[derive(Template)]
#[template(path = "page/setup.html")]
pub struct SetupTemplate {
    pub urls: &'static TemplateUrls,
    pub t: &'static I18n,
    /// The one-time credential, part of the form's action.
    pub token: String,
    pub email: String,
    pub addr: String,
    pub port: String,
    pub site_url: String,
    pub data_dir: String,
    pub encryption: String,
    /// Where the configuration will be written, shown before it happens: this is
    /// the file the operator will have to find later.
    pub config_path: String,
    /// The configured address could not be bound, so this page is being served
    /// on a random port.
    pub port_in_use: bool,
    /// Accounts already found in the default data directory.
    pub existing_accounts: Option<u64>,
    /// The password rule, in words.
    pub password_hint: String,
    /// Empty for a fresh form; otherwise the message shown as the error alert.
    pub error: String,
}

/// What a completed setup says.
#[derive(Template)]
#[template(path = "page/setup_done.html")]
pub struct SetupDoneTemplate {
    pub urls: &'static TemplateUrls,
    pub t: &'static I18n,
    pub site_url: String,
    pub config_path: String,
    pub data_dir: String,
    /// Whether the page's own socket became the server's, which decides whether
    /// the automatic refresh can be relative.
    pub handover: bool,
    /// The administrator this setup created, when there was no account yet.
    pub created_email: Option<String>,
    /// Accounts the database already had.
    pub accounts_kept: u64,
}

/// Anything else: the port reached without the token, a conflicting submit, or a
/// failure with its reason.
#[derive(Template)]
#[template(path = "page/setup_notice.html")]
pub struct SetupNoticeTemplate {
    pub urls: &'static TemplateUrls,
    pub t: &'static I18n,
    pub heading_key: &'static str,
    pub body_key: &'static str,
    /// One technical line — the configuration path, or the error itself.
    pub detail: String,
}
