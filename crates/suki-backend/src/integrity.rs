use crate::{auth::User, *};
use axum::extract::Path;
use futures_util::TryStreamExt;
use sqlx::{MySql, Transaction};
use std::collections::VecDeque;
use suki_shared::{
    crypto::{
        CHAT_AUTH, IntegrityEntry, IntegrityKey, IntegrityReport, SnapshotVerification,
        TRANSACTION_AUTH, hash_transaction,
    },
    realtime::ChatMessage,
};

type Tx<'a> = Transaction<'a, MySql>;
const GENESIS: &str = "genesis";
const RECENT_LIMIT: usize = 25;

#[derive(sqlx::FromRow)]
struct Entry {
    id: i64,
    event: String,
    payload: String,
    created_at: i64,
    actor_id: Option<String>,
    tag: Option<String>,
}

impl Entry {
    fn hash(&self, order: &str, actor: &str, previous: &str) -> String {
        hash_transaction(
            order,
            self.id,
            &self.event,
            actor,
            self.created_at,
            &self.payload,
            previous,
        )
    }
}

/// Call after locking the order and before writing. Never sign unsigned
/// changes committed by another writer before this transaction began.
pub async fn check_pending(tx: &mut Tx<'_>, order: &str) -> Result<()> {
    let unsealed: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM order_integrity WHERE order_id=? AND tag IS NULL LIMIT 1",
    )
    .bind(order)
    .fetch_optional(&mut **tx)
    .await?;
    if unsealed.is_some() {
        return Err(ApiError::conflict(
            "Order integrity needs administrator review",
        ));
    }
    Ok(())
}

/// The caller owns the order lock. Sealing and business data commit together.
pub async fn commit(mut tx: Tx<'_>, key: &IntegrityKey, order: &str, actor: &str) -> Result<()> {
    let mut previous = sqlx::query_scalar::<_, String>(
        "SELECT tag FROM order_integrity WHERE order_id=? AND tag IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .bind(order)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or_else(|| GENESIS.into());
    let staged: Vec<Entry> = sqlx::query_as(
        "SELECT id,event,payload,created_at,actor_id,tag FROM order_integrity WHERE order_id=? AND tag IS NULL ORDER BY id",
    )
    .bind(order)
    .fetch_all(&mut *tx)
    .await?;
    for entry in staged {
        let tag = key.sign(TRANSACTION_AUTH, &entry.hash(order, actor, &previous));
        sqlx::query("UPDATE order_integrity SET actor_id=?, tag=? WHERE id=?")
            .bind(actor)
            .bind(&tag)
            .bind(entry.id)
            .execute(&mut *tx)
            .await?;
        previous = tag;
    }
    tx.commit().await?;
    Ok(())
}

pub fn valid_message(key: &IntegrityKey, message: &ChatMessage) -> bool {
    message.valid_digest()
        && message
            .hash
            .as_deref()
            .zip(message.signature.as_deref())
            .is_some_and(|(hash, tag)| key.verify(CHAT_AUTH, hash, tag))
}

pub async fn inspect(
    State(s): State<App>,
    u: User,
    Path(order): Path<String>,
) -> Result<Json<IntegrityReport>> {
    u.require(&["admin"])?;
    // Normalize through the stored ID, since MariaDB may match it case insensitively.
    let order: String = sqlx::query_scalar("SELECT id FROM orders WHERE id=?")
        .bind(order)
        .fetch_optional(&s.db)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Order not found".into()))?;
    let mut tx = s.db.begin().await?;
    let mut verified = 0;
    let mut invalid_transaction = None;
    let mut previous = GENESIS.to_string();
    let mut recent = VecDeque::with_capacity(RECENT_LIMIT);
    let mut snapshot_event = false;
    let mut snapshot_anchor = None;
    {
        let mut entries = sqlx::query_as::<_, Entry>(
            "SELECT id,event,payload,created_at,actor_id,tag FROM order_integrity WHERE order_id=? ORDER BY id",
        ).bind(&order).fetch(&mut *tx);
        while let Some(entry) = entries.try_next().await? {
            let is_snapshot = entry.event == "order_snapshots.insert";
            snapshot_event |= is_snapshot;
            if entry
                .actor_id
                .as_deref()
                .zip(entry.tag.as_deref())
                .is_some_and(|(actor, tag)| {
                    s.integrity_key.verify(
                        TRANSACTION_AUTH,
                        &entry.hash(&order, actor, &previous),
                        tag,
                    )
                })
                && invalid_transaction.is_none()
            {
                verified += 1;
                if is_snapshot {
                    snapshot_anchor = serde_json::from_str::<Value>(&entry.payload).ok();
                }
            } else {
                invalid_transaction.get_or_insert(entry.id);
            }
            if let Some(tag) = entry.tag.as_ref() {
                previous.clone_from(tag);
            }
            if recent.len() == RECENT_LIMIT {
                recent.pop_front();
            }
            recent.push_back(IntegrityEntry {
                id: entry.id,
                event: entry.event,
                created_at: entry.created_at,
                tag: entry.tag,
            });
        }
    }
    let (mut messages_verified, mut legacy_messages, mut invalid_message) = (0, 0, None);
    {
        let mut messages = sqlx::query_as::<_, ChatMessage>("SELECT m.id,m.order_id,m.sender_id,u.name AS sender_name,COALESCE(m.sender_role,CAST(u.role AS CHAR)) AS sender_role,m.content,m.created_at,m.nonce,m.hash,m.signature FROM chat_messages m JOIN users u ON u.id=m.sender_id WHERE m.order_id=? ORDER BY m.id")
            .bind(&order)
            .fetch(&mut *tx);
        while let Some(message) = messages.try_next().await? {
            if message.is_legacy() {
                legacy_messages += 1;
            } else if valid_message(&s.integrity_key, &message) {
                messages_verified += 1;
            } else {
                invalid_message.get_or_insert(message.id);
            }
        }
    }
    let snapshot = match snapshots::load(&mut *tx, &order).await? {
        Some(row)
            if snapshot_anchor.is_some_and(|anchor| {
                anchor["snapshot_hash"] == row.snapshot_hash && anchor["signature"] == row.signature
            }) =>
        {
            if row.verified(&s.integrity_key).is_ok() {
                SnapshotVerification::Verified
            } else {
                SnapshotVerification::Invalid
            }
        }
        None if !snapshot_event => SnapshotVerification::Legacy,
        _ => SnapshotVerification::Invalid,
    };
    tx.commit().await?;
    Ok(Json(IntegrityReport {
        order_id: order,
        transactions_verified: verified,
        first_invalid_transaction: invalid_transaction,
        messages_verified,
        legacy_messages,
        snapshot,
        first_invalid_message: invalid_message,
        recent: recent.into_iter().rev().collect(),
    }))
}
