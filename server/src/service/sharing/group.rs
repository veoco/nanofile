use std::collections::HashMap;

use crate::repository::Repositories;
use base::error::AppError;
use infra::common::util::timestamp_rfc3339;
use infra::entity::{group, user};

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

    // Batch-load member groups and their creators in two queries.
    let group_ids: Vec<i32> = memberships.iter().map(|m| m.group_id).collect();
    let groups: HashMap<i32, group::Model> = repos
        .group
        .find_by_ids(&group_ids)
        .await?
        .into_iter()
        .map(|g| (g.id, g))
        .collect();
    let creator_ids: Vec<i32> = groups.values().map(|g| g.creator_id).collect();
    let creators: HashMap<i32, user::Model> = repos
        .user
        .find_by_ids(&creator_ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    // Batch-load all members of the user's groups and the admin users they
    // reference, instead of one member + one user query per group.
    let all_members = repos.group_member.find_by_group_ids(&group_ids).await?;
    let mut admin_user_ids_by_group: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut admin_user_ids: Vec<i32> = Vec::new();
    for gm in &all_members {
        let is_admin =
            gm.role.eq_ignore_ascii_case("owner") || gm.role.eq_ignore_ascii_case("admin");
        if is_admin {
            admin_user_ids_by_group
                .entry(gm.group_id)
                .or_default()
                .push(gm.user_id);
            admin_user_ids.push(gm.user_id);
        }
    }
    let admin_email_by_id: HashMap<i32, String> = repos
        .user
        .find_by_ids(&admin_user_ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u.email))
        .collect();

    let mut result = Vec::new();
    for m in &memberships {
        let Some(g) = groups.get(&m.group_id) else {
            continue;
        };

        let creator_email = creators.get(&g.creator_id).map(|u| u.email.clone());

        // admins: members whose role is Owner/Admin (case-insensitive),
        // plus the group creator (the owner) even if their role is unset.
        let mut admins: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(ids) = admin_user_ids_by_group.get(&g.id) {
            for uid in ids {
                if let Some(email) = admin_email_by_id.get(uid)
                    && seen.insert(email.clone())
                {
                    admins.push(email.clone());
                }
            }
        }
        if let Some(email) = &creator_email
            && seen.insert(email.clone())
        {
            admins.push(email.clone());
        }

        let created_at = timestamp_rfc3339(g.created_at);

        let mut info = serde_json::json!({
            "id": g.id,
            "parent_group_id": 0,
            "name": g.name.clone(),
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

    // Batch-load member groups and their creators in two queries.
    let group_ids: Vec<i32> = memberships.iter().map(|m| m.group_id).collect();
    let groups: HashMap<i32, group::Model> = repos
        .group
        .find_by_ids(&group_ids)
        .await?
        .into_iter()
        .map(|g| (g.id, g))
        .collect();
    let creator_ids: Vec<i32> = groups.values().map(|g| g.creator_id).collect();
    let creators: HashMap<i32, user::Model> = repos
        .user
        .find_by_ids(&creator_ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    // Batch-load all members of the user's groups and count per group in
    // memory instead of one COUNT query per group.
    let all_members = repos.group_member.find_by_group_ids(&group_ids).await?;
    let mut member_count_by_group: HashMap<i32, i64> = HashMap::new();
    for gm in &all_members {
        *member_count_by_group.entry(gm.group_id).or_default() += 1;
    }

    let mut result = Vec::new();
    for m in &memberships {
        if let Some(g) = groups.get(&m.group_id) {
            let creator = creators.get(&g.creator_id);

            let member_count = member_count_by_group.get(&g.id).copied().unwrap_or(0);

            result.push(serde_json::json!({
                "id": g.id,
                "name": g.name.clone(),
                "creator_name": creator.map(|u| u.nickname()).unwrap_or_default(),
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

    // Batch-load member groups and their creators in two queries.
    let group_ids: Vec<i32> = memberships.iter().map(|m| m.group_id).collect();
    let groups: HashMap<i32, group::Model> = repos
        .group
        .find_by_ids(&group_ids)
        .await?
        .into_iter()
        .map(|g| (g.id, g))
        .collect();
    let creator_ids: Vec<i32> = groups.values().map(|g| g.creator_id).collect();
    let creators: HashMap<i32, user::Model> = repos
        .user
        .find_by_ids(&creator_ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    let mut groups_list = Vec::new();
    for m in &memberships {
        if let Some(g) = groups.get(&m.group_id) {
            let creator = creators.get(&g.creator_id);

            groups_list.push(serde_json::json!({
                "id": g.id,
                "name": g.name.clone(),
                "creator_name": creator.map(|u| u.nickname()).unwrap_or_default(),
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
