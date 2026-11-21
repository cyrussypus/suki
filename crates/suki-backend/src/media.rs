use crate::{auth::User, *};
use axum::{
    extract::{Multipart, Path, State},
    http::header,
};
use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use suki_shared::catalog::{CreateProductDto, MAX_PHOTO_BYTES, UpdatePriceDto};

const PREFIX: &str = "/images/vendor/";

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
    let bytes = tokio::fs::read(s.media_dir.join(name))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ApiError(StatusCode::NOT_FOUND, "Photo not found".into())
            } else {
                storage_error(error)
            }
        })?;
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
    static UPLOADS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
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
        return Err(ApiError::bad("Select a JPEG or PNG photo"));
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
        Some(
            tokio::task::spawn_blocking(move || {
                let _upload = upload;
                let invalid = || ApiError::bad("Select a valid JPEG or PNG up to 4096 × 4096");
                let mut reader = ImageReader::new(Cursor::new(bytes))
                    .with_guessed_format()
                    .map_err(|_| invalid())?;
                if !matches!(reader.format(), Some(ImageFormat::Jpeg | ImageFormat::Png)) {
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
                Ok::<_, ApiError>(encoded.into_inner())
            })
            .await
            .map_err(|_| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Image processing failed".into(),
                )
            })??,
        )
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

    let mut written = None;
    let url = if let Some(bytes) = sanitized {
        let filename = format!("{}.jpg", id());
        let path = s.media_dir.join(&filename);
        tokio::fs::create_dir_all(&s.media_dir)
            .await
            .map_err(storage_error)?;
        let temporary = path.with_extension("upload");
        let saved = async {
            tokio::fs::write(&temporary, bytes).await?;
            tokio::fs::rename(&temporary, &path).await
        }
        .await;
        if let Err(error) = saved {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(storage_error(error));
        }
        written = Some(path);
        format!("{PREFIX}{filename}")
    } else {
        existing.clone()
    };

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
        audit(&mut tx, &u.id, if pid.is_some() {"product.edit"} else {"product.create"}, &product_id.to_string(), json!({"price_centavos":input.price_centavos})).await?;
        Ok(product_id)
    }.await;
    match result {
        Ok(product_id) => {
            tx.commit().await?;
            Ok(product_id)
        }
        Err(error) => {
            if let Some(written) = written {
                let _ = tokio::fs::remove_file(written).await;
            }
            Err(error)
        }
    }
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
