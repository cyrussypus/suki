use crate::{auth::User, *};
use axum::extract::Path;
use sqlx::{Executor, MySql};
use suki_shared::{
    crypto::{IntegrityKey, SNAPSHOT_AUTH, hash_snapshot},
    snapshots::{OrderSnapshot, SnapshotPayload},
};

#[derive(sqlx::FromRow)]
pub(crate) struct StoredSnapshot {
    pub order_id: String,
    payload: Vec<u8>,
    pub snapshot_hash: String,
    pub signature: String,
}

pub(crate) fn invalid() -> ApiError {
    ApiError::conflict("Checkout receipt failed its integrity check")
}

impl StoredSnapshot {
    pub fn verified(self, key: &IntegrityKey) -> Result<OrderSnapshot> {
        if hash_snapshot(&self.payload) != self.snapshot_hash
            || !key.verify(SNAPSHOT_AUTH, &self.snapshot_hash, &self.signature)
        {
            return Err(invalid());
        }
        let payload = serde_json::from_slice(&self.payload).map_err(|_| invalid())?;
        let receipt = OrderSnapshot {
            payload,
            snapshot_hash: self.snapshot_hash,
            signature: self.signature,
        };
        if !receipt.valid_digest_for(&self.order_id) {
            return Err(invalid());
        }
        Ok(receipt)
    }
}

pub(crate) async fn load<'e>(
    db: impl Executor<'e, Database = MySql>,
    order: &str,
) -> Result<Option<StoredSnapshot>> {
    Ok(sqlx::query_as(
        "SELECT order_id,payload,snapshot_hash,signature FROM order_snapshots WHERE order_id=?",
    )
    .bind(order)
    .fetch_optional(db)
    .await?)
}

pub(crate) enum ReceiptState {
    Absent,
    Verified(Box<OrderSnapshot>),
    Invalid,
}

impl ReceiptState {
    pub fn into_result(self) -> Result<Option<OrderSnapshot>> {
        match self {
            Self::Absent => Ok(None),
            Self::Verified(receipt) => Ok(Some(*receipt)),
            Self::Invalid => Err(invalid()),
        }
    }
}

pub(crate) async fn receipt(
    tx: &mut sqlx::Transaction<'_, MySql>,
    key: &IntegrityKey,
    order: &str,
    customer: &str,
) -> Result<ReceiptState> {
    if let Some(row) = load(&mut **tx, order).await? {
        match row.verified(key) {
            Ok(snapshot) if snapshot.payload.customer_id == customer => {
                Ok(ReceiptState::Verified(Box::new(snapshot)))
            }
            _ => Ok(ReceiptState::Invalid),
        }
    } else {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM order_integrity WHERE order_id=? AND event='order_snapshots.insert')",
        ).bind(order).fetch_one(&mut **tx).await?;
        Ok(if found {
            ReceiptState::Invalid
        } else {
            ReceiptState::Absent
        })
    }
}

pub(crate) async fn store(
    tx: &mut sqlx::Transaction<'_, MySql>,
    key: &IntegrityKey,
    payload: SnapshotPayload,
) -> Result<OrderSnapshot> {
    let json = serde_json::to_vec(&payload).map_err(|_| invalid())?;
    let snapshot_hash = hash_snapshot(&json);
    let signature = key.sign(SNAPSHOT_AUTH, &snapshot_hash);
    sqlx::query(
        "INSERT INTO order_snapshots(order_id,payload,snapshot_hash,signature) VALUES(?,?,?,?)",
    )
    .bind(&payload.order_id)
    .bind(json)
    .bind(&snapshot_hash)
    .bind(&signature)
    .execute(&mut **tx)
    .await?;
    Ok(OrderSnapshot {
        payload,
        snapshot_hash,
        signature,
    })
}

pub async fn get(
    State(s): State<App>,
    u: User,
    Path(order): Path<String>,
) -> Result<Json<OrderSnapshot>> {
    let order = orders::access(&s, &u, &order).await?;
    let mut tx = s.db.begin().await?;
    let snapshot = receipt(&mut tx, &s.integrity_key, &order.id, &order.customer_id)
        .await?
        .into_result()?;
    snapshot.map(Json).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "This order predates signed checkout receipts".into(),
    ))
}
