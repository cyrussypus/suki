use crate::*;
use crate::{auth::User, orders};
use axum::{
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
};
use futures_util::{SinkExt, StreamExt};
pub use suki_shared::realtime::{ChatMessage, Cursor, MessageInput, Position};
use suki_shared::{
    auth::SessionToken,
    crypto::{CHAT_AUTH, hash_message},
    realtime::{CourierLocation, LocationSnapshot, PositionResult},
};
async fn chat_access(s: &App, u: &User, oid: &str) -> Result<orders::Order> {
    let o = if u.role == "vendor" {
        sqlx::query_as::<_, orders::Order>("SELECT * FROM orders WHERE id=? AND vendor_id=?")
            .bind(oid)
            .bind(&u.id)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(ApiError::forbidden)?
    } else {
        orders::access(s, u, oid).await?
    };
    let cfg = crate::admin::load(&s.db).await?;
    if ["completed", "canceled"].contains(&o.status.as_str())
        && o.updated_at + cfg.chat_retention_days as i64 * 86400 < now()
    {
        return Err(ApiError::forbidden());
    }
    Ok(o)
}

pub async fn messages(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Vec<ChatMessage>>> {
    chat_access(&s, &u, &oid).await?;
    let messages: Vec<ChatMessage> = sqlx::query_as("SELECT m.id,m.order_id,m.sender_id,u.name AS sender_name,COALESCE(m.sender_role,CAST(u.role AS CHAR)) AS sender_role,m.content,m.created_at,m.nonce,m.hash,m.signature FROM chat_messages m JOIN users u ON u.id=m.sender_id WHERE m.order_id=? AND m.id>? ORDER BY m.id LIMIT 200")
        .bind(oid)
        .bind(cursor.after)
        .fetch_all(&s.db)
        .await?;
    if messages
        .iter()
        .any(|m| !m.is_legacy() && !integrity::valid_message(&s.integrity_key, m))
    {
        return Err(ApiError::conflict("Order chat failed its integrity check"));
    }
    Ok(Json(messages))
}
pub async fn send_message(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<MessageInput>,
) -> Result<Json<OrderEvent>> {
    let oid = chat_access(&s, &u, &oid).await?.id;
    let content = text(&v.content, 1, 2000)?;
    let mut tx = s.db.begin().await?;
    if u.role == "vendor" {
        let vendor: Option<Option<String>> =
            sqlx::query_scalar("SELECT vendor_id FROM orders WHERE id=? FOR UPDATE")
                .bind(&oid)
                .fetch_optional(&mut *tx)
                .await?;
        if vendor.flatten().as_deref() != Some(&u.id) {
            return Err(ApiError::forbidden());
        }
    } else {
        orders::locked(&mut tx, &u, &oid).await?;
    }
    let recent: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE sender_id=? AND created_at>?")
            .bind(&u.id)
            .bind(now() - 60)
            .fetch_one(&mut *tx)
            .await?;
    if recent >= 30 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Message limit reached; retry shortly".into(),
        ));
    }
    let created = now();
    let nonce = id();
    let hash = hash_message(&oid, &u.id, &u.role, &content, created, &nonce);
    let signature = s.integrity_key.sign(CHAT_AUTH, &hash);
    let row = sqlx::query(
        "INSERT INTO chat_messages(order_id,sender_id,content,created_at,sender_role,nonce,hash,signature) VALUES(?,?,?,?,?,?,?,?)",
    )
    .bind(&oid)
    .bind(&u.id)
    .bind(&content)
    .bind(created)
    .bind(&u.role)
    .bind(&nonce)
    .bind(&hash)
    .bind(&signature)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let event = OrderEvent::Message {
        message: Box::new(ChatMessage {
            id: row.last_insert_id().try_into().map_err(|_| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Invalid message ID".into(),
                )
            })?,
            sender_id: u.id.clone(),
            order_id: oid.clone(),
            sender_role: u.role.clone(),
            sender_name: u.name.clone(),
            content,
            created_at: created,
            nonce: Some(nonce),
            hash: Some(hash),
            signature: Some(signature),
        }),
    };
    let _ = s.events.send((oid, event.clone()));
    Ok(Json(event))
}
pub async fn update_location(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<Position>,
) -> Result<Json<PositionResult>> {
    store_location(&s, &u, &oid, v).await.map(Json)
}

async fn store_location(s: &App, u: &User, oid: &str, v: Position) -> Result<PositionResult> {
    coordinates(v.lat, v.lng)?;
    if v.heading
        .is_some_and(|h| !h.is_finite() || !(0.0..360.0).contains(&h))
        || v.speed
            .is_some_and(|speed| !speed.is_finite() || !(0.0..=150.0).contains(&speed))
        || v.observed_at
            .is_some_and(|t| t < now() - 30 || t > now() + 5)
    {
        return Err(ApiError::bad("Invalid or stale location telemetry"));
    }
    u.require(&["courier", "admin"])?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, u, oid).await?;
    if o.status != "out_for_delivery" || o.courier_id.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    let last: Option<i64> =
        sqlx::query_scalar("SELECT reported_at FROM delivery_locations WHERE order_id=?")
            .bind(oid)
            .fetch_optional(&mut *tx)
            .await?;
    if last.is_some_and(|t| now() - t < 3) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Wait at least three seconds between location updates".into(),
        ));
    }
    let timestamp = now();
    sqlx::query("INSERT INTO delivery_locations(order_id,courier_id,lat,lng,reported_at,heading,speed,observed_at) VALUES(?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE courier_id=VALUES(courier_id),lat=VALUES(lat),lng=VALUES(lng),reported_at=VALUES(reported_at),heading=VALUES(heading),speed=VALUES(speed),observed_at=VALUES(observed_at)").bind(oid).bind(&u.id).bind(v.lat).bind(v.lng).bind(timestamp).bind(v.heading).bind(v.speed).bind(v.observed_at).execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = s.events.send((
        oid.to_owned(),
        OrderEvent::Location(CourierLocation {
            lat: v.lat,
            lng: v.lng,
            reported_at: timestamp,
            heading: v.heading,
            speed: v.speed,
            observed_at: v.observed_at,
        }),
    ));
    Ok(PositionResult {
        reported_at: timestamp,
    })
}
pub async fn location(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
) -> Result<Json<LocationSnapshot>> {
    let o = orders::access(&s, &u, &oid).await?;
    if o.status != "out_for_delivery" {
        return Err(ApiError::conflict("Location sharing is not active"));
    }
    type LocationRow = (f64, f64, i64, Option<f64>, Option<f64>, Option<i64>);
    let row: Option<LocationRow> =
        sqlx::query_as("SELECT lat,lng,reported_at,heading,speed,observed_at FROM delivery_locations WHERE order_id=?")
            .bind(oid)
            .fetch_optional(&s.db)
            .await?;
    Ok(Json(
        row.map(
            |(lat, lng, reported_at, heading, speed, observed_at)| LocationSnapshot::Report {
                position: CourierLocation {
                    lat,
                    lng,
                    reported_at,
                    heading,
                    speed,
                    observed_at,
                },
                stale: now() - reported_at > 15,
            },
        )
        .unwrap_or(LocationSnapshot::Missing {
            location: (),
            stale: true,
        }),
    ))
}
pub async fn websocket(
    State(s): State<App>,
    u: User,
    headers: HeaderMap,
    Path(oid): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(&s.origin) {
        return Err(ApiError::forbidden());
    }
    let oid = chat_access(&s, &u, &oid).await?.id;
    let token = crate::auth::session_token(&headers).ok_or_else(ApiError::forbidden)?;
    Ok(ws
        .max_message_size(4096)
        .on_upgrade(move |socket| events(socket, s, u, oid, token)))
}
async fn events(socket: WebSocket, s: App, u: User, oid: String, token: SessionToken) {
    let mut rx = s.events.subscribe();
    let (mut output, mut input) = socket.split();
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            incoming = input.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok((order, value)) if order == oid => {
                        if !session_valid(&s, &u, &oid, &token).await { break; }
                        if matches!(value, OrderEvent::Location(_)) && orders::access(&s, &u, &oid).await.map(|o|o.status!="out_for_delivery").unwrap_or(true) { continue; }
                        if u.role == "vendor" && !matches!(value, OrderEvent::Message { .. } | OrderEvent::Resync) { continue; }
                        if let OrderEvent::Message { message } = &value
                            && !integrity::valid_message(&s.integrity_key, message) { continue; }
                        if output.send(Message::Text(serde_json::to_string(&value).expect("serializable order event").into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !session_valid(&s, &u, &oid, &token).await { break; }
                        let _=output.send(Message::Text(serde_json::to_string(&OrderEvent::Resync).expect("serializable resync event").into())).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
            _ = timer.tick() => {
                if !session_valid(&s, &u, &oid, &token).await { break; }
                if output.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
    let _ = output.close().await;
}
async fn session_valid(s: &App, u: &User, oid: &str, token: &SessionToken) -> bool {
    match crate::auth::authenticate_token(s, token, true).await {
        Ok(current) if current.id == u.id => chat_access(s, &current, oid).await.is_ok(),
        _ => false,
    }
}

#[derive(serde::Deserialize)]
pub struct TrackingQuery {
    order_id: String,
}

async fn tracking_access(s: &App, u: &User, oid: &str) -> Result<orders::Order> {
    let o = orders::access(s, u, oid).await?;
    let permitted = (u.role == "customer" && o.customer_id == u.id)
        || (matches!(u.role.as_str(), "courier" | "admin")
            && o.courier_id.as_deref() == Some(&u.id));
    if !permitted || o.status != "out_for_delivery" || o.courier_id.is_none() {
        return Err(ApiError::forbidden());
    }
    Ok(o)
}

pub async fn tracking_websocket(
    State(s): State<App>,
    u: User,
    headers: HeaderMap,
    Query(query): Query<TrackingQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(&s.origin) {
        return Err(ApiError::forbidden());
    }
    let oid = tracking_access(&s, &u, &query.order_id).await?.id;
    let token = crate::auth::session_token(&headers).ok_or_else(ApiError::forbidden)?;
    Ok(ws
        .max_message_size(4096)
        .on_upgrade(move |socket| tracking(socket, s, u, oid, token)))
}

async fn tracking_valid(s: &App, u: &User, oid: &str, token: &SessionToken) -> bool {
    match crate::auth::authenticate_token(s, token, true).await {
        Ok(current) if current.id == u.id && current.role == u.role => {
            tracking_access(s, &current, oid).await.is_ok()
        }
        _ => false,
    }
}

async fn tracking(socket: WebSocket, s: App, u: User, oid: String, token: SessionToken) {
    let mut rx = s.events.subscribe();
    let (mut output, mut input) = socket.split();
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            incoming = input.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if !tracking_valid(&s, &u, &oid, &token).await || u.role == "customer" { break; }
                    let Ok(position) = serde_json::from_str::<Position>(&text) else { break; };
                    if position.observed_at.is_none() || s.rate_limits.rider(&crate::auth::rider_session_key(&s, &token)).is_err() { break; }
                    if let Err(error) = store_location(&s, &u, &oid, position).await {
                        let body = json!({"error": error.1}).to_string();
                        if output.send(Message::Text(body.into())).await.is_err() || matches!(error.0, StatusCode::FORBIDDEN | StatusCode::CONFLICT | StatusCode::UNAUTHORIZED) { break; }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
            event = rx.recv() => match event {
                Ok((order, value)) if order == oid => {
                    if !tracking_valid(&s, &u, &oid, &token).await { break; }
                    if matches!(value, OrderEvent::Location(_)) && output.send(Message::Text(serde_json::to_string(&value).expect("serializable event").into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                _ => {}
            },
            _ = timer.tick() => {
                if !tracking_valid(&s, &u, &oid, &token).await || output.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
    let _ = output.close().await;
}

pub async fn eta(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
) -> Result<Json<suki_shared::realtime::Eta>> {
    u.require(&["customer"])?;
    let order = tracking_access(&s, &u, &oid).await?;
    if s.maps_key.is_empty() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Route estimates are unavailable".into(),
        ));
    }
    let (lat, lng, reported): (f64, f64, i64) =
        sqlx::query_as("SELECT lat,lng,reported_at FROM delivery_locations WHERE order_id=?")
            .bind(&oid)
            .fetch_optional(&s.db)
            .await?
            .ok_or(ApiError::conflict("Waiting for rider location"))?;
    if now() - reported > 30 {
        return Err(ApiError::conflict("Rider location is stale"));
    }
    s.rate_limits.eta(&u.id)?;
    let destination = order.destination(&s.field_cipher)?;
    let response = s.client.post(&s.maps_url).header("X-Goog-Api-Key", &s.maps_key)
        .header("X-Goog-FieldMask", "routes.distanceMeters,routes.duration")
        .json(&json!({"origin":{"location":{"latLng":{"latitude":lat,"longitude":lng}}},"destination":{"location":{"latLng":{"latitude":destination.lat,"longitude":destination.lng}}},"travelMode":"DRIVE","routingPreference":"TRAFFIC_UNAWARE","computeAlternativeRoutes":false}))
        .send().await.map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "Route service unavailable".into()))?;
    if !response.status().is_success() {
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "Route service unavailable".into(),
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "Invalid route response".into()))?;
    let meters = body
        .pointer("/routes/0/distanceMeters")
        .and_then(|v| v.as_i64())
        .filter(|n| *n >= 0)
        .ok_or(ApiError(
            StatusCode::BAD_GATEWAY,
            "Invalid route distance".into(),
        ))?;
    let seconds = body
        .pointer("/routes/0/duration")
        .and_then(|v| v.as_str())
        .and_then(|v| v.strip_suffix('s'))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0 && *n < 604800.0)
        .ok_or(ApiError(
            StatusCode::BAD_GATEWAY,
            "Invalid route duration".into(),
        ))?;
    tracking_access(&s, &u, &oid).await?;
    Ok(Json(suki_shared::realtime::Eta {
        distance_meters: meters,
        duration_seconds: seconds.ceil() as i64,
        calculated_at: now(),
    }))
}
