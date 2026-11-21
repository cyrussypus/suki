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
    auth::{SESSION_COOKIE, SessionToken},
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
    Ok(Json(sqlx::query_as("SELECT m.id,m.order_id,m.sender_id,u.name AS sender_name,COALESCE(m.sender_role,CAST(u.role AS CHAR)) AS sender_role,m.content,m.created_at FROM chat_messages m JOIN users u ON u.id=m.sender_id WHERE order_id=? AND m.id>? ORDER BY m.id LIMIT 200").bind(oid).bind(cursor.after).fetch_all(&s.db).await?))
}
pub async fn send_message(
    State(s): State<App>,
    u: User,
    Path(oid): Path<String>,
    Json(v): Json<MessageInput>,
) -> Result<Json<OrderEvent>> {
    chat_access(&s, &u, &oid).await?;
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
    let row = sqlx::query(
        "INSERT INTO chat_messages(order_id,sender_id,content,created_at,sender_role) VALUES(?,?,?,?,?)",
    )
    .bind(&oid)
    .bind(&u.id)
    .bind(&content)
    .bind(created)
    .bind(&u.role)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let event = OrderEvent::Message {
        message: ChatMessage {
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
        },
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
    coordinates(v.lat, v.lng)?;
    u.require(&["courier", "admin"])?;
    let mut tx = s.db.begin().await?;
    let o = orders::locked(&mut tx, &u, &oid).await?;
    if o.status != "out_for_delivery" || o.courier_id.as_deref() != Some(&u.id) {
        return Err(ApiError::forbidden());
    }
    let last: Option<i64> =
        sqlx::query_scalar("SELECT reported_at FROM delivery_locations WHERE order_id=?")
            .bind(&oid)
            .fetch_optional(&mut *tx)
            .await?;
    if last.is_some_and(|t| now() - t < 3) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Wait at least three seconds between location updates".into(),
        ));
    }
    let timestamp = now();
    sqlx::query("INSERT INTO delivery_locations VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE courier_id=VALUES(courier_id),lat=VALUES(lat),lng=VALUES(lng),reported_at=VALUES(reported_at)").bind(&oid).bind(&u.id).bind(v.lat).bind(v.lng).bind(timestamp).execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = s.events.send((
        oid,
        OrderEvent::Location(CourierLocation {
            lat: v.lat,
            lng: v.lng,
            reported_at: timestamp,
        }),
    ));
    Ok(Json(PositionResult {
        reported_at: timestamp,
    }))
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
    let row: Option<(f64, f64, i64)> =
        sqlx::query_as("SELECT lat,lng,reported_at FROM delivery_locations WHERE order_id=?")
            .bind(oid)
            .fetch_optional(&s.db)
            .await?;
    Ok(Json(
        row.map(|(lat, lng, reported_at)| LocationSnapshot::Report {
            position: CourierLocation {
                lat,
                lng,
                reported_at,
            },
            stale: now() - reported_at > 15,
        })
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
    chat_access(&s, &u, &oid).await?;
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
    let mut headers = HeaderMap::new();
    let Ok(cookie) = format!("{SESSION_COOKIE}={}", token.secret()).parse() else {
        return false;
    };
    headers.insert("cookie", cookie);
    match crate::auth::authenticate(s, &headers).await {
        Ok(current) if current.id == u.id => chat_access(s, &current, oid).await.is_ok(),
        _ => false,
    }
}
