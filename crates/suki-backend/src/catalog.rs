use crate::auth::User;
use crate::*;
use axum::extract::{Path, State};
pub use suki_shared::catalog::{Product, ProductInput};
pub async fn list(State(s): State<App>) -> Result<Json<Vec<Product>>> {
    Ok(Json(
        sqlx::query_as("SELECT p.*,v.shop_name AS vendor FROM products p JOIN vendor_profiles v ON v.user_id=p.vendor_id JOIN users u ON u.id=p.vendor_id WHERE u.status='active' ORDER BY p.id")
            .fetch_all(&s.db)
            .await?,
    ))
}
fn validate(product: &ProductInput) -> Result<()> {
    text(&product.name, 2, 120)?;
    text(&product.local_name, 0, 100)?;
    text(&product.description, 0, 4000)?;
    text(&product.category, 2, 80)?;
    text(&product.unit, 1, 30)?;
    if product.price_centavos <= 0 || product.price_centavos > 100_000_000 {
        return Err(ApiError::bad("Invalid price"));
    }
    if !product.image.is_empty()
        && (!product.image.starts_with("/images/")
            || product.image.contains("..")
            || product.image.len() > 200)
    {
        return Err(ApiError::bad("Use a local /images/ path"));
    }
    Ok(())
}
pub async fn create(
    State(s): State<App>,
    u: User,
    Json(p): Json<ProductInput>,
) -> Result<Json<Created<u64>>> {
    u.require(&["vendor"])?;
    validate(&p)?;
    let mut tx = s.db.begin().await?;
    let category: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE name=?")
        .bind(&p.category)
        .fetch_one(&mut *tx)
        .await?;
    if category == 0 {
        return Err(ApiError::bad("Unknown category"));
    }
    let row=sqlx::query("INSERT INTO products(vendor_id,name,local_name,description,category,unit,price_centavos,image,available,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)").bind(&u.id).bind(&p.name).bind(&p.local_name).bind(&p.description).bind(&p.category).bind(&p.unit).bind(p.price_centavos).bind(&p.image).bind(p.available).bind(now()).execute(&mut *tx).await?;
    let pid = row.last_insert_id();
    audit(
        &mut tx,
        &u.id,
        "product.create",
        &pid.to_string(),
        json!({"price_centavos":p.price_centavos}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Created { id: pid }))
}
pub async fn update(
    State(s): State<App>,
    u: User,
    Path(pid): Path<i64>,
    Json(p): Json<ProductInput>,
) -> Result<Json<Acknowledged>> {
    u.require(&["vendor"])?;
    validate(&p)?;
    let mut tx = s.db.begin().await?;
    let owner: Option<String> =
        sqlx::query_scalar("SELECT vendor_id FROM products WHERE id=? FOR UPDATE")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?;
    if owner.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    let category: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE name=?")
        .bind(&p.category)
        .fetch_one(&mut *tx)
        .await?;
    if category == 0 {
        return Err(ApiError::bad("Unknown category"));
    }
    sqlx::query("UPDATE products SET name=?,local_name=?,description=?,category=?,unit=?,price_centavos=?,image=?,available=?,updated_at=? WHERE id=?").bind(&p.name).bind(&p.local_name).bind(&p.description).bind(&p.category).bind(&p.unit).bind(p.price_centavos).bind(&p.image).bind(p.available).bind(now()).bind(pid).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &u.id,
        "product.update",
        &pid.to_string(),
        json!({"price_centavos":p.price_centavos,"available":p.available}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Acknowledged { ok: true }))
}
