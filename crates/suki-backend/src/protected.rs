use crate::{ApiError, Result};
use axum::http::StatusCode;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::MySqlPool;

pub struct FieldCipher(LessSafeKey);

#[derive(Deserialize, Serialize)]
pub(crate) struct Destination {
    pub address: String,
    pub lat: f64,
    pub lng: f64,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct SavedDestination {
    pub label: String,
    #[serde(flatten)]
    pub destination: Destination,
}

fn unavailable() -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Protected data is unavailable".into(),
    )
}

impl FieldCipher {
    pub fn from_hex(key: &str) -> std::result::Result<Self, &'static str> {
        let err = "DATA_ENCRYPTION_KEY must be 64 hex characters";
        let bytes = key.as_bytes();
        if bytes.len() != 64 {
            return Err(err);
        }
        let mut decoded = [0; 32];
        for (out, pair) in decoded.iter_mut().zip(bytes.as_chunks::<2>().0) {
            let hi = char::from(pair[0]).to_digit(16).ok_or(err)?;
            let lo = char::from(pair[1]).to_digit(16).ok_or(err)?;
            *out = (hi * 16 + lo) as u8;
        }
        let key = UnboundKey::new(&AES_256_GCM, &decoded).map_err(|_| err)?;
        Ok(Self(LessSafeKey::new(key)))
    }

    pub(crate) fn seal(&self, field: &str, id: &str, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| unavailable())?;
        let aad = format!("suki:v1:{field}:{id}");
        let mut body = plain.to_vec();
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut body,
            )
            .map_err(|_| unavailable())?;
        let mut value = Vec::with_capacity(13 + body.len());
        value.push(1);
        value.extend(nonce);
        value.extend(body);
        Ok(value)
    }

    pub(crate) fn open(&self, field: &str, id: &str, value: &[u8]) -> Result<Vec<u8>> {
        if value.len() < 29 || value[0] != 1 {
            return Err(unavailable());
        }
        let aad = format!("suki:v1:{field}:{id}");
        let nonce = Nonce::try_assume_unique_for_key(&value[1..13]).map_err(|_| unavailable())?;
        let mut body = value[13..].to_vec();
        let len = self
            .0
            .open_in_place(nonce, Aad::from(aad.as_bytes()), &mut body)
            .map_err(|_| unavailable())?
            .len();
        body.truncate(len);
        Ok(body)
    }

    pub(crate) fn encode(&self, field: &str, id: &str, value: &impl Serialize) -> Result<Vec<u8>> {
        self.seal(
            field,
            id,
            &serde_json::to_vec(value).map_err(|_| unavailable())?,
        )
    }

    pub(crate) fn decode<T: DeserializeOwned>(
        &self,
        field: &str,
        id: &str,
        value: &[u8],
    ) -> Result<T> {
        serde_json::from_slice(&self.open(field, id, value)?).map_err(|_| unavailable())
    }
}

pub(crate) fn address_input(value: &str, min: usize, max: usize) -> Result<String> {
    let value = crate::text(value, min, max)?;
    if value != suki_shared::addresses::display_address_text(&value) {
        return Err(ApiError::bad("Invalid address text"));
    }
    Ok(value)
}

// Runs after SQLx schema changes and before the service listens. The singleton
// row serializes concurrent startups, and the marker and backfill commit together.
pub async fn backfill(db: &MySqlPool, cipher: &FieldCipher) -> Result<()> {
    use sqlx::Row;
    let mut tx = db.begin().await?;
    let marker = cipher.seal("key-check", "1", b"suki protected data")?;
    sqlx::query("INSERT INTO data_encryption_check(id,ciphertext) VALUES (1,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(&marker).execute(&mut *tx).await?;
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM data_encryption_check WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    if cipher.open("key-check", "1", &stored)? != b"suki protected data" {
        return Err(unavailable());
    }
    if stored == marker {
        loop {
            let rows = sqlx::query("SELECT id,address,lat,lng,estimated_total_centavos FROM orders WHERE delivery_address_encrypted IS NULL LIMIT 500 FOR UPDATE").fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let id: String = row.try_get("id")?;
                let destination = Destination {
                    address: row.try_get("address")?,
                    lat: row.try_get("lat")?,
                    lng: row.try_get("lng")?,
                };
                let total: i64 = row.try_get("estimated_total_centavos")?;
                sqlx::query("UPDATE orders SET delivery_address_encrypted=?,total_amount_centavos_encrypted=?,address=NULL,lat=NULL,lng=NULL,estimated_total_centavos=NULL WHERE id=?")
                    .bind(cipher.encode("orders.address", &id, &destination)?)
                    .bind(cipher.encode("orders.total", &id, &total)?)
                    .bind(id).execute(&mut *tx).await?;
            }
        }
        loop {
            let rows = sqlx::query_as::<_, (String,String,f64,f64)>("SELECT id,address,lat,lng FROM delivery_quotes WHERE address_encrypted IS NULL LIMIT 500 FOR UPDATE").fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for (id, address, lat, lng) in rows {
                let value =
                    cipher.encode("quotes.address", &id, &Destination { address, lat, lng })?;
                sqlx::query("UPDATE delivery_quotes SET address_encrypted=?,address=NULL,lat=NULL,lng=NULL WHERE id=?").bind(value).bind(id).execute(&mut *tx).await?;
            }
        }
        loop {
            let rows = sqlx::query_as::<_, (String,String,String,f64,f64)>("SELECT id,label,address,lat,lng FROM customer_addresses WHERE address_encrypted IS NULL LIMIT 500 FOR UPDATE").fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for (id, label, address, lat, lng) in rows {
                let value = cipher.encode(
                    "saved.address",
                    &id,
                    &SavedDestination {
                        label,
                        destination: Destination { address, lat, lng },
                    },
                )?;
                sqlx::query("UPDATE customer_addresses SET address_encrypted=?,address=NULL,lat=NULL,lng=NULL,label=NULL WHERE id=?").bind(value).bind(id).execute(&mut *tx).await?;
            }
        }
        loop {
            let rows = sqlx::query_as::<_, (String,Vec<u8>)>("SELECT order_id,payload FROM order_snapshots WHERE payload_encrypted IS NULL LIMIT 500 FOR UPDATE").fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for (id, payload) in rows {
                let value = cipher.seal("receipt", &id, &payload)?;
                sqlx::query(
                    "UPDATE order_snapshots SET payload_encrypted=?,payload=NULL WHERE order_id=?",
                )
                .bind(value)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        loop {
            let rows = sqlx::query_as::<_, (i64,String,String)>("SELECT id,order_id,payload FROM order_integrity WHERE payload_encrypted IS NULL AND event IN ('orders.insert','orders.update') AND JSON_CONTAINS_PATH(payload,'one','$.estimated_total_centavos') LIMIT 500 FOR UPDATE").fetch_all(&mut *tx).await?;
            if rows.is_empty() {
                break;
            }
            for (id, order, payload) in rows {
                let value =
                    cipher.seal("legacy-audit", &format!("{order}:{id}"), payload.as_bytes())?;
                sqlx::query(
                    "UPDATE order_integrity SET payload_encrypted=?,payload='{}' WHERE id=?",
                )
                .bind(value)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticates_ciphertext_and_its_context() {
        let key = FieldCipher::from_hex(&"42".repeat(32)).unwrap();
        let other = FieldCipher::from_hex(&"11".repeat(32)).unwrap();
        let bytes = key.seal("address", "one", b"private address").unwrap();
        assert_ne!(
            bytes,
            key.seal("address", "one", b"private address").unwrap()
        );
        assert_eq!(
            key.open("address", "one", &bytes).unwrap(),
            b"private address"
        );
        assert!(key.open("address", "two", &bytes).is_err());
        assert!(key.open("receipt", "one", &bytes).is_err());
        assert!(other.open("address", "one", &bytes).is_err());
        let mut changed = bytes;
        changed[13] ^= 1;
        assert!(key.open("address", "one", &changed).is_err());
        assert!(FieldCipher::from_hex(&"é".repeat(32)).is_err());
    }
}
