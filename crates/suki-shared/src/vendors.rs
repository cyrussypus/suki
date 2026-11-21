use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Availability {
    pub available_quantity_milli: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct VendorItem {
    pub item_id: String,
    pub order_id: String,
    pub name: String,
    pub quantity_milli: i64,
    pub unit: String,
    pub order_status: String,
    pub available_quantity_milli: Option<i64>,
}
