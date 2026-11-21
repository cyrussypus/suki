use crate::*;
use crate::{auth::User, orders};
use axum::extract::{Path, State};
use suki_shared::payments::CashCollection;
pub use suki_shared::payments::{Evidence, EvidenceResult, Payment, RefundReference, Verification};
pub async fn received(tx: &mut sqlx::Transaction<'_, sqlx::MySql>, oid: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT CAST(COALESCE(SUM(received_amount_centavos),0) AS SIGNED) FROM payments WHERE order_id=? AND status='paid'").bind(oid).fetch_one(&mut **tx).await?)
}
pub async fn submit(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<Evidence>,
) -> Result<Json<EvidenceResult>> {
    u.require(&["customer"])?;
    let reference = text(&v.reference, 6, 100)?.to_uppercase();
    if !reference
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ApiError::bad(
            "Reference must contain only letters, digits or hyphens",
        ));
    }
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if o.is_cod() {
        return Err(ApiError::conflict("This order uses cash on delivery"));
    }
    if ![
        "awaiting_payment",
        "payment_verification",
        "price_adjustment",
    ]
    .contains(&o.status.as_str())
    {
        return Err(ApiError::conflict("Order is not awaiting payment"));
    }
    let due =
        o.authorized_goods_centavos + o.delivery_fee_centavos - received(&mut tx, &oid).await?;
    if v.amount_centavos != due || due <= 0 {
        return Err(ApiError::bad(
            "Payment must match the outstanding authorized total",
        ));
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM payments WHERE order_id=? AND status='under_verification'",
    )
    .bind(&oid)
    .fetch_one(&mut *tx)
    .await?;
    if pending > 0 {
        return Err(ApiError::conflict(
            "A payment is already under verification",
        ));
    }
    let pid = id();
    sqlx::query("INSERT INTO payments(id,order_id,reference,claimed_amount_centavos,created_at) VALUES(?,?,?,?,?)").bind(&pid).bind(&oid).bind(reference).bind(v.amount_centavos).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE orders SET payment_status='under_verification' WHERE id=?")
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    if o.status == "awaiting_payment" {
        orders::history(&mut tx, &u, &oid, "payment_verification").await?;
    }
    audit(
        &mut tx,
        &u.id,
        "payment.submit",
        &pid,
        json!({"order_id":oid,"claimed_centavos":v.amount_centavos}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    if o.vendor_id.is_some() {
        let _ = s.events.send((oid, OrderEvent::Resync));
    }
    Ok(Json(EvidenceResult {
        id: pid,
        status: "under_verification".into(),
    }))
}
pub async fn verify(
    State(s): State<App>,
    u: User,
    Path(pid): Path<String>,
    Json(v): Json<Verification>,
) -> Result<Json<StatusResult>> {
    u.require(&["admin", "courier"])?;
    let oid: String = sqlx::query_scalar("SELECT order_id FROM payments WHERE id=?")
        .bind(&pid)
        .fetch_optional(&s.db)
        .await?
        .ok_or(ApiError::bad("Payment not found"))?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if o.is_cod() || u.role == "courier" && o.vendor_id.is_none() {
        return Err(ApiError::forbidden());
    }
    let p: Payment = sqlx::query_as("SELECT * FROM payments WHERE id=? FOR UPDATE")
        .bind(&pid)
        .fetch_one(&mut *tx)
        .await?;
    if p.reference_hash.as_deref() != Some(&suki_shared::crypto::gcash_reference_hash(&p.reference))
    {
        return Err(ApiError::conflict("GCash reference integrity check failed"));
    }
    if p.status != "under_verification" {
        return Err(ApiError::conflict("Payment was already verified"));
    }
    if ![
        "awaiting_payment",
        "payment_verification",
        "price_adjustment",
    ]
    .contains(&o.status.as_str())
    {
        return Err(ApiError::conflict(
            "Order is not eligible for payment verification",
        ));
    }
    let due =
        o.authorized_goods_centavos + o.delivery_fee_centavos - received(&mut tx, &oid).await?;
    if v.approved
        && (v.received_amount_centavos != due
            || v.received_amount_centavos != p.claimed_amount_centavos)
    {
        return Err(ApiError::bad(
            "Received amount does not match the required authorized amount",
        ));
    }
    if !v.approved && v.received_amount_centavos != 0 {
        return Err(ApiError::bad(
            "Reject with zero received amount; resolve any received funds before rejection",
        ));
    }
    let status = if v.approved { "paid" } else { "rejected" };
    sqlx::query("UPDATE payments SET status=?,received_amount_centavos=?,verified_by=?,verified_at=? WHERE id=?").bind(status).bind(v.received_amount_centavos).bind(&u.id).bind(now()).bind(&pid).execute(&mut *tx).await?;
    sqlx::query("UPDATE orders SET payment_status=? WHERE id=?")
        .bind(status)
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    orders::history(
        &mut tx,
        &u,
        &oid,
        if v.approved {
            if o.final_goods_centavos.is_some() || o.status == "price_adjustment" {
                "shopping"
            } else {
                "paid"
            }
        } else if o.status == "price_adjustment" {
            "price_adjustment"
        } else {
            "awaiting_payment"
        },
    )
    .await?;
    audit(
        &mut tx,
        &u.id,
        "payment.verify",
        &pid,
        json!({"approved":v.approved,"received_centavos":v.received_amount_centavos}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    if o.vendor_id.is_some() {
        let _ = s.events.send((oid, OrderEvent::Resync));
    }
    Ok(Json(StatusResult {
        status: status.into(),
    }))
}
pub async fn refund(
    State(s): State<App>,
    u: User,
    Path(rid): Path<String>,
    Json(v): Json<RefundReference>,
) -> Result<Json<Acknowledged>> {
    u.require(&["admin"])?;
    let reference = text(&v.reference, 6, 100)?;
    let oid: String = sqlx::query_scalar("SELECT order_id FROM refunds WHERE id=?")
        .bind(&rid)
        .fetch_optional(&s.db)
        .await?
        .ok_or(ApiError::bad("Refund not found"))?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if ![
        "ready_for_delivery",
        "out_for_delivery",
        "delivered",
        "completed",
        "canceled",
    ]
    .contains(&o.status.as_str())
    {
        return Err(ApiError::conflict(
            "Complete reconciliation or cancellation before refunding",
        ));
    }
    let result=sqlx::query("UPDATE refunds SET status='processed',reference=?,processed_by=? WHERE id=? AND status='pending'").bind(reference).bind(&u.id).bind(&rid).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict("Refund already processed"));
    }
    let received = received(&mut tx, &oid).await?;
    let refunded:i64=sqlx::query_scalar("SELECT CAST(COALESCE(SUM(amount_centavos),0) AS SIGNED) FROM refunds WHERE order_id=? AND status='processed'").bind(&oid).fetch_one(&mut *tx).await?;
    if refunded > received {
        return Err(ApiError::conflict("Refund exceeds verified payments"));
    }
    sqlx::query("UPDATE orders SET payment_status=? WHERE id=?")
        .bind(if refunded == received {
            "refunded"
        } else {
            "partially_refunded"
        })
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "refund.process",
        &rid,
        json!({"order_id":oid}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}

pub async fn pending(State(s): State<App>, u: User) -> Result<Json<Vec<Payment>>> {
    u.require(&["admin"])?;
    Ok(Json(sqlx::query_as("SELECT * FROM payments WHERE status='under_verification' ORDER BY created_at LIMIT 200")
        .fetch_all(&s.db).await?))
}

pub async fn collect_cash(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(input): Json<CashCollection>,
) -> Result<Json<Acknowledged>> {
    u.require(&["courier", "admin"])?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if o.courier_id.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    if !o.is_cod() || o.payment_status != "cod_pending" || o.status != "out_for_delivery" {
        return Err(ApiError::conflict("Cash is not collectible for this order"));
    }
    let due = o
        .final_goods_centavos
        .ok_or(ApiError::conflict("Reconcile goods before collection"))?
        + o.delivery_fee_centavos;
    if input.amount_centavos != due {
        return Err(ApiError::bad("Cash must match the reconciled total"));
    }
    let timestamp = now();
    sqlx::query("INSERT INTO payments(id,order_id,reference,claimed_amount_centavos,received_amount_centavos,status,verified_by,created_at,verified_at) VALUES(?,?,?,?,?,'paid',?,?,?)")
        .bind(id()).bind(&oid).bind(format!("cod:{oid}")).bind(due).bind(due).bind(&u.id).bind(timestamp).bind(timestamp)
        .execute(&mut *tx).await?;
    sqlx::query("UPDATE orders SET payment_status='paid' WHERE id=?")
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "payment.cash_collected",
        &oid,
        json!({"amount_centavos": due}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    let _ = s.events.send((oid, OrderEvent::Resync));
    Ok(Json(Acknowledged { ok: true }))
}

pub async fn settle_vendor(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(input): Json<RefundReference>,
) -> Result<Json<Acknowledged>> {
    u.require(&["courier", "admin"])?;
    let reference = text(&input.reference, 6, 100)?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if o.courier_id.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    if o.vendor_id.is_none()
        || o.status != "delivered"
        || !matches!(o.payment_status.as_str(), "paid" | "partially_refunded")
    {
        return Err(ApiError::conflict(
            "Only delivered and paid marketplace orders can be settled",
        ));
    }
    let amount = o
        .final_goods_centavos
        .ok_or(ApiError::conflict("Reconcile goods before settling"))?;
    sqlx::query("INSERT INTO vendor_settlements(order_id,amount_centavos,reference,recorded_by,recorded_at) VALUES(?,?,?,?,?)")
        .bind(&oid).bind(amount).bind(&reference).bind(&u.id).bind(now()).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &u.id,
        "payment.vendor_settlement",
        &oid,
        json!({"amount_centavos":amount}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    let _ = s.events.send((oid, OrderEvent::Resync));
    Ok(Json(Acknowledged { ok: true }))
}
