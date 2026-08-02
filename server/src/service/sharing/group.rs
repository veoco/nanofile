use crate::repository::Repositories;
use base::error::AppError;

/// List groups for a user in the official `/api/v2.1/groups/` response format.
///
/// Field layout matches seahub's `get_group_info` (groups.py) and the seadroid
/// `GroupEntity` parser. `group_quota_usage` is always the integer `0`
/// (non-Pro semantics) — never an empty string, which newer Android clients
/// fail to parse as `long` (seahub issue #9342).
pub async fn list_groups_v21(
    repos: &Repositories,
    user_id: i32,
    with_repos: bool,
) -> Result<Vec<serde_json::Value>, AppError> {
    let memberships = repos.group_member.find_by_user_id(user_id).await?;

    let mut result = Vec::new();
    for m in &memberships {
        let Some(g) = repos.group.find_by_id(m.group_id).await? else {
            continue;
        };

        let creator_email = repos.user.find_by_id(g.creator_id).await?.map(|u| u.email);

        // admins: members whose role is Owner/Admin (case-insensitive),
        // plus the group creator (the owner) even if their role is unset.
        let mut admins: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for gm in repos.group_member.find_by_group_id(g.id).await? {
            let is_admin =
                gm.role.eq_ignore_ascii_case("owner") || gm.role.eq_ignore_ascii_case("admin");
            if is_admin
                && let Some(u) = repos.user.find_by_id(gm.user_id).await?
                && seen.insert(u.email.clone())
            {
                admins.push(u.email);
            }
        }
        if let Some(email) = &creator_email
            && seen.insert(email.clone())
        {
            admins.push(email.clone());
        }

        let created_at = chrono::DateTime::from_timestamp(g.created_at, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default();

        let mut info = serde_json::json!({
            "id": g.id,
            "parent_group_id": 0,
            "name": g.name,
            "owner": creator_email.unwrap_or_default(),
            "created_at": created_at,
            "admins": admins,
            "group_quota_usage": 0,
        });
        if with_repos {
            info["repos"] = serde_json::Value::Array(Vec::new());
        }
        result.push(info);
    }

    Ok(result)
}

/// List all groups for a user.
pub async fn list_groups(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let memberships = repos.group_member.find_by_user_id(user_id).await?;

    let mut result = Vec::new();
    for m in &memberships {
        if let Some(g) = repos.group.find_by_id(m.group_id).await? {
            let creator = repos.user.find_by_id(g.creator_id).await?;

            let member_count = repos
                .group_member
                .find_by_group_id(g.id)
                .await
                .unwrap_or_default()
                .len() as i64;

            result.push(serde_json::json!({
                "id": g.id,
                "name": g.name,
                "creator_name": creator.as_ref().map(|u| u.nickname()).unwrap_or_default(),
                "created_at": g.created_at,
                "member_count": member_count,
            }));
        }
    }

    Ok(result)
}

/// List groups and contacts for a user.
pub async fn groups_and_contacts(
    repos: &Repositories,
    user_id: i32,
) -> Result<serde_json::Value, AppError> {
    let memberships = repos.group_member.find_by_user_id(user_id).await?;

    let mut groups_list = Vec::new();
    for m in &memberships {
        if let Some(g) = repos.group.find_by_id(m.group_id).await? {
            let creator = repos.user.find_by_id(g.creator_id).await?;

            groups_list.push(serde_json::json!({
                "id": g.id,
                "name": g.name,
                "creator_name": creator.as_ref().map(|u| u.nickname()).unwrap_or_default(),
                "created_at": g.created_at,
            }));
        }
    }

    let contacts = repos.user_contact.find_by_user_id(user_id).await?;

    let contacts_list: Vec<serde_json::Value> = contacts
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "email": c.contact_email,
                "name": c.contact_name,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "groups": groups_list,
        "contacts": contacts_list,
    }))
}

/// Search users by email pattern.
pub async fn search_user(
    repos: &Repositories,
    query: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let pattern = format!("%{}%", query);
    let users = repos.user.find_by_email_like(&pattern).await?;

    let result: Vec<serde_json::Value> = users
        .into_iter()
        .map(|u| {
            serde_json::json!({
                "email": u.email,
                "name": u.nickname(),
                "contact_email": u.email,
            })
        })
        .collect();

    Ok(result)
}
