use crate::{auth::User, *};
use axum::extract::{Path, State};
pub use suki_shared::vendors::{Availability, Category, VendorItem};
pub async fn items(State(s): State<App>, u: User) -> Result<Json<Vec<VendorItem>>> {
    u.require(&["vendor"])?;
    let mut tx = s.db.begin().await?;
    let mut items: Vec<VendorItem> = sqlx::query_as("SELECT i.id AS item_id,i.order_id,i.name,i.quantity_milli,i.unit,o.status AS order_status,(o.vendor_id=i.vendor_id AND o.vendor_id IS NOT NULL) AS chat_enabled,i.available_quantity_milli,i.unit_price_centavos,i.estimated_cost_centavos AS line_total_centavos FROM order_items i JOIN orders o ON o.id=i.order_id WHERE i.vendor_id=? AND (o.status IN ('paid','shopping','price_adjustment') OR (o.vendor_id=i.vendor_id AND o.status IN ('awaiting_payment','payment_verification','confirmed','ready_for_delivery','out_for_delivery'))) ORDER BY o.created_at LIMIT 500")
        .bind(&u.id).fetch_all(&mut *tx).await?;
    let order_ids: std::collections::BTreeSet<_> =
        items.iter().map(|item| &item.order_id).collect();
    if order_ids.is_empty() {
        return Ok(Json(items));
    }
    let mut query = sqlx::QueryBuilder::<sqlx::MySql>::new(
        "SELECT order_id,payload,snapshot_hash,signature FROM order_snapshots WHERE order_id IN (",
    );
    let mut ids = query.separated(",");
    for id in order_ids {
        ids.push_bind(id);
    }
    ids.push_unseparated(")");
    let receipts = query
        .build_query_as::<snapshots::StoredSnapshot>()
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut verified = std::collections::HashMap::new();
    for stored in receipts {
        let snapshot = stored.verified(&s.integrity_key)?;
        verified.insert(snapshot.payload.order_id, snapshot.payload.items);
    }
    for item in &mut items {
        if let Some(lines) = verified.get(&item.order_id) {
            let line = lines
                .iter()
                .find(|line| line.item_id == item.item_id && line.vendor_id == u.id)
                .ok_or_else(snapshots::invalid)?;
            item.unit_price_centavos = Some(line.unit_price_centavos);
            item.line_total_centavos = Some(line.line_total_centavos);
            item.snapshot_verified = true;
        }
    }
    Ok(Json(items))
}
pub async fn confirm(
    State(s): State<App>,
    u: User,
    Path(iid): Path<String>,
    Json(v): Json<Availability>,
) -> Result<Json<Acknowledged>> {
    u.require(&["vendor"])?;
    let mut tx = s.db.begin().await?;
    let row:Option<(String,i64,String,String)>=sqlx::query_as("SELECT i.vendor_id,i.quantity_milli,o.status,o.id FROM order_items i JOIN orders o ON o.id=i.order_id WHERE i.id=? FOR UPDATE").bind(&iid).fetch_optional(&mut *tx).await?;
    let (vendor, quantity, status, order_id) = row.ok_or(ApiError::forbidden())?;
    if vendor != u.id {
        return Err(ApiError::forbidden());
    }
    if !["confirmed", "paid", "shopping", "price_adjustment"].contains(&status.as_str())
        || v.available_quantity_milli < 0
        || v.available_quantity_milli > quantity
    {
        return Err(ApiError::bad("Invalid availability confirmation"));
    }
    integrity::check_pending(&mut tx, &order_id).await?;
    sqlx::query("UPDATE order_items SET available_quantity_milli=? WHERE id=?")
        .bind(v.available_quantity_milli)
        .bind(&iid)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "vendor.confirm",
        &iid,
        json!({"available_quantity_milli":v.available_quantity_milli}),
    )
    .await?;
    integrity::commit(tx, &s.integrity_key, &order_id, &u.id).await?;
    Ok(Json(Acknowledged { ok: true }))
}
pub async fn categories(State(s): State<App>) -> Result<Json<Vec<String>>> {
    Ok(Json(
        sqlx::query_scalar("SELECT name FROM categories ORDER BY name")
            .fetch_all(&s.db)
            .await?,
    ))
}
pub async fn add_category(
    State(s): State<App>,
    u: User,
    Json(v): Json<Category>,
) -> Result<Json<Acknowledged>> {
    u.require(&["admin"])?;
    let name = text(&v.name, 2, 80)?;
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO categories VALUES(?)")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "category.create",
        &name,
        json!({"name":name}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Acknowledged { ok: true }))
}

pub async fn payment_profile(
    State(s): State<App>,
    u: User,
) -> Result<Json<suki_shared::catalog::VendorPaymentProfile>> {
    u.require(&["vendor"])?;
    let gcash_number =
        sqlx::query_scalar("SELECT gcash_number FROM vendor_profiles WHERE user_id=?")
            .bind(&u.id)
            .fetch_one(&s.db)
            .await?;
    Ok(Json(suki_shared::catalog::VendorPaymentProfile {
        gcash_number,
    }))
}

pub async fn update_payment_profile(
    State(s): State<App>,
    u: User,
    Json(input): Json<suki_shared::catalog::VendorPaymentProfile>,
) -> Result<Json<Acknowledged>> {
    u.require(&["vendor"])?;
    if input.gcash_number.as_ref().is_some_and(|n| {
        n.len() != 11 || !n.starts_with("09") || !n.bytes().all(|c| c.is_ascii_digit())
    }) {
        return Err(ApiError::bad(
            "Use an 11-digit Philippine mobile number starting with 09",
        ));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE vendor_profiles SET gcash_number=? WHERE user_id=?")
        .bind(input.gcash_number)
        .bind(&u.id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, &u.id, "vendor.payment_profile", &u.id, json!({})).await?;
    tx.commit().await?;
    Ok(Json(Acknowledged { ok: true }))
}
