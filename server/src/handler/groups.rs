//! Group routes — retained only as client-compatibility stubs.
//!
//! nanofile has no groups. Nothing creates, joins or lists a real one, the
//! `groups` / `group_members` tables were dropped, and no library can be shared
//! to a group. These two handlers exist because an official client fails on a
//! load path without them:
//!
//! * `GET /api/v2.1/groups/` — seadroid subscribes to it *before* the library
//!   list inside `getReposSingleFromServer` (`Objs.java:93-146`, declared at
//!   `RepoService.java:22-23`). A 404 rejects the whole chain, which clears the
//!   cached library list and shows "Error when loading libraries"; the SAF
//!   documents provider fails the same way. It must answer 200 with a
//!   top-level JSON array.
//! * `GET /api2/groups/?with_msg=false` — the desktop client's group-share
//!   dialog (`seafile-client/src/api/requests.cpp:1290`) rejects with "Failed
//!   to get your groups and contacts information" on a non-2xx. nanofile
//!   advertises `seafile-pro` (`handler::server_info`), which is what makes
//!   that dialog reachable.
//!
//! Both answer an empty list. `with_repos` keeps its validation so a client
//! sending something other than 0/1 gets a 400 instead of a silently different
//! shape.

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

/// `GET /api/v2.1/groups/` — always `[]`, in the array shape seadroid parses.
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

/// `GET /api2/groups/` — always `[]`, in the array shape the desktop client parses.
pub async fn list_groups(_auth: AuthUser) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    Ok(Json(Vec::new()))
}
