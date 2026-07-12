use crate::repository::Repositories;
use base::error::AppError;

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
