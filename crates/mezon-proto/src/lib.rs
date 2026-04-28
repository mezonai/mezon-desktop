// mezon-proto: Protobuf generated types for Mezon API
// These will be generated from .proto files via prost-build in a build.rs
// For now, stub types matching the mezon-js API shape.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelMessage {
    pub id: String,
    pub channel_id: String,
    pub sender_id: String,
    pub username: String,
    pub content: serde_json::Value,
    pub create_time_seconds: i64,
    pub update_time_seconds: Option<i64>,
    pub code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub token: String,
    pub refresh_token: String,
    pub created: bool,
    pub expires_at: u64,
    pub vars: std::collections::HashMap<String, String>,
}
