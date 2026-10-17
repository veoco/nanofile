//! The pages the setup listener renders.
//!
//! Three templates rather than one with flags, all extending `auth_base.html`.

use askama::Template;

use crate::i18n::I18n;
use crate::static_assets::TemplateUrls;

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
    pub config_path: String,
    /// The configured address could not be bound, so this page is served on a random port.
    pub port_in_use: bool,
    pub existing_accounts: Option<u64>,
    pub password_hint: String,
    pub error: String,
}

/// What a completed setup says: the site URL, the paths it wrote, and what happened.
#[derive(Template)]
#[template(path = "page/setup_done.html")]
pub struct SetupDoneTemplate {
    pub urls: &'static TemplateUrls,
    pub t: &'static I18n,
    pub site_url: String,
    pub config_path: String,
    pub data_dir: String,
    /// Whether the page's own socket became the server's: only then can refresh be relative.
    pub handover: bool,
    pub created_email: Option<String>,
    pub accounts_kept: u64,
}

/// Anything else: the port reached without the token, a conflicting submit, or a failure.
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
