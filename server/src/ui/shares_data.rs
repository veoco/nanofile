//! Row loading shared by the user shares page and the admin shares page.
//!
//! Both pages read the same tables and need the same batched repository and user
//! lookups; one loader and one row shape keep their error handling from drifting
//! apart. Only the link scope (own vs all) and the template differ; deletion is
//! the sharing service's job.

use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

/// One share link as both pages render it.
pub struct ShareLinkRow {
    pub token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub creator_email: String,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub has_password: bool,
    pub view_cnt: i64,
    pub s_type: String,
    pub link_url: String,
    pub description: Option<String>,
}

/// One upload link as both pages render it.
pub struct UploadLinkRow {
    pub token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub creator_email: String,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub has_password: bool,
    pub view_cnt: i64,
    pub link_url: String,
    pub description: Option<String>,
}

pub struct SharePageLinks {
    pub share_links: Vec<ShareLinkRow>,
    pub upload_links: Vec<UploadLinkRow>,
}

/// Whose links to load.
#[derive(Clone, Copy)]
pub enum LinkScope {
    /// The caller's own links (the user page); empty when that direction is off.
    Own(i32),
    /// Every link (the admin page); the direction switches do not hide rows there.
    All,
}

/// Load both link kinds and their repository/creator labels in batched queries.
pub async fn load_links(
    state: &Arc<AppState>,
    scope: LinkScope,
) -> Result<SharePageLinks, AppError> {
    let download_enabled = state.config().server.share_download_enabled;
    let upload_enabled = state.config().server.share_upload_enabled;

    let (share_models, upload_models) = match scope {
        LinkScope::Own(user_id) => {
            let shares = if download_enabled {
                state.repos.share_link.find_by_creator_id(user_id).await?
            } else {
                Vec::new()
            };
            let uploads = if upload_enabled {
                state.repos.upload_link.find_by_creator_id(user_id).await?
            } else {
                Vec::new()
            };
            (shares, uploads)
        }
        LinkScope::All => (
            state.repos.share_link.find_all().await?,
            state.repos.upload_link.find_all().await?,
        ),
    };

    let mut repo_ids: Vec<String> = Vec::new();
    let mut creator_ids: Vec<i32> = Vec::new();
    for s in &share_models {
        if !repo_ids.contains(&s.repo_id) {
            repo_ids.push(s.repo_id.clone());
        }
        if !creator_ids.contains(&s.creator_id) {
            creator_ids.push(s.creator_id);
        }
    }
    for u in &upload_models {
        if !repo_ids.contains(&u.repo_id) {
            repo_ids.push(u.repo_id.clone());
        }
        if !creator_ids.contains(&u.creator_id) {
            creator_ids.push(u.creator_id);
        }
    }

    let mut repo_names: HashMap<String, String> = HashMap::new();
    if !repo_ids.is_empty() {
        for repo in state.repos.repo.find_by_ids(&repo_ids).await? {
            repo_names.insert(repo.id, repo.name);
        }
    }
    // Creator labels are only rendered by the admin page; the user page's own rows
    // all carry the caller's address, so skip the lookup there.
    let mut creator_emails: HashMap<i32, String> = HashMap::new();
    if matches!(scope, LinkScope::All) && !creator_ids.is_empty() {
        for user in state.repos.user.find_by_ids(&creator_ids).await? {
            creator_emails.insert(user.id, user.email);
        }
    }

    let share_links = share_models
        .into_iter()
        .map(|s| ShareLinkRow {
            token: s.token.clone(),
            repo_id: s.repo_id.clone(),
            repo_name: repo_names.get(&s.repo_id).cloned().unwrap_or_default(),
            path: s.path.clone(),
            name: entry_name(&s.path),
            creator_email: creator_emails
                .get(&s.creator_id)
                .cloned()
                .unwrap_or_default(),
            created_at_ts: s.created_at,
            expires_at_ts: s.expires_at,
            has_password: s.password.is_some(),
            view_cnt: s.view_cnt,
            s_type: s.s_type.clone(),
            link_url: if s.s_type == "d" {
                format!("/d/{}/", s.token)
            } else {
                format!("/f/{}/", s.token)
            },
            description: s.description,
        })
        .collect();

    let upload_links = upload_models
        .into_iter()
        .map(|u| UploadLinkRow {
            token: u.token.clone(),
            repo_id: u.repo_id.clone(),
            repo_name: repo_names.get(&u.repo_id).cloned().unwrap_or_default(),
            path: u.path.clone(),
            name: upload_link_name(&u.path),
            creator_email: creator_emails
                .get(&u.creator_id)
                .cloned()
                .unwrap_or_default(),
            created_at_ts: u.created_at,
            expires_at_ts: u.expires_at,
            has_password: u.password.is_some(),
            view_cnt: u.view_cnt,
            link_url: format!("/u/{}/", u.token),
            description: u.description,
        })
        .collect();

    Ok(SharePageLinks {
        share_links,
        upload_links,
    })
}

/// The name shown for an upload link: the final path segment, or the original
/// path for a whole-library link.
fn upload_link_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path.to_string()
    } else {
        entry_name(trimmed)
    }
}

/// The displayed name of the shared entry: the last path segment, or the whole
/// path for a root share.
fn entry_name(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_upload_link_keeps_its_path_as_the_name() {
        assert_eq!(upload_link_name("/"), "/");
        assert_eq!(upload_link_name("/subdir"), "subdir");
        assert_eq!(upload_link_name("/subdir/"), "subdir");
    }

    #[test]
    fn share_entries_use_their_last_segment() {
        assert_eq!(entry_name("/dir/file.txt"), "file.txt");
        assert_eq!(entry_name("file.txt"), "file.txt");
    }
}
