use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct Account {
    pub id: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub role: String,
    pub status: String,
}

/// Authenticated session response. Keep the existing account fields at the top level.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SessionAccount {
    #[serde(flatten)]
    pub account: Account,
    #[serde(default)]
    pub admin_access: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<i64>,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub name: String,
    pub email: String,
    pub phone: String,
    pub password: String,
    pub role: String,
    pub shop_name: Option<String>,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    pub email: String,
    pub password: String,
}

/// Stable wire role names. Account and admin DTOs retain their existing string fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Customer,
    Vendor,
    #[serde(rename = "courier", alias = "rider")]
    Rider,
    Admin,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Vendor => "vendor",
            Self::Rider => "courier",
            Self::Admin => "admin",
        }
    }
}

impl Account {
    pub fn has_role(&self, role: Role) -> bool {
        self.role == role.as_str()
    }
}

pub const SESSION_COOKIE: &str = "suki_session";
pub const SESSION_SECONDS: i64 = 7 * 86_400;
pub const PASSWORD_MIN_BYTES: usize = 12;
pub const PASSWORD_MAX_BYTES: usize = 128;
// These match the existing Argon2 defaults used to issue hashes.
pub const ARGON2_MEMORY_KIB: u32 = 19_456;
pub const ARGON2_ITERATIONS: u32 = 2;
pub const ARGON2_LANES: u32 = 1;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RegistrationResult {
    pub id: String,
    pub status: String,
}

/// Opaque cookie credential. Deliberately avoids Debug and Serialize.
pub struct SessionToken(String);

impl SessionToken {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn secret(&self) -> &str {
        &self.0
    }
}

pub type UserRole = Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    PendingApproval,
    Active,
    Rejected,
    Suspended,
    Deactivated,
}

impl AccountStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingApproval => "pending_approval",
            Self::Active => "active",
            Self::Rejected => "rejected",
            Self::Suspended => "suspended",
            Self::Deactivated => "deactivated",
        }
    }
}

pub const REFRESH_COOKIE: &str = "suki_refresh";
pub const ACCESS_SECONDS: i64 = 900;
pub const REFRESH_SECONDS: i64 = 30 * 86_400;

#[derive(Clone, Deserialize, Serialize)]
pub struct DeviceSession {
    pub id: String,
    pub device_name: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
    pub current: bool,
}
