pub mod addresses;
pub mod admin;
pub mod auth;
pub mod catalog;
pub mod delivery;
pub mod orders;
pub mod payments;
pub mod pricing;
pub mod realtime;
pub mod vendors;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Acknowledged {
    pub ok: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Created<T> {
    pub id: T,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct StatusResult {
    pub status: String,
}
