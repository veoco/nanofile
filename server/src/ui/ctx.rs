//! Shared page-context construction for authenticated web UI handlers.

use crate::AppState;
use crate::i18n::I18n;
use crate::service::repo::service::LeftPanelRepo;
use crate::ui::auth_extractor::WebUser;
use base::error::AppError;

/// Common template fields shared by every authenticated page.
pub struct PageCtx {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub left_panel_repos: Vec<LeftPanelRepo>,
}

/// Build the common page context (I18n, template urls, CSRF token and the
/// left-panel repo list) for an authenticated web UI handler.
pub async fn build_page_ctx(state: &AppState, user: &WebUser) -> Result<PageCtx, AppError> {
    let left_panel_repos = state
        .left_panel_cache
        .get_for_user(&state.repos, user.user_id)
        .await?;
    let csrf_token =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    Ok(PageCtx {
        urls: crate::static_assets::template_urls(),
        t: I18n::get(user.language.as_deref()),
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        csrf_token,
        left_panel_repos,
    })
}
