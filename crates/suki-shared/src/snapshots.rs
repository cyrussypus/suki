use crate::{
    crypto::{hash_snapshot, verify_hash},
    delivery::{MINIMUM_FEE_CENTAVOS, weighted_fee},
    payments::PaymentMethod,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotItem {
    pub item_id: String,
    pub product_id: i64,
    pub vendor_id: String,
    pub vendor_name: String,
    pub name: String,
    pub unit: String,
    pub available_at_checkout: bool,
    pub quantity_milli: i64,
    pub unit_weight_grams: i64,
    pub line_weight_milligrams: i64,
    pub unit_price_centavos: i64,
    pub line_total_centavos: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDelivery {
    pub quote_id: String,
    pub distance_meters: i64,
    pub weight_grams: Option<i64>,
    pub distance_rate_centavos_per_km: i64,
    pub weight_rate_centavos_per_kg: Option<i64>,
    pub floor_centavos: Option<i64>,
    pub legacy_base_centavos: i64,
    // Exact terms prior to the single half-up rounding of the combined charge.
    pub distance_millicentavos: i64,
    pub weight_millicentavos: i64,
    pub floor_top_up_centavos: i64,
    pub total_centavos: i64,
}

impl SnapshotDelivery {
    pub fn distance_charge_centavos(&self) -> i64 {
        (self.distance_millicentavos + 500) / 1000
    }

    pub fn weight_charge_centavos(&self) -> i64 {
        (self.distance_millicentavos + self.weight_millicentavos + 500) / 1000
            - self.distance_charge_centavos()
    }
}

pub fn delivery_breakdown(
    quote_id: String,
    meters: i64,
    weight: Option<i64>,
    base: i64,
    per_km: i64,
) -> Option<SnapshotDelivery> {
    if weight.is_some()
        && (base != MINIMUM_FEE_CENTAVOS || per_km != crate::delivery::DISTANCE_RATE_CENTAVOS)
    {
        return None;
    }
    let per_kg = weight.map(|_| crate::delivery::WEIGHT_RATE_CENTAVOS);
    let distance_millicentavos = meters.checked_mul(per_km)?;
    let weight_millicentavos = weight
        .unwrap_or_default()
        .checked_mul(per_kg.unwrap_or_default())?;
    let variable = distance_millicentavos
        .checked_add(weight_millicentavos)?
        .checked_add(500)?
        / 1000;
    let (floor, legacy_base, total) = match weight {
        Some(grams) => (Some(MINIMUM_FEE_CENTAVOS), 0, weighted_fee(meters, grams)?),
        None => (None, base, base.checked_add(variable)?),
    };
    Some(SnapshotDelivery {
        quote_id,
        distance_meters: meters,
        weight_grams: weight,
        distance_rate_centavos_per_km: per_km,
        weight_rate_centavos_per_kg: per_kg,
        floor_centavos: floor,
        legacy_base_centavos: legacy_base,
        distance_millicentavos,
        weight_millicentavos,
        floor_top_up_centavos: total.checked_sub(legacy_base)?.checked_sub(variable)?,
        total_centavos: total,
    })
}

// Persisted v1 wire format. Field order and encoding must remain stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPayload {
    pub order_id: String,
    pub customer_id: String,
    pub vendor_id: Option<String>,
    pub address: String,
    pub payment_method: PaymentMethod,
    pub items: Vec<SnapshotItem>,
    pub delivery: SnapshotDelivery,
    pub goods_subtotal_centavos: i64,
    pub total_centavos: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSnapshot {
    pub payload: SnapshotPayload,
    pub snapshot_hash: String,
    pub signature: String,
}

impl OrderSnapshot {
    pub fn valid_digest_for(&self, order_id: &str) -> bool {
        self.payload.order_id == order_id
            && serde_json::to_vec(&self.payload)
                .is_ok_and(|payload| verify_hash(&hash_snapshot(&payload), &self.snapshot_hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_binds_every_serialized_field() {
        let payload = SnapshotPayload {
            order_id: "order".into(),
            customer_id: "customer".into(),
            vendor_id: Some("vendor".into()),
            address: "Davao City".into(),
            payment_method: PaymentMethod::CashOnDelivery,
            items: vec![SnapshotItem {
                item_id: "item".into(),
                product_id: 1,
                vendor_id: "vendor".into(),
                vendor_name: "Stall".into(),
                name: "Rice".into(),
                unit: "bag".into(),
                available_at_checkout: true,
                quantity_milli: 1000,
                unit_weight_grams: 2000,
                line_weight_milligrams: 2_000_000,
                unit_price_centavos: 45_000,
                line_total_centavos: 45_000,
            }],
            delivery: delivery_breakdown("quote".into(), 3000, Some(2000), 8500, 700).unwrap(),
            goods_subtotal_centavos: 45_000,
            total_centavos: 53_500,
            created_at: 20,
        };
        let hash = hash_snapshot(&serde_json::to_vec(&payload).unwrap());
        let snapshot = OrderSnapshot {
            payload: payload.clone(),
            snapshot_hash: hash.clone(),
            signature: String::new(),
        };
        assert!(snapshot.valid_digest_for("order"));
        assert!(!snapshot.valid_digest_for("other"));
        assert_eq!(payload.delivery.floor_top_up_centavos, 5400);
        assert_eq!(
            payload.delivery.distance_charge_centavos()
                + payload.delivery.weight_charge_centavos()
                + payload.delivery.floor_top_up_centavos,
            8500
        );
        let mut changed = serde_json::to_value(payload).unwrap();
        // Serializing the typed DTO reconstructs the canonical field order.
        fn verify_edits(value: &mut serde_json::Value, check: &impl Fn(&serde_json::Value)) {
            fn paths(
                value: &serde_json::Value,
                path: &mut Vec<String>,
                all: &mut Vec<Vec<String>>,
            ) {
                match value {
                    serde_json::Value::Object(map) => {
                        for (name, child) in map {
                            path.push(name.clone());
                            paths(child, path, all);
                            path.pop();
                        }
                    }
                    serde_json::Value::Array(list) => {
                        for (idx, child) in list.iter().enumerate() {
                            path.push(idx.to_string());
                            paths(child, path, all);
                            path.pop();
                        }
                    }
                    _ => all.push(path.clone()),
                }
            }
            let mut all = Vec::new();
            paths(value, &mut Vec::new(), &mut all);
            for path in all {
                let ptr = format!("/{}", path.join("/"));
                let original = value.pointer(&ptr).unwrap().clone();
                *value.pointer_mut(&ptr).unwrap() = match &original {
                    serde_json::Value::String(s) if s == "cash_on_delivery" => "gcash".into(),
                    serde_json::Value::String(s) => format!("{s}!").into(),
                    serde_json::Value::Number(n) => (n.as_i64().unwrap() + 1).into(),
                    serde_json::Value::Bool(v) => (!v).into(),
                    _ => panic!("test needs a valid alternate value"),
                };
                check(value);
                *value.pointer_mut(&ptr).unwrap() = original;
            }
        }
        verify_edits(&mut changed, &|value| {
            let edited: SnapshotPayload = serde_json::from_value(value.clone()).unwrap();
            assert_ne!(hash_snapshot(&serde_json::to_vec(&edited).unwrap()), hash);
        });
    }
}
