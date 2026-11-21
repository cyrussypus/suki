use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub address: String,
    pub lat: f64,
    pub lng: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DeliveryQuote {
    pub id: String,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub distance_meters: i64,
    pub delivery_fee_centavos: i64,
    pub base_fee_centavos: i64,
    pub per_km_centavos: i64,
    pub expires_at: i64,
}
