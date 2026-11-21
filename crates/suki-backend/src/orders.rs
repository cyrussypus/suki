use crate::auth::User;
use crate::*;
use axum::extract::{Path, State};
pub use suki_shared::orders::{
    AdjustmentProposal, Assignment, Checkout, CheckoutItem, Decision, Item, Order, Purchase,
    Status, Substitution,
};
use suki_shared::{
    delivery::{grams_rounded_up, line_weight_milligrams},
    orders::{CheckoutResult, OrderDetail, PurchaseResult, Reconciliation},
    payments::{PaymentMethod, VendorSettlement},
    snapshots::{SnapshotItem, SnapshotPayload, delivery_breakdown},
};
pub async fn access(s: &App, u: &User, oid: &str) -> Result<Order> {
    let o: Order = sqlx::query_as("SELECT * FROM orders WHERE id=?")
        .bind(oid)
        .fetch_optional(&s.db)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Order not found".into()))?;
    authorize(u, &o)?;
    Ok(o)
}
pub fn authorize(u: &User, o: &Order) -> Result<()> {
    if u.role == "admin"
        || (u.role == "customer" && o.customer_id == u.id)
        || (u.role == "courier" && o.courier_id.as_deref() == Some(&u.id))
    {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}
pub async fn locked(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    u: &User,
    oid: &str,
) -> Result<Order> {
    let o: Order = sqlx::query_as("SELECT * FROM orders WHERE id=? FOR UPDATE")
        .bind(oid)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Order not found".into()))?;
    authorize(u, &o)?;
    integrity::check_pending(tx, &o.id).await?;
    Ok(o)
}
pub async fn history(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    u: &User,
    oid: &str,
    status: &str,
) -> Result<()> {
    sqlx::query("UPDATE orders SET status=?,updated_at=? WHERE id=?")
        .bind(status)
        .bind(now())
        .bind(oid)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO order_status_history(order_id,status,actor_id,created_at) VALUES (?,?,?,?)",
    )
    .bind(oid)
    .bind(status)
    .bind(&u.id)
    .bind(now())
    .execute(&mut **tx)
    .await?;
    audit(tx, &u.id, "order.status", oid, json!({"status":status})).await?;
    Ok(())
}
pub async fn list(State(s): State<App>, u: User) -> Result<Json<Vec<Order>>> {
    u.require(&["customer", "courier", "admin"])?;
    Ok(Json(sqlx::query_as("SELECT * FROM orders WHERE (?='admin') OR (customer_id=? AND ?='customer') OR (courier_id=? AND ?='courier') ORDER BY created_at DESC LIMIT 100").bind(&u.role).bind(&u.id).bind(&u.role).bind(&u.id).bind(&u.role).fetch_all(&s.db).await?))
}
pub async fn detail(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
) -> Result<Json<OrderDetail>> {
    let o = access(&s, &u, &oid).await?;
    let items: Vec<Item> = sqlx::query_as("SELECT * FROM order_items WHERE order_id=? ORDER BY id")
        .bind(&oid)
        .fetch_all(&s.db)
        .await?;
    let payments: Vec<crate::payments::Payment> =
        sqlx::query_as("SELECT * FROM payments WHERE order_id=? ORDER BY created_at")
            .bind(&oid)
            .fetch_all(&s.db)
            .await?;
    let refunds: Vec<(String, i64, String)> =
        sqlx::query_as("SELECT id,amount_centavos,status FROM refunds WHERE order_id=?")
            .bind(&oid)
            .fetch_all(&s.db)
            .await?;
    let adjustments: Vec<(String, i64, String, String)> = sqlx::query_as(
        "SELECT id,proposed_goods_centavos,reason,status FROM adjustments WHERE order_id=?",
    )
    .bind(&oid)
    .fetch_all(&s.db)
    .await?;
    let substitutions:Vec<(String,String,String,i64,i64,String)>=sqlx::query_as("SELECT id,item_id,name,quantity_milli,unit_price_centavos,status FROM substitutions WHERE order_id=?").bind(&oid).fetch_all(&s.db).await?;
    let history: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT status,actor_id,created_at FROM order_status_history WHERE order_id=? ORDER BY id",
    )
    .bind(&oid)
    .fetch_all(&s.db)
    .await?;
    let settlement: Option<VendorSettlement> = sqlx::query_as(
        "SELECT amount_centavos,reference,recorded_at FROM vendor_settlements WHERE order_id=?",
    )
    .bind(&oid)
    .fetch_optional(&s.db)
    .await?;
    let mut tx = s.db.begin().await?;
    let snapshot = snapshots::receipt(&mut tx, &s.integrity_key, &o.id, &o.customer_id)
        .await?
        .into_result()?;
    tx.commit().await?;
    Ok(Json(OrderDetail {
        order: o,
        snapshot,
        items,
        payments,
        refunds,
        adjustments,
        substitutions,
        history,
        settlement,
    }))
}
#[derive(sqlx::FromRow)]
struct Quote {
    id: String,
    customer_id: String,
    address: String,
    lat: f64,
    lng: f64,
    distance_meters: i64,
    fee_centavos: i64,
    base_fee_centavos: i64,
    per_km_centavos: i64,
    expires_at: i64,
    consumed: bool,
    weight_grams: Option<i64>,
}
pub async fn create(
    State(s): State<App>,
    u: User,
    Json(mut v): Json<Checkout>,
) -> Result<Json<CheckoutResult>> {
    u.require(&["customer"])?;
    text(&v.idempotency_key, 8, 100)?;
    if v.items.is_empty() || v.items.len() > 100 {
        return Err(ApiError::bad("Choose 1–100 products"));
    }
    v.items.sort_by_key(|i| i.product_id);
    if v.items
        .windows(2)
        .any(|w| w[0].product_id == w[1].product_id)
    {
        return Err(ApiError::bad("Duplicate product lines"));
    }
    let digest = hash(&serde_json::to_string(&v).unwrap());
    let mut tx = s.db.begin().await?;
    // Serialize a customer's order submissions before looking up the idempotency key.
    sqlx::query("SELECT id FROM users WHERE id=? FOR UPDATE")
        .bind(&u.id)
        .fetch_one(&mut *tx)
        .await?;
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id,request_hash FROM orders WHERE customer_id=? AND idempotency_key=?",
    )
    .bind(&u.id)
    .bind(&v.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((oid, h)) = existing {
        if h != digest {
            return Err(ApiError::conflict(
                "Idempotency key was used for different order details",
            ));
        }
        let receipt = snapshots::receipt(&mut tx, &s.integrity_key, &oid, &u.id).await?;
        // The order already committed. Report its identity even if its receipt fails verification.
        let (snapshot, snapshot_invalid) = match receipt {
            snapshots::ReceiptState::Invalid => (None, true),
            other => (other.into_result()?, false),
        };
        tx.commit().await?;
        return Ok(Json(CheckoutResult {
            id: oid,
            replayed: true,
            snapshot_invalid,
            snapshot,
        }));
    }
    let q: Quote = sqlx::query_as("SELECT * FROM delivery_quotes WHERE id=? FOR UPDATE")
        .bind(&v.quote_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::bad("Delivery quote not found"))?;
    if q.customer_id != u.id || q.expires_at < now() || q.consumed {
        return Err(ApiError::bad(
            "Delivery quote expired or unavailable; request a new quote",
        ));
    }
    let settings = crate::admin::load(&s.db).await?;
    crate::delivery::coverage(&settings, q.lat, q.lng)?;
    if q.distance_meters > settings.max_distance_meters {
        return Err(ApiError::bad("Delivery exceeds current service coverage"));
    }
    let cod = v.payment_method == PaymentMethod::CashOnDelivery;
    if cod && q.weight_grams.is_none() {
        return Err(ApiError::bad(
            "Request a weight-based quote for cash on delivery",
        ));
    }
    if q.weight_grams.is_none() && settings.gcash_qr_url.is_empty() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "The operator has not configured GCash payments".into(),
        ));
    }
    let mut lines = Vec::new();
    let mut subtotal = 0i64;
    for line in &v.items {
        if line.quantity_milli <= 0 {
            return Err(ApiError::bad("Quantity must be positive"));
        }
        let p: crate::catalog::Product =
            sqlx::query_as("SELECT p.*,v.shop_name AS vendor FROM products p JOIN vendor_profiles v ON v.user_id=p.vendor_id JOIN users u ON u.id=p.vendor_id WHERE u.status='active' AND p.id=? FOR UPDATE")
                .bind(line.product_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(ApiError::bad("A product is no longer available"))?;
        if !p.available || p.price_centavos != line.expected_price_centavos {
            return Err(ApiError::conflict(
                "A product's price or availability changed; refresh your basket",
            ));
        }
        let value = cost(p.price_centavos, line.quantity_milli)?;
        subtotal += value;
        let line_weight = line_weight_milligrams(p.weight_grams, line.quantity_milli)
            .ok_or(ApiError::bad("Invalid item weight"))?;
        lines.push(SnapshotItem {
            item_id: id(),
            product_id: p.id,
            vendor_id: p.vendor_id,
            vendor_name: p.vendor,
            name: p.name,
            unit: p.unit,
            available_at_checkout: p.available,
            quantity_milli: line.quantity_milli,
            unit_weight_grams: p.weight_grams,
            line_weight_milligrams: line_weight,
            unit_price_centavos: p.price_centavos,
            line_total_centavos: value,
        });
    }
    if !suki_shared::pricing::permits_goods_subtotal(subtotal, settings.minimum_centavos) {
        return Err(ApiError::bad(
            "Goods subtotal must meet the minimum and supported spending limit",
        ));
    }
    let mut vendor_id = None;
    let mut gcash_number = None;
    if let Some(quoted_grams) = q.weight_grams {
        let vendor = &lines[0].vendor_id;
        if lines.iter().any(|line| &line.vendor_id != vendor) {
            return Err(ApiError::bad(
                "Each order must contain goods from one vendor stall",
            ));
        }
        let milligrams = lines.iter().map(|line| line.line_weight_milligrams).sum();
        if grams_rounded_up(milligrams) != quoted_grams {
            return Err(ApiError::conflict(
                "Basket weight changed; request a new delivery quote",
            ));
        }
        vendor_id = Some(vendor.clone());
        if !cod {
            gcash_number = sqlx::query_scalar::<_, Option<String>>(
                "SELECT gcash_number FROM vendor_profiles WHERE user_id=?",
            )
            .bind(vendor)
            .fetch_one(&mut *tx)
            .await?;
            if gcash_number.is_none() {
                return Err(ApiError::conflict(
                    "This vendor has not configured GCash; choose cash on delivery",
                ));
            }
        }
    }
    let status = if cod { "confirmed" } else { "awaiting_payment" };
    let payment_status = if cod {
        "cod_pending"
    } else {
        "awaiting_payment"
    };
    let oid = id();
    let created_at = now();
    let delivery = delivery_breakdown(
        q.id.clone(),
        q.distance_meters,
        q.weight_grams,
        q.base_fee_centavos,
        q.per_km_centavos,
    )
    .filter(|breakdown| breakdown.total_centavos == q.fee_centavos)
    .ok_or(ApiError::conflict(
        "Delivery quote could not be verified; request a new quote",
    ))?;
    let total = subtotal + delivery.total_centavos;
    sqlx::query("INSERT INTO orders(id,customer_id,status,payment_status,payment_method,vendor_id,gcash_number,weight_grams,address,lat,lng,distance_meters,delivery_fee_centavos,goods_subtotal_centavos,estimated_total_centavos,authorized_goods_centavos,idempotency_key,request_hash,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&oid).bind(&u.id).bind(status).bind(payment_status).bind(v.payment_method.as_str())
        .bind(&vendor_id).bind(gcash_number).bind(q.weight_grams).bind(&q.address).bind(q.lat).bind(q.lng)
        .bind(q.distance_meters).bind(q.fee_centavos).bind(subtotal).bind(total)
        .bind(subtotal).bind(&v.idempotency_key).bind(digest).bind(created_at).bind(created_at).execute(&mut *tx).await?;
    for line in &lines {
        sqlx::query("INSERT INTO order_items(id,order_id,product_id,vendor_id,name,unit,unit_price_centavos,quantity_milli,estimated_cost_centavos) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(&line.item_id).bind(&oid).bind(line.product_id).bind(&line.vendor_id).bind(&line.name).bind(&line.unit)
            .bind(line.unit_price_centavos).bind(line.quantity_milli).bind(line.line_total_centavos).execute(&mut *tx).await?;
    }
    let snapshot = snapshots::store(
        &mut tx,
        &s.integrity_key,
        SnapshotPayload {
            order_id: oid.clone(),
            customer_id: u.id.clone(),
            vendor_id,
            address: q.address,
            payment_method: v.payment_method,
            items: lines,
            delivery,
            goods_subtotal_centavos: subtotal,
            total_centavos: total,
            created_at,
        },
    )
    .await?;
    sqlx::query("UPDATE delivery_quotes SET consumed=TRUE WHERE id=?")
        .bind(q.id)
        .execute(&mut *tx)
        .await?;
    history(&mut tx, &u, &oid, status).await?;
    integrity::commit(tx, &s.integrity_key, &oid, &u.id).await?;
    Ok(Json(CheckoutResult {
        id: oid,
        replayed: false,
        snapshot_invalid: false,
        snapshot: Some(snapshot),
    }))
}
pub async fn assign(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<Assignment>,
) -> Result<Json<Acknowledged>> {
    u.require(&["admin"])?;
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    if ![
        "paid",
        "confirmed",
        "shopping",
        "ready_for_delivery",
        "price_adjustment",
    ]
    .contains(&o.status.as_str())
        && !(o.vendor_id.is_some()
            && matches!(
                o.status.as_str(),
                "awaiting_payment" | "payment_verification"
            ))
    {
        return Err(ApiError::conflict("This order cannot be assigned"));
    }
    let valid: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE id=? AND role IN ('courier','admin') AND status='active'",
    )
    .bind(&v.courier_id)
    .fetch_one(&mut *tx)
    .await?;
    if valid != 1 {
        return Err(ApiError::bad("Courier is unavailable"));
    }
    sqlx::query("UPDATE orders SET courier_id=? WHERE id=?")
        .bind(&v.courier_id)
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "order.assign",
        &oid,
        json!({"courier_id":v.courier_id}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}
pub fn allowed_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("paid" | "confirmed", "shopping")
            | ("ready_for_delivery", "out_for_delivery")
            | ("out_for_delivery", "delivered")
            | ("delivered", "completed")
    )
}
pub async fn transition(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<Status>,
) -> Result<Json<Acknowledged>> {
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    if v.status == "canceled" {
        if ["completed", "delivered", "canceled"].contains(&o.status.as_str())
            || !(u.role == "admin"
                || u.role == "customer"
                    && [
                        "awaiting_payment",
                        "payment_verification",
                        "paid",
                        "confirmed",
                    ]
                    .contains(&o.status.as_str()))
        {
            return Err(ApiError::forbidden());
        }
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM payments WHERE order_id=? AND status='under_verification'",
        )
        .bind(&oid)
        .fetch_one(&mut *tx)
        .await?;
        if pending > 0 {
            return Err(ApiError::conflict(
                "A payment is under verification; verify or reject it before canceling",
            ));
        }
        let received = crate::payments::received(&mut tx, &oid).await?;
        if received > 0 {
            refund_obligation(&mut tx, &oid, received).await?;
        }
    } else {
        u.require(&["courier", "admin"])?;
        if !allowed_transition(&o.status, &v.status) {
            return Err(ApiError::conflict("Invalid order status transition"));
        }
        let cod_in_progress = o.is_cod()
            && o.payment_status == "cod_pending"
            && matches!(v.status.as_str(), "shopping" | "out_for_delivery");
        if !cod_in_progress && !["paid", "partially_refunded"].contains(&o.payment_status.as_str())
        {
            return Err(ApiError::conflict(
                "Payment must be verified before shopping or delivery",
            ));
        }
        if o.vendor_id.is_some() && v.status == "completed" {
            let settled: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM vendor_settlements WHERE order_id=?")
                    .bind(&oid)
                    .fetch_one(&mut *tx)
                    .await?;
            if settled != 1 {
                return Err(ApiError::conflict(
                    "Record vendor settlement before completing this order",
                ));
            }
        }
        if v.status == "out_for_delivery" && o.courier_id.is_none() {
            return Err(ApiError::conflict("Assign a courier before delivery"));
        }
    }
    history(&mut tx, &u, &oid, &v.status).await?;
    if ["delivered", "canceled"].contains(&v.status.as_str()) {
        sqlx::query("DELETE FROM delivery_locations WHERE order_id=?")
            .bind(&oid)
            .execute(&mut *tx)
            .await?;
        if v.status == "delivered" {
            sqlx::query("UPDATE orders SET delivered_at=? WHERE id=?")
                .bind(now())
                .bind(&oid)
                .execute(&mut *tx)
                .await?;
        }
    }
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    let _ = s
        .events
        .send((oid, OrderEvent::Status { status: v.status }));
    Ok(Json(Acknowledged { ok: true }))
}
fn shopping(u: &User, o: &Order) -> Result<()> {
    u.require(&["courier", "admin"])?;
    if o.status != "shopping"
        || !(o.payment_status == "paid" || o.is_cod() && o.payment_status == "cod_pending")
    {
        return Err(ApiError::conflict("Order must be paid and shopping"));
    }
    Ok(())
}
pub async fn purchase(
    State(s): State<App>,
    u: User,
    Path((oid, iid)): Path<(String, String)>,
    Json(v): Json<Purchase>,
) -> Result<Json<PurchaseResult>> {
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    shopping(&u, &o)?;
    if v.unavailable && (v.actual_quantity_milli != 0 || v.actual_unit_price_centavos != 0) {
        return Err(ApiError::bad(
            "Unavailable items must have zero actual quantity and price",
        ));
    }
    if !v.unavailable && v.actual_quantity_milli <= 0 {
        return Err(ApiError::bad("Purchased quantity must be positive"));
    }
    let value = cost(v.actual_unit_price_centavos, v.actual_quantity_milli)?;
    let item: Option<Item> = sqlx::query_as("SELECT * FROM order_items WHERE id=? AND order_id=?")
        .bind(&iid)
        .bind(&oid)
        .fetch_optional(&mut *tx)
        .await?;
    if item.is_none() {
        return Err(ApiError::bad("Item not found"));
    }
    let proposed:Option<(i64,i64,String)>=sqlx::query_as("SELECT quantity_milli,unit_price_centavos,status FROM substitutions WHERE item_id=? ORDER BY id DESC LIMIT 1").bind(&iid).fetch_optional(&mut *tx).await?;
    if let Some((qty, price, status)) = proposed
        && (status == "pending"
            || status == "rejected" && !v.unavailable
            || status == "approved"
                && (v.actual_quantity_milli > qty || v.actual_unit_price_centavos > price))
    {
        return Err(ApiError::conflict(
            "Substitution requires customer approval for the purchased quantity and price",
        ));
    }
    let recorded:i64=sqlx::query_scalar("SELECT CAST(COALESCE(SUM(actual_cost_centavos),0) AS SIGNED) FROM order_items WHERE order_id=? AND id<>?").bind(&oid).bind(&iid).fetch_one(&mut *tx).await?;
    if recorded + value > o.authorized_goods_centavos {
        return Err(ApiError::conflict(
            "Customer spending approval is required before recording this purchase",
        ));
    }
    sqlx::query("UPDATE order_items SET actual_quantity_milli=?,actual_unit_price_centavos=?,actual_cost_centavos=?,unavailable=? WHERE id=? AND order_id=?").bind(v.actual_quantity_milli).bind(v.actual_unit_price_centavos).bind(value).bind(v.unavailable).bind(&iid).bind(&oid).execute(&mut *tx).await?;
    audit(&mut tx,&u.id,"purchase.record",&iid,json!({"quantity_milli":v.actual_quantity_milli,"unit_price_centavos":v.actual_unit_price_centavos,"cost_centavos":value})).await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(PurchaseResult {
        cost_centavos: value,
    }))
}
pub async fn refund_obligation(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    oid: &str,
    total_due: i64,
) -> Result<()> {
    let existing: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(amount_centavos),0) AS SIGNED) FROM refunds WHERE order_id=?",
    )
    .bind(oid)
    .fetch_one(&mut **tx)
    .await?;
    if total_due > existing {
        sqlx::query("INSERT INTO refunds(id,order_id,amount_centavos,created_at) VALUES (?,?,?,?)")
            .bind(id())
            .bind(oid)
            .bind(total_due - existing)
            .bind(now())
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}
pub async fn reconcile(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
) -> Result<Json<Reconciliation>> {
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    shopping(&u, &o)?;
    let items: Vec<Item> = sqlx::query_as("SELECT * FROM order_items WHERE order_id=?")
        .bind(&oid)
        .fetch_all(&mut *tx)
        .await?;
    if items.iter().any(|i| i.actual_cost_centavos.is_none()) {
        return Err(ApiError::conflict(
            "Record every purchased or unavailable item first",
        ));
    }
    let total: i64 = items
        .iter()
        .map(|i| i.actual_cost_centavos.unwrap_or(0))
        .sum();
    if total > 100_000_000 {
        return Err(ApiError::bad("Purchase exceeds spending limit"));
    }
    let paid = crate::payments::received(&mut tx, &oid).await?;
    sqlx::query("UPDATE orders SET final_goods_centavos=? WHERE id=?")
        .bind(total)
        .bind(&oid)
        .execute(&mut *tx)
        .await?;
    let status = if total > o.authorized_goods_centavos {
        sqlx::query("INSERT INTO adjustments(id,order_id,proposed_goods_centavos,reason) VALUES (?,?,?,'Actual quantity or purchase price changed')").bind(id()).bind(&oid).bind(total).execute(&mut *tx).await?;
        "price_adjustment"
    } else if o.is_cod() {
        "ready_for_delivery"
    } else if paid < total + o.delivery_fee_centavos {
        "price_adjustment"
    } else {
        refund_obligation(
            &mut tx,
            &oid,
            (paid - total - o.delivery_fee_centavos).max(0),
        )
        .await?;
        "ready_for_delivery"
    };
    history(&mut tx, &u, &oid, status).await?;
    audit(
        &mut tx,
        &u.id,
        "order.reconcile",
        &oid,
        json!({"actual_goods_centavos":total}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Reconciliation {
        status: status.into(),
        final_goods_centavos: total,
        difference_centavos: total - o.goods_subtotal_centavos,
    }))
}
pub async fn decide_adjustment(
    State(s): State<App>,
    u: User,
    Path((oid, aid)): Path<(String, String)>,
    Json(v): Json<Decision>,
) -> Result<Json<Acknowledged>> {
    u.require(&["customer"])?;
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    if o.status != "price_adjustment" {
        return Err(ApiError::conflict(
            "No price adjustment is awaiting approval",
        ));
    }
    let total:Option<i64>=sqlx::query_scalar("SELECT proposed_goods_centavos FROM adjustments WHERE id=? AND order_id=? AND status='pending'").bind(&aid).bind(&oid).fetch_optional(&mut *tx).await?;
    let total = total.ok_or(ApiError::conflict(
        "Adjustment already decided or not found",
    ))?;
    sqlx::query("UPDATE adjustments SET status=?,customer_decided_at=? WHERE id=?")
        .bind(if v.approved { "approved" } else { "rejected" })
        .bind(now())
        .bind(aid)
        .execute(&mut *tx)
        .await?;
    if v.approved {
        sqlx::query("UPDATE orders SET authorized_goods_centavos=? WHERE id=?")
            .bind(total)
            .bind(&oid)
            .execute(&mut *tx)
            .await?;
        if o.is_cod()
            || crate::payments::received(&mut tx, &oid).await? >= total + o.delivery_fee_centavos
        {
            history(&mut tx, &u, &oid, "shopping").await?;
        }
    }
    audit(
        &mut tx,
        &u.id,
        "adjustment.decide",
        &oid,
        json!({"approved":v.approved}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}
pub async fn propose_substitution(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<Substitution>,
) -> Result<Json<Created<String>>> {
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    shopping(&u, &o)?;
    text(&v.name, 2, 120)?;
    text(&v.unit, 1, 30)?;
    if v.quantity_milli <= 0 {
        return Err(ApiError::bad("Quantity must be positive"));
    }
    cost(v.unit_price_centavos, v.quantity_milli)?;
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM substitutions WHERE item_id=?")
        .bind(&v.item_id)
        .fetch_one(&mut *tx)
        .await?;
    if existing > 0 {
        return Err(ApiError::conflict(
            "An item already has a substitution proposal",
        ));
    }
    let item: Option<Item> = sqlx::query_as("SELECT * FROM order_items WHERE id=? AND order_id=?")
        .bind(&v.item_id)
        .bind(&oid)
        .fetch_optional(&mut *tx)
        .await?;
    if item.is_none() || item.unwrap().actual_cost_centavos.is_some() {
        return Err(ApiError::conflict("Item not found or already purchased"));
    }
    let sid = id();
    sqlx::query("INSERT INTO substitutions(id,order_id,item_id,name,unit,quantity_milli,unit_price_centavos) VALUES(?,?,?,?,?,?,?)").bind(&sid).bind(&oid).bind(&v.item_id).bind(&v.name).bind(&v.unit).bind(v.quantity_milli).bind(v.unit_price_centavos).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &u.id,
        "substitution.propose",
        &sid,
        json!({"name":v.name,"order_id":oid}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Created { id: sid }))
}
pub async fn decide_substitution(
    State(s): State<App>,
    u: User,
    Path((oid, sid)): Path<(String, String)>,
    Json(v): Json<Decision>,
) -> Result<Json<Acknowledged>> {
    u.require(&["customer"])?;
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    if o.status != "shopping" {
        return Err(ApiError::conflict("Order is not shopping"));
    }
    let changed=sqlx::query("UPDATE substitutions SET status=?,decided_at=? WHERE id=? AND order_id=? AND status='pending'").bind(if v.approved{"approved"}else{"rejected"}).bind(now()).bind(&sid).bind(&oid).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::conflict(
            "Substitution is already decided or not found",
        ));
    }
    audit(
        &mut tx,
        &u.id,
        "substitution.decide",
        &sid,
        json!({"approved":v.approved}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}
pub async fn propose_adjustment(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<AdjustmentProposal>,
) -> Result<Json<Created<String>>> {
    let mut tx = s.db.begin().await?;
    let o = locked(&mut tx, &u, &oid).await?;
    shopping(&u, &o)?;
    let reason = text(&v.reason, 5, 1000)?;
    if v.proposed_goods_centavos <= o.authorized_goods_centavos
        || v.proposed_goods_centavos > 100_000_000
    {
        return Err(ApiError::bad(
            "Propose a higher goods spending limit within the supported maximum",
        ));
    }
    let aid = id();
    sqlx::query(
        "INSERT INTO adjustments(id,order_id,proposed_goods_centavos,reason) VALUES(?,?,?,?)",
    )
    .bind(&aid)
    .bind(&oid)
    .bind(v.proposed_goods_centavos)
    .bind(&reason)
    .execute(&mut *tx)
    .await?;
    history(&mut tx, &u, &oid, "price_adjustment").await?;
    audit(
        &mut tx,
        &u.id,
        "adjustment.propose",
        &aid,
        json!({"goods_centavos":v.proposed_goods_centavos,"reason":reason}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &o.id, &u.id).await?;
    Ok(Json(Created { id: aid }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_graph() {
        assert!(allowed_transition("paid", "shopping"));
        assert!(!allowed_transition("awaiting_payment", "shopping"));
        assert!(!allowed_transition("shopping", "delivered"));
        assert!(!allowed_transition("completed", "shopping"));
    }
}
