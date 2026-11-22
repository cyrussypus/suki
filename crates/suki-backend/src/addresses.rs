use crate::{auth::User, *};
use axum::extract::State;
pub use suki_shared::addresses::{Address, AddressInput};
pub async fn list(State(s): State<App>, u: User) -> Result<Json<Vec<Address>>> {
    u.require(&["customer"])?;
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT id,address_encrypted FROM customer_addresses WHERE customer_id=? LIMIT 20",
    )
    .bind(&u.id)
    .fetch_all(&s.db)
    .await?;
    let mut addresses = Vec::with_capacity(rows.len());
    for (id, bytes) in rows {
        let data: protected::SavedDestination =
            s.field_cipher.decode("saved.address", &id, &bytes)?;
        addresses.push(Address {
            id,
            label: data.label,
            address: data.destination.address,
            lat: data.destination.lat,
            lng: data.destination.lng,
        });
    }
    addresses.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(Json(addresses))
}
pub async fn create(
    State(s): State<App>,
    u: User,
    Json(v): Json<AddressInput>,
) -> Result<Json<Created<String>>> {
    u.require(&["customer"])?;
    coordinates(v.lat, v.lng)?;
    let label = protected::address_input(&v.label, 1, 80)?;
    let address = protected::address_input(&v.address, 10, 500)?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=? FOR UPDATE")
        .bind(&u.id)
        .fetch_one(&mut *tx)
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM customer_addresses WHERE customer_id=?")
            .bind(&u.id)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 20 {
        return Err(ApiError::bad("Maximum of 20 saved addresses"));
    }
    let aid = id();
    let value = s.field_cipher.encode(
        "saved.address",
        &aid,
        &protected::SavedDestination {
            label,
            destination: protected::Destination {
                address,
                lat: v.lat,
                lng: v.lng,
            },
        },
    )?;
    sqlx::query("INSERT INTO customer_addresses (id,customer_id,address_encrypted) VALUES(?,?,?)")
        .bind(&aid)
        .bind(&u.id)
        .bind(value)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(Created { id: aid }))
}
