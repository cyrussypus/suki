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

/// Render address fields as text and omit unsafe controls, including bidi overrides.
pub fn display_address_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect()
}
