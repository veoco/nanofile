use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct ProtocolVersion {
    pub version: i32,
}

pub async fn protocol_version() -> Json<ProtocolVersion> {
    Json(ProtocolVersion { version: 2 })
}
