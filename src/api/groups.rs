use axum::{
    Json,
    extract::{Query, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{group, group_member, user, user_contact};
use crate::error::AppError;

/// GET /api2/groups/
pub async fn list_groups(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let memberships = group_member::Entity::find()
        .filter(group_member::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let mut result = Vec::new();
    for m in &memberships {
        if let Some(g) = group::Entity::find_by_id(m.group_id)
            .one(state.db.as_ref())
            .await?
        {
            let creator = user::Entity::find_by_id(g.creator_id)
                .one(state.db.as_ref())
                .await?;

            let member_count = group_member::Entity::find()
                .filter(group_member::Column::GroupId.eq(g.id))
                .all(state.db.as_ref())
                .await
                .unwrap_or_default()
                .len() as i64;

            result.push(serde_json::json!({
                "id": g.id,
                "name": g.name,
                "creator_name": creator.map(|u| u.email).unwrap_or_default(),
                "created_at": g.created_at,
                "member_count": member_count,
            }));
        }
    }

    Ok(Json(result))
}

/// GET /api2/groupandcontacts/
pub async fn groups_and_contacts(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let memberships = group_member::Entity::find()
        .filter(group_member::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let mut groups_list = Vec::new();
    for m in &memberships {
        if let Some(g) = group::Entity::find_by_id(m.group_id)
            .one(state.db.as_ref())
            .await?
        {
            let creator = user::Entity::find_by_id(g.creator_id)
                .one(state.db.as_ref())
                .await?;

            groups_list.push(serde_json::json!({
                "id": g.id,
                "name": g.name,
                "creator_name": creator.map(|u| u.email).unwrap_or_default(),
                "created_at": g.created_at,
            }));
        }
    }

    let contacts = user_contact::Entity::find()
        .filter(user_contact::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let contacts_list: Vec<serde_json::Value> = contacts
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "email": c.contact_email,
                "name": c.contact_name,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "groups": groups_list,
        "contacts": contacts_list,
    })))
}

/// GET /api2/search-user/?q=
#[derive(Deserialize)]
pub struct SearchUserQuery {
    pub q: Option<String>,
}

pub async fn search_user(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchUserQuery>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let q = query.q.unwrap_or_default();
    if q.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let pattern = format!("%{}%", q);
    let users = user::Entity::find()
        .filter(user::Column::Email.like(&pattern))
        .all(state.db.as_ref())
        .await?;

    let result: Vec<serde_json::Value> = users
        .into_iter()
        .map(|u| {
            serde_json::json!({
                "email": u.email,
                "name": u.email,
                "contact_email": u.email,
            })
        })
        .collect();

    Ok(Json(result))
}
