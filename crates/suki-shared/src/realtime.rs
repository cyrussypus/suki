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
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl ChatMessage {
    pub fn computed_hash(&self) -> Option<String> {
        Some(crate::crypto::hash_message(
            &self.order_id,
            &self.sender_id,
            &self.sender_role,
            &self.content,
            self.created_at,
            self.nonce.as_deref()?,
        ))
    }

    pub fn is_legacy(&self) -> bool {
        self.nonce.is_none() && self.hash.is_none() && self.signature.is_none()
    }

    pub fn valid_digest(&self) -> bool {
        match (self.computed_hash(), &self.hash, &self.signature) {
            (Some(computed), Some(stored), Some(_)) => {
                crate::crypto::verify_hash(&computed, stored)
            }
            _ => false,
        }
    }
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

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub lat: f64,
    pub lng: f64,
    #[serde(default)]
    pub heading: Option<f64>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub observed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CourierLocation {
    pub lat: f64,
    pub lng: f64,
    pub reported_at: i64,
    #[serde(default)]
    pub heading: Option<f64>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub observed_at: Option<i64>,
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
    Message { message: Box<ChatMessage> },
    Location(CourierLocation),
    Status { status: String },
    Resync,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PositionResult {
    pub reported_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Eta {
    pub distance_meters: i64,
    pub duration_seconds: i64,
    pub calculated_at: i64,
}
