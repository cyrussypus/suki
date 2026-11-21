use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Order {
    pub id: String,
    pub customer_id: String,
    pub courier_id: Option<String>,
    pub status: String,
    pub payment_status: String,
    pub payment_method: String,
    pub vendor_id: Option<String>,
    pub gcash_number: Option<String>,
    pub weight_grams: Option<i64>,
    pub address: String,
    pub lat: f64,
    pub lng: f64,
    pub distance_meters: i64,
    pub delivery_fee_centavos: i64,
    pub goods_subtotal_centavos: i64,
    pub estimated_total_centavos: i64,
    pub final_goods_centavos: Option<i64>,
    pub authorized_goods_centavos: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub delivered_at: Option<i64>,
}

impl Order {
    pub fn is_cod(&self) -> bool {
        self.payment_method == crate::payments::PaymentMethod::CashOnDelivery.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Item {
    pub id: String,
    pub product_id: i64,
    pub vendor_id: String,
    pub name: String,
    pub unit: String,
    pub unit_price_centavos: i64,
    pub quantity_milli: i64,
    pub estimated_cost_centavos: i64,
    pub actual_quantity_milli: Option<i64>,
    pub actual_unit_price_centavos: Option<i64>,
    pub actual_cost_centavos: Option<i64>,
    pub unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutItem {
    pub product_id: i64,
    pub quantity_milli: i64,
    pub expected_price_centavos: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub quote_id: String,
    pub idempotency_key: String,
    pub items: Vec<CheckoutItem>,
    #[serde(default, skip_serializing_if = "is_gcash")]
    pub payment_method: crate::payments::PaymentMethod,
}

fn is_gcash(method: &crate::payments::PaymentMethod) -> bool {
    *method == crate::payments::PaymentMethod::Gcash
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub courier_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Purchase {
    pub actual_quantity_milli: i64,
    pub actual_unit_price_centavos: i64,
    pub unavailable: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Substitution {
    pub item_id: String,
    pub name: String,
    pub unit: String,
    pub quantity_milli: i64,
    pub unit_price_centavos: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdjustmentProposal {
    pub proposed_goods_centavos: i64,
    pub reason: String,
}

pub type Refund = (String, i64, String);
pub type Adjustment = (String, i64, String, String);
pub type SubstitutionRecord = (String, String, String, i64, i64, String);
pub type StatusEntry = (String, String, i64);

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct OrderDetail {
    pub order: Order,
    #[serde(default)]
    pub snapshot: Option<crate::snapshots::OrderSnapshot>,
    pub items: Vec<Item>,
    pub payments: Vec<crate::payments::Payment>,
    pub refunds: Vec<Refund>,
    pub adjustments: Vec<Adjustment>,
    pub substitutions: Vec<SubstitutionRecord>,
    pub history: Vec<StatusEntry>,
    pub settlement: Option<crate::payments::VendorSettlement>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CheckoutResult {
    pub id: String,
    pub replayed: bool,
    #[serde(default)]
    pub snapshot_invalid: bool,
    #[serde(default)]
    pub snapshot: Option<crate::snapshots::OrderSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PurchaseResult {
    pub cost_centavos: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Reconciliation {
    pub status: String,
    pub final_goods_centavos: i64,
    pub difference_centavos: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    AwaitingPayment,
    PaymentVerification,
    Paid,
    Confirmed,
    Shopping,
    PriceAdjustment,
    ReadyForDelivery,
    OutForDelivery,
    Delivered,
    Completed,
    Canceled,
}
