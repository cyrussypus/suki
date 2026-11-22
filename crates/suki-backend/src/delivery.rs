use crate::*;
use crate::{admin::Settings, auth::User};
use axum::extract::State;
pub use suki_shared::delivery::Destination;
use suki_shared::delivery::{
    BANKEROHAN_LAT, BANKEROHAN_LNG, DISTANCE_RATE_CENTAVOS, MAX_CART_WEIGHT_GRAMS,
    MINIMUM_FEE_CENTAVOS, WEIGHT_RATE_CENTAVOS, WeightedDestination, weighted_fee,
};
pub use suki_shared::{delivery::DeliveryQuote, pricing::delivery_fee as fee};
pub fn coverage(v: &Settings, lat: f64, lng: f64) -> Result<()> {
    coordinates(lat, lng)?;
    if lat < v.south || lat > v.north || lng < v.west || lng > v.east {
        return Err(ApiError::bad(
            "Address is outside the configured service area",
        ));
    }
    Ok(())
}
pub async fn quote(
    State(s): State<App>,
    u: User,
    Json(v): Json<Destination>,
) -> Result<Json<DeliveryQuote>> {
    u.require(&["customer"])?;
    if !s.legacy_quotes {
        return Err(ApiError(
            StatusCode::GONE,
            "Request a weight-based quote at /api/quotes/delivery".into(),
        ));
    }
    quote_inner(s, u, v, None).await
}

pub async fn weighted_quote(
    State(s): State<App>,
    u: User,
    Json(v): Json<WeightedDestination>,
) -> Result<Json<DeliveryQuote>> {
    if !(1..=MAX_CART_WEIGHT_GRAMS).contains(&v.weight_grams) {
        return Err(ApiError::bad("Cart must weigh between 1 g and 100 kg"));
    }
    quote_inner(
        s,
        u,
        Destination {
            address: v.address,
            lat: v.lat,
            lng: v.lng,
        },
        Some(v.weight_grams),
    )
    .await
}

async fn quote_inner(
    s: App,
    u: User,
    v: Destination,
    weight: Option<i64>,
) -> Result<Json<DeliveryQuote>> {
    u.require(&["customer"])?;
    let address = protected::address_input(&v.address, 10, 500)?;
    let settings = crate::admin::load(&s.db).await?;
    coverage(&settings, v.lat, v.lng)?;
    if s.maps_key.is_empty() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Google Maps routing is not configured; checkout is unavailable".into(),
        ));
    }
    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_quotes WHERE customer_id=? AND expires_at>?",
    )
    .bind(&u.id)
    .bind(now())
    .fetch_one(&s.db)
    .await?;
    if recent >= 15 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many route requests; retry later".into(),
        ));
    }
    let (origin_lat, origin_lng) = if weight.is_some() {
        (BANKEROHAN_LAT, BANKEROHAN_LNG)
    } else {
        (settings.pickup_lat, settings.pickup_lng)
    };
    let response=s.client.post(&s.maps_url).header("X-Goog-Api-Key",&s.maps_key).header("X-Goog-FieldMask","routes.distanceMeters").json(&json!({"origin":{"location":{"latLng":{"latitude":origin_lat,"longitude":origin_lng}}},"destination":{"location":{"latLng":{"latitude":v.lat,"longitude":v.lng}}},"travelMode":"DRIVE","routingPreference":"TRAFFIC_UNAWARE","computeAlternativeRoutes":false})).send().await.map_err(|_|ApiError(StatusCode::BAD_GATEWAY,"Route service unavailable; please retry".into()))?;
    if !response.status().is_success() {
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "Road distance could not be verified; please retry".into(),
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| ApiError::bad("Invalid routing response"))?;
    let meters = body
        .pointer("/routes/0/distanceMeters")
        .and_then(Value::as_i64)
        .filter(|m| *m > 0 && *m <= settings.max_distance_meters)
        .ok_or(ApiError::bad(
            "No eligible driving route within the service area",
        ))?;
    let (fee, base, per_km) = match weight {
        Some(grams) => (
            weighted_fee(meters, grams).ok_or(ApiError::bad("Invalid delivery fee inputs"))?,
            MINIMUM_FEE_CENTAVOS,
            DISTANCE_RATE_CENTAVOS,
        ),
        None => (
            fee(settings.base_fee_centavos, settings.per_km_centavos, meters),
            settings.base_fee_centavos,
            settings.per_km_centavos,
        ),
    };
    let qid = id();
    let expires_at = now() + 600;
    sqlx::query("INSERT INTO delivery_quotes(id,customer_id,address_encrypted,distance_meters,fee_centavos,base_fee_centavos,per_km_centavos,expires_at,weight_grams) VALUES (?,?,?,?,?,?,?,?,?)").bind(&qid).bind(&u.id).bind(s.field_cipher.encode("quotes.address", &qid, &protected::Destination { address: address.clone(), lat: v.lat, lng: v.lng })?).bind(meters).bind(fee).bind(base).bind(per_km).bind(expires_at).bind(weight).execute(&s.db).await?;
    Ok(Json(DeliveryQuote {
        id: qid,
        address,
        lat: v.lat,
        lng: v.lng,
        distance_meters: meters,
        delivery_fee_centavos: fee,
        base_fee_centavos: base,
        per_km_centavos: per_km,
        weight_grams: weight,
        per_kg_centavos: weight.map(|_| WEIGHT_RATE_CENTAVOS),
        expires_at,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_final_delivery_fee() {
        assert_eq!(fee(6000, 350, 5000), 7750);
        assert_eq!(fee(6000, 350, 1234), 6432);
        assert_eq!(fee(6000, 350, 10), 6004);
    }
}
