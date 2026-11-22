use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub rating: u8,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Review {
    pub rating: u8,
    pub comment: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OrderReviews {
    pub vendor: Option<Review>,
    pub rider: Option<Review>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Reputation {
    pub score: String,
    pub total_reviews: i64,
    pub reputation_points: i64,
    pub total_deliveries: Option<i64>,
}
