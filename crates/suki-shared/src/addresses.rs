use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Address {
    pub id: String,
    pub label: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddressInput {
    pub label: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
}
