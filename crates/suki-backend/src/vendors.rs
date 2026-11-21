use crate::{auth::User, *};
use axum::extract::{Path, State};
pub use suki_shared::vendors::{Availability, Category, VendorItem};
pub async fn items(State(s): State<App>, u: User) -> Result<Json<Vec<VendorItem>>> {
    u.require(&["vendor"])?;
    Ok(Json(sqlx::query_as("SELECT i.id AS item_id,i.order_id,i.name,i.quantity_milli,i.unit,o.status AS order_status,(o.vendor_id=i.vendor_id AND o.vendor_id IS NOT NULL) AS chat_enabled,i.available_quantity_milli FROM order_items i JOIN orders o ON o.id=i.order_id WHERE i.vendor_id=? AND (o.status IN ('paid','shopping','price_adjustment') OR (o.vendor_id=i.vendor_id AND o.status IN ('awaiting_payment','payment_verification','confirmed','ready_for_delivery','out_for_delivery'))) ORDER BY o.created_at LIMIT 500").bind(&u.id).fetch_all(&s.db).await?))
}
pub async fn confirm(
    State(s): State<App>,
    u: User,
    Path(iid): Path<String>,
    Json(v): Json<Availability>,
) -> Result<Json<Acknowledged>> {
    u.require(&["vendor"])?;
    let mut tx = s.db.begin().await?;
    let row:Option<(String,i64,String)>=sqlx::query_as("SELECT i.vendor_id,i.quantity_milli,o.status FROM order_items i JOIN orders o ON o.id=i.order_id WHERE i.id=? FOR UPDATE").bind(&iid).fetch_optional(&mut *tx).await?;
    let (vendor, quantity, status) = row.ok_or(ApiError::forbidden())?;
    if vendor != u.id {
        return Err(ApiError::forbidden());
    }
    if !["confirmed", "paid", "shopping", "price_adjustment"].contains(&status.as_str())
        || v.available_quantity_milli < 0
        || v.available_quantity_milli > quantity
    {
        return Err(ApiError::bad("Invalid availability confirmation"));
    }
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
    tx.commit().await?;
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
