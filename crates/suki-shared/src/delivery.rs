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
    pub weight_grams: Option<i64>,
    pub per_kg_centavos: Option<i64>,
}

pub const BANKEROHAN_LAT: f64 = 7.0644;
pub const BANKEROHAN_LNG: f64 = 125.6027;
pub const MINIMUM_FEE_CENTAVOS: i64 = 8500;
pub const DISTANCE_RATE_CENTAVOS: i64 = 700;
pub const WEIGHT_RATE_CENTAVOS: i64 = 500;
pub const MAX_CART_WEIGHT_GRAMS: i64 = 100_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeightedDestination {
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub weight_grams: i64,
}

/// Apply the floor after adding both rates, rounding half up once.
pub fn weighted_fee(meters: i64, grams: i64) -> Option<i64> {
    if meters <= 0 || !(1..=MAX_CART_WEIGHT_GRAMS).contains(&grams) {
        return None;
    }
    let scaled = meters
        .checked_mul(DISTANCE_RATE_CENTAVOS)?
        .checked_add(grams.checked_mul(WEIGHT_RATE_CENTAVOS)?)?;
    Some(((scaled.checked_add(500)?) / 1000).max(MINIMUM_FEE_CENTAVOS))
}

/// Each quantity is expressed in thousandths of the product's selling unit.
pub fn line_weight_milligrams(unit_grams: i64, quantity_milli: i64) -> Option<i64> {
    if !(1..=50_000).contains(&unit_grams)
        || !(1..=crate::pricing::MAX_QUANTITY_MILLI).contains(&quantity_milli)
    {
        return None;
    }
    Some(unit_grams * quantity_milli)
}

pub fn grams_rounded_up(milligrams: i64) -> i64 {
    (milligrams + 999) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bankerohan_example_quotes() {
        assert_eq!(weighted_fee(3000, 2000), Some(8500));
        assert_eq!(weighted_fee(15000, 10000), Some(15500));
        assert_eq!(weighted_fee(15001, 10001), Some(15501));
        assert_eq!(weighted_fee(15000, 0), None);
        assert_eq!(weighted_fee(i64::MAX, 10000), None);
    }
}
