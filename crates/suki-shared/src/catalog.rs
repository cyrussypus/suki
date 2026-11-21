use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Product {
    pub id: i64,
    pub vendor_id: String,
    pub vendor: String,
    pub name: String,
    pub local_name: String,
    pub description: String,
    pub category: String,
    pub unit: String,
    pub price_centavos: i64,
    pub image: String,
    pub available: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductInput {
    pub name: String,
    pub local_name: String,
    pub description: String,
    pub category: String,
    pub unit: String,
    pub price_centavos: i64,
    pub image: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProductAvailability {
    pub available: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogFilter {
    pub search: String,
    pub category: Option<String>,
    pub vendor_id: Option<String>,
}

impl CatalogFilter {
    pub fn matches(&self, product: &Product) -> bool {
        let term = self.search.trim().to_lowercase();
        self.category
            .as_ref()
            .is_none_or(|v| *v == product.category)
            && self
                .vendor_id
                .as_ref()
                .is_none_or(|v| *v == product.vendor_id)
            && [
                &product.name,
                &product.local_name,
                &product.vendor,
                &product.description,
            ]
            .into_iter()
            .any(|field| field.to_lowercase().contains(&term))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BasketLine {
    pub product_id: i64,
    pub price_centavos: i64,
    pub quantity_milli: i64,
}

pub type Favorites = std::collections::BTreeSet<i64>;
