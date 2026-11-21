use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Payment {
    pub id: String,
    pub order_id: String,
    pub reference: String,
    #[serde(default)]
    pub reference_hash: Option<String>,
    pub claimed_amount_centavos: i64,
    pub received_amount_centavos: Option<i64>,
    pub status: String,
    pub verified_by: Option<String>,
    pub created_at: i64,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub reference: String,
    pub amount_centavos: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub approved: bool,
    pub received_amount_centavos: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RefundReference {
    pub reference: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvidenceResult {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    #[default]
    Gcash,
    CashOnDelivery,
}
impl PaymentMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gcash => "gcash",
            Self::CashOnDelivery => "cash_on_delivery",
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CashCollection {
    pub amount_centavos: i64,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct VendorSettlement {
    pub amount_centavos: i64,
    pub reference: String,
    pub recorded_at: i64,
}
