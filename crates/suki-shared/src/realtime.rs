use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ChatMessage {
    pub id: i64,
    pub sender_id: String,
    pub order_id: String,
    pub sender_role: String,
    pub sender_name: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Cursor {
    #[serde(default)]
    pub after: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageInput {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub lat: f64,
    pub lng: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CourierLocation {
    pub lat: f64,
    pub lng: f64,
    pub reported_at: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LocationSnapshot {
    Report {
        #[serde(flatten)]
        position: CourierLocation,
        stale: bool,
    },
    Missing {
        location: (),
        stale: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrderEvent {
    Message { message: ChatMessage },
    Location(CourierLocation),
    Status { status: String },
    Resync,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PositionResult {
    pub reported_at: i64,
}
