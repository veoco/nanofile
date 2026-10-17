//! Group routes — client-compatibility stubs only.
//!
//! nanofile has no groups: nothing creates, joins or lists one, the `groups` /
//! `group_members` tables were dropped, and no library is shared to a group. These
//! handlers exist because an official client fails on a load path without them:
//!
//! * `GET /api/v2.1/groups/` — seadroid subscribes to it before the library list
//!   (`Objs.java:93-146`); a 404 rejects the whole chain, so it must answer 200 with a
//!   top-level JSON array.
//! * `GET /api2/groups/?with_msg=false` — the desktop group-share dialog
//!   (`requests.cpp:1290`) rejects a non-2xx, and advertising `seafile-pro` is what
//!   makes that dialog reachable.
//!
//! Both answer an empty list, and `with_repos` keeps its 0/1 validation.

use axum::{Json, extract::Query};
use serde::Deserialize;

use crate::middleware::auth::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct GroupsV21Query {
    pub with_repos: Option<i64>,
    #[allow(dead_code)]
    pub avatar_size: Option<i64>,
}

pub async fn list_groups_v21(
    _auth: AuthUser,
    Query(query): Query<GroupsV21Query>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let with_repos = query.with_repos.unwrap_or(0);
    if with_repos != 0 && with_repos != 1 {
        return Err(AppError::BadRequest("with_repos invalid".into()));
    }
    Ok(Json(Vec::new()))
}

pub async fn list_groups(_auth: AuthUser) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    Ok(Json(Vec::new()))
}
