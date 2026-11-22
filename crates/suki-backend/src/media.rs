use crate::{auth::User, *};
use axum::{
    extract::{Multipart, Path, State},
    http::header,
};
use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use suki_shared::catalog::{CreateProductDto, MAX_PHOTO_BYTES, UpdatePriceDto};

const PREFIX: &str = "/images/vendor/";
static UPLOADS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

fn storage_error(error: std::io::Error) -> ApiError {
    tracing::error!(%error, "Vendor image storage failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Image storage unavailable".into(),
    )
}

fn valid_image_name(name: &str) -> bool {
    name.strip_suffix(".jpg")
        .is_some_and(|stem| uuid::Uuid::parse_str(stem).is_ok_and(|id| id.to_string() == stem))
}

pub async fn photo(State(s): State<App>, Path(name): Path<String>) -> Result<impl IntoResponse> {
    if !valid_image_name(&name) {
        return Err(ApiError(StatusCode::NOT_FOUND, "Photo not found".into()));
    }
    let bytes = match tokio::fs::read(s.media_dir.join(&name)).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let object_id = name.trim_end_matches(".jpg");
            let key: Option<String> =
                sqlx::query_scalar("SELECT object_key FROM media_objects WHERE id=?")
                    .bind(object_id)
                    .fetch_optional(&s.db)
                    .await?;
            let key = key.ok_or(ApiError(StatusCode::NOT_FOUND, "Photo not found".into()))?;
            s.r2.as_ref()
                .ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Image storage unavailable".into(),
                ))?
                .get(&key)
                .await?
        }
        Err(error) => return Err(storage_error(error)),
    };
    Ok(([(header::CONTENT_TYPE, "image/jpeg")], bytes))
}

pub async fn create(
    State(s): State<App>,
    u: User,
    multipart: Multipart,
) -> Result<Json<Created<u64>>> {
    mutate(s, u, None, multipart)
        .await
        .map(|id| Json(Created { id }))
}

pub async fn edit(
    State(s): State<App>,
    u: User,
    Path(pid): Path<i64>,
    multipart: Multipart,
) -> Result<Json<Acknowledged>> {
    mutate(s, u, Some(pid), multipart).await?;
    Ok(Json(Acknowledged { ok: true }))
}

async fn mutate(s: App, u: User, pid: Option<i64>, mut multipart: Multipart) -> Result<u64> {
    u.require(&["vendor"])?;
    // Bound both buffered upload memory and concurrent decoding.
    let upload = UPLOADS.try_acquire();
    let upload = upload.map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Photo uploads are busy; retry shortly".into(),
        )
    })?;
    let mut product = None;
    let mut contents = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid multipart upload".into()))?
    {
        match field.name() {
            Some("product") if product.is_none() => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError(e.status(), "Invalid product field".into()))?;
                if bytes.len() > 8192 {
                    return Err(ApiError::bad("Product details are too large"));
                }
                product = Some(
                    serde_json::from_slice::<CreateProductDto>(&bytes)
                        .map_err(|_| ApiError::bad("Invalid product details"))?,
                );
            }
            Some("photo") if contents.is_none() => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError(e.status(), "Invalid photo field".into()))?;
                if bytes.is_empty() || bytes.len() > MAX_PHOTO_BYTES {
                    return Err(ApiError::bad("Photo must be no larger than 5 MiB"));
                }
                contents = Some(bytes);
            }
            _ => return Err(ApiError::bad("Unexpected or repeated upload field")),
        }
    }
    if pid.is_some() && contents.is_some() {
        s.rate_limits.upload(&u.id)?;
    }
    let input = product.ok_or(ApiError::bad("Product details are required"))?;
    let name = text(&input.name, 2, 120)?;
    let description = text(&input.description, 0, 4000)?;
    let unit = text(&input.unit, 1, 30)?;
    let category = text(&input.category, 2, 80)?;
    validate_price(input.price_centavos)?;
    if !(1..=50_000).contains(&input.weight_grams) {
        return Err(ApiError::bad("Selling unit weight must be 1–50,000 grams"));
    }
    if pid.is_none() && contents.is_none() {
        return Err(ApiError::bad("Select a JPEG, PNG or WebP photo"));
    }
    if let Some(pid) = pid {
        let owner: Option<String> = sqlx::query_scalar("SELECT vendor_id FROM products WHERE id=?")
            .bind(pid)
            .fetch_optional(&s.db)
            .await?;
        if owner.as_deref() != Some(&u.id) {
            return Err(ApiError::forbidden());
        }
    }

    let sanitized = if let Some(bytes) = contents {
        Some(sanitize(bytes.to_vec(), upload).await?)
    } else {
        None
    };

    let mut tx = s.db.begin().await?;
    let existing = if let Some(pid) = pid {
        let image: Option<String> =
            sqlx::query_scalar("SELECT image FROM products WHERE id=? AND vendor_id=? FOR UPDATE")
                .bind(pid)
                .bind(&u.id)
                .fetch_optional(&mut *tx)
                .await?;
        image.ok_or_else(ApiError::forbidden)?
    } else {
        String::new()
    };
    let found: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE name=?")
        .bind(&category)
        .fetch_one(&mut *tx)
        .await?;
    if found != 1 {
        return Err(ApiError::bad("Choose an existing category"));
    }

    let uploaded = if let Some(bytes) = sanitized {
        let object_id = id();
        let key = format!("images/{object_id}.jpg");
        let r2 = s.r2.as_ref().ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Image storage unavailable".into(),
        ))?;
        let public_url = r2.put(&key, bytes).await?;
        Some((object_id, key, public_url))
    } else {
        None
    };
    let url = uploaded
        .as_ref()
        .map(|(object_id, _, _)| format!("{PREFIX}{object_id}.jpg"))
        .unwrap_or(existing);

    let result: Result<u64> = async {
        let product_id = if let Some(pid) = pid {
            sqlx::query("UPDATE products SET name=?,description=?,category=?,unit=?,price_centavos=?,weight_grams=?,image=?,updated_at=? WHERE id=? AND vendor_id=?")
                .bind(&name).bind(&description).bind(&category).bind(&unit).bind(input.price_centavos)
                .bind(input.weight_grams).bind(&url).bind(now()).bind(pid).bind(&u.id).execute(&mut *tx).await?;
            pid as u64
        } else {
            sqlx::query("INSERT INTO products(vendor_id,name,description,category,unit,price_centavos,weight_grams,image,updated_at) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(&u.id).bind(name).bind(description).bind(category).bind(unit).bind(input.price_centavos)
                .bind(input.weight_grams).bind(&url).bind(now()).execute(&mut *tx).await?.last_insert_id()
        };
        if let Some((object_id, key, public_url)) = &uploaded {
            sqlx::query("INSERT INTO media_objects(id,owner_id,product_id,object_key,public_url,created_at) VALUES (?,?,?,?,?,?)")
                .bind(object_id).bind(&u.id).bind(product_id).bind(key).bind(public_url).bind(now()).execute(&mut *tx).await?;
            sqlx::query("UPDATE products SET image_url=? WHERE id=? AND vendor_id=?")
                .bind(public_url).bind(product_id).bind(&u.id).execute(&mut *tx).await?;
        }
        audit(&mut tx, &u.id, if pid.is_some() {"product.edit"} else {"product.create"}, &product_id.to_string(), json!({"price_centavos":input.price_centavos})).await?;
        Ok(product_id)
    }.await;
    match result {
        Ok(product_id) => {
            tx.commit().await?;
            Ok(product_id)
        }
        Err(error) => Err(error),
    }
}

async fn sanitize(
    bytes: Vec<u8>,
    upload: tokio::sync::SemaphorePermit<'static>,
) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let _upload = upload;
        let invalid = || ApiError::bad("Select a valid JPEG, PNG or WebP up to 4096 × 4096");
        if bytes.is_empty() || bytes.len() > MAX_PHOTO_BYTES {
            return Err(invalid());
        }
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|_| invalid())?;
        if !matches!(
            reader.format(),
            Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
        ) {
            return Err(invalid());
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(96 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().map_err(|_| invalid())?;
        let image = if image.width() > 1600 || image.height() > 1600 {
            image.thumbnail(1600, 1600)
        } else {
            image
        }
        .into_rgb8();
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Jpeg)
            .map_err(|_| invalid())?;
        let encoded = encoded.into_inner();
        if encoded.len() > MAX_PHOTO_BYTES {
            return Err(invalid());
        }
        Ok(encoded)
    })
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Image processing failed".into(),
        )
    })?
}

pub async fn avatar(
    State(s): State<App>,
    u: User,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    u.require(&["customer", "vendor", "courier", "admin"])?;
    let permit = UPLOADS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Photo uploads are busy; retry shortly".into(),
        )
    })?;
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid multipart upload".into()))?
        .ok_or(ApiError::bad("Select a photo"))?;
    if field.name() != Some("photo") {
        return Err(ApiError::bad("Expected photo field"));
    }
    let bytes = field
        .bytes()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid photo field".into()))?;
    if multipart
        .next_field()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid multipart upload".into()))?
        .is_some()
    {
        return Err(ApiError::bad("Unexpected upload field"));
    }
    let bytes = sanitize(bytes.to_vec(), permit).await?;
    let object_id = id();
    let key = format!("images/{object_id}.jpg");
    let r2 = s.r2.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Image storage unavailable".into(),
    ))?;
    let public_url = r2.put(&key, bytes).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=? FOR UPDATE")
        .bind(&u.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO media_objects(id,owner_id,object_key,public_url,created_at) VALUES (?,?,?,?,?)")
        .bind(&object_id).bind(&u.id).bind(&key).bind(&public_url).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE users SET avatar_url=? WHERE id=?")
        .bind(&public_url)
        .bind(&u.id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, &u.id, "avatar.update", &u.id, json!({})).await?;
    tx.commit().await?;
    Ok(Json(json!({"image": format!("{PREFIX}{object_id}.jpg")})))
}

pub async fn upload_product(
    State(s): State<App>,
    u: User,
    Path(pid): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    u.require(&["vendor"])?;
    let owner: Option<String> = sqlx::query_scalar("SELECT vendor_id FROM products WHERE id=?")
        .bind(pid)
        .fetch_optional(&s.db)
        .await?;
    if owner.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    let permit = UPLOADS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Photo uploads are busy; retry shortly".into(),
        )
    })?;
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid multipart upload".into()))?
        .ok_or(ApiError::bad("Select a photo"))?;
    if field.name() != Some("photo") {
        return Err(ApiError::bad("Expected photo field"));
    }
    let bytes = field
        .bytes()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid photo field".into()))?;
    if multipart
        .next_field()
        .await
        .map_err(|e| ApiError(e.status(), "Invalid multipart upload".into()))?
        .is_some()
    {
        return Err(ApiError::bad("Unexpected upload field"));
    }
    let bytes = sanitize(bytes.to_vec(), permit).await?;
    let object_id = id();
    let key = format!("images/{object_id}.jpg");
    let r2 = s.r2.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Image storage unavailable".into(),
    ))?;
    let public_url = r2.put(&key, bytes).await?;
    let image = format!("{PREFIX}{object_id}.jpg");
    let mut tx = s.db.begin().await?;
    let owner: Option<String> =
        sqlx::query_scalar("SELECT vendor_id FROM products WHERE id=? FOR UPDATE")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?;
    if owner.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    sqlx::query("INSERT INTO media_objects(id,owner_id,product_id,object_key,public_url,created_at) VALUES (?,?,?,?,?,?)")
        .bind(&object_id).bind(&u.id).bind(pid).bind(&key).bind(&public_url).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE products SET image=?,image_url=?,updated_at=? WHERE id=? AND vendor_id=?")
        .bind(&image)
        .bind(&public_url)
        .bind(now())
        .bind(pid)
        .bind(&u.id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, &u.id, "product.photo", &pid.to_string(), json!({})).await?;
    tx.commit().await?;
    Ok(Json(json!({"image": image})))
}

pub async fn get_avatar(State(s): State<App>, u: User) -> Result<Json<Value>> {
    let object: Option<String> = sqlx::query_scalar("SELECT m.id FROM media_objects m JOIN users u ON u.avatar_url=m.public_url WHERE u.id=? AND m.owner_id=u.id ORDER BY m.created_at DESC LIMIT 1")
        .bind(&u.id).fetch_optional(&s.db).await?;
    Ok(Json(
        json!({"image": object.map(|id| format!("{PREFIX}{id}.jpg"))}),
    ))
}

fn validate_price(price: i64) -> Result<()> {
    if !(1..=suki_shared::pricing::MAX_GOODS_CENTAVOS).contains(&price) {
        return Err(ApiError::bad("Invalid price in centavos"));
    }
    Ok(())
}

pub async fn change_price(
    State(s): State<App>,
    u: User,
    Path(pid): Path<i64>,
    Json(input): Json<UpdatePriceDto>,
) -> Result<Json<Acknowledged>> {
    u.require(&["vendor"])?;
    validate_price(input.price_centavos)?;
    let mut tx = s.db.begin().await?;
    let owner: Option<String> =
        sqlx::query_scalar("SELECT vendor_id FROM products WHERE id=? FOR UPDATE")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?;
    if owner.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    sqlx::query("UPDATE products SET price_centavos=?,updated_at=? WHERE id=?")
        .bind(input.price_centavos)
        .bind(now())
        .bind(pid)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        &u.id,
        "product.price",
        &pid.to_string(),
        json!({"price_centavos":input.price_centavos}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(Acknowledged { ok: true }))
}
