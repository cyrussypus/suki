use crate::{
    auth::{self, User},
    *,
};
use axum::{
    extract::{Path, State},
    http::{HeaderValue, header::SET_COOKIE},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as ENCODING};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use suki_shared::auth::{
    ACCESS_SECONDS, DeviceSession, REFRESH_COOKIE, REFRESH_SECONDS, SESSION_COOKIE,
};

const HEADER: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";

#[derive(Serialize, Deserialize)]
pub(crate) struct Claims {
    iss: String,
    aud: String,
    pub sub: String,
    pub sid: String,
    jti: String,
    iat: i64,
    exp: i64,
}

pub struct SessionKey(hmac::Key);

impl SessionKey {
    pub fn from_hex(value: &str) -> std::result::Result<Self, &'static str> {
        let mut bytes = [0; 32];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| "SESSION_SIGNING_KEY must be 64 hex characters")?;
        Ok(Self(hmac::Key::new(hmac::HMAC_SHA256, &bytes)))
    }
}

fn jwt(s: &App, uid: &str, session: &str, issued: i64) -> String {
    let claims = Claims {
        iss: "suki".into(),
        aud: "suki-api".into(),
        sub: uid.into(),
        sid: session.into(),
        jti: id(),
        iat: issued,
        exp: issued + ACCESS_SECONDS,
    };
    let payload = ENCODING.encode(serde_json::to_vec(&claims).expect("valid JWT claims"));
    let message = format!("{HEADER}.{payload}");
    let signature = hmac::sign(&s.session_key.0, message.as_bytes());
    format!("{message}.{}", ENCODING.encode(signature))
}

pub(crate) fn check_jwt(s: &App, value: &str, expired_ok: bool) -> Option<Claims> {
    if value.len() > 2048 {
        return None;
    }
    let (message, signature) = value.rsplit_once('.')?;
    let (header, payload) = message.split_once('.')?;
    if header != HEADER {
        return None;
    }
    hmac::verify(
        &s.session_key.0,
        message.as_bytes(),
        &ENCODING.decode(signature).ok()?,
    )
    .ok()?;
    let claims: Claims = serde_json::from_slice(&ENCODING.decode(payload).ok()?).ok()?;
    let now = now();
    (claims.iss == "suki"
        && claims.aud == "suki-api"
        && claims.iat <= now
        && claims.iat.checked_add(ACCESS_SECONDS) == Some(claims.exp)
        && (expired_ok || claims.exp > now))
        .then_some(claims)
}

pub(crate) fn claims(s: &App, value: &str) -> Option<Claims> {
    check_jwt(s, value, false)
}

fn unauthorized() -> ApiError {
    ApiError(
        StatusCode::UNAUTHORIZED,
        "Session expired or revoked".into(),
    )
}

fn random_token() -> Result<String> {
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Session service unavailable".into(),
        )
    })?;
    Ok(ENCODING.encode(bytes))
}

pub(crate) fn cookie(h: &HeaderMap, name: &str) -> Option<String> {
    h.get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|v| {
            v.trim()
                .split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, v)| v.to_owned())
        })
}

fn refresh_cookie(h: &HeaderMap) -> Option<String> {
    cookie(h, REFRESH_COOKIE).filter(|v| {
        v.len() == 43
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

fn response(s: &App, access: &str, refresh: &str, access_age: i64, refresh_age: i64) -> Response {
    let mut headers = HeaderMap::new();
    let secure = if s.secure_cookies { "; Secure" } else { "" };
    for (name, value, path, age) in [
        (SESSION_COOKIE, access, "/", access_age),
        (REFRESH_COOKIE, refresh, "/api/auth", refresh_age),
    ] {
        headers.append(
            SET_COOKIE,
            HeaderValue::from_str(&format!(
                "{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={age}{secure}"
            ))
            .expect("ASCII session cookie"),
        );
    }
    (headers, Json(Acknowledged { ok: true })).into_response()
}

pub(crate) async fn issue(
    s: &App,
    uid: &str,
    headers: &HeaderMap,
    login_attempt: &str,
) -> Result<Response> {
    let mut tx = s.db.begin().await?;
    let (role, status, email): (String, String, String) =
        sqlx::query_as("SELECT role,status,email FROM users WHERE id=? FOR UPDATE")
            .bind(uid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(unauthorized)?;
    auth::check_account_status(&status)?;
    s.admin_emails.check(&role, &email)?;
    let issued = now();
    sqlx::query("DELETE FROM user_sessions WHERE user_id=? AND expires_at<=?")
        .bind(uid)
        .bind(issued)
        .execute(&mut *tx)
        .await?;
    // Replace the session previously held by this client only if it belongs to this account.
    if let Some(old) = refresh_cookie(headers) {
        sqlx::query("DELETE FROM user_sessions WHERE user_id=? AND refresh_token_hash=?")
            .bind(uid)
            .bind(hash(&old))
            .execute(&mut *tx)
            .await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_sessions WHERE user_id=?")
        .bind(uid)
        .fetch_one(&mut *tx)
        .await?;
    if count >= 20 {
        sqlx::query(
            "DELETE FROM user_sessions WHERE user_id=? ORDER BY last_used_at, created_at LIMIT 1",
        )
        .bind(uid)
        .execute(&mut *tx)
        .await?;
    }
    let device = headers
        .get("x-suki-device")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| text(v, 1, 120).ok())
        .filter(|v| !v.chars().any(char::is_control))
        .unwrap_or_else(|| "Web browser".into());
    let sid = id();
    let refresh = random_token()?;
    sqlx::query("INSERT INTO user_sessions(id,user_id,device_name,refresh_token_hash,expires_at,created_at,last_used_at) VALUES (?,?,?,?,?,?,?)")
        .bind(&sid).bind(uid).bind(device).bind(hash(&refresh)).bind(issued + REFRESH_SECONDS).bind(issued).bind(issued)
        .execute(&mut *tx).await?;
    sqlx::query("DELETE FROM login_attempts WHERE email_hash=?")
        .bind(login_attempt)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(response(
        s,
        &jwt(s, uid, &sid, issued),
        &refresh,
        ACCESS_SECONDS,
        REFRESH_SECONDS,
    ))
}

async fn locate(s: &App, hash: &str) -> Result<Option<(String, String)>> {
    Ok(sqlx::query_as("SELECT id,user_id FROM user_sessions WHERE refresh_token_hash=? UNION ALL SELECT s.id,s.user_id FROM spent_refresh_tokens t JOIN user_sessions s ON s.id=t.session_id WHERE t.token_hash=? LIMIT 1")
        .bind(hash).bind(hash).fetch_optional(&s.db).await?)
}

pub async fn refresh(State(s): State<App>, headers: HeaderMap) -> Result<Response> {
    let digest = hash(&refresh_cookie(&headers).ok_or_else(unauthorized)?);
    let (sid, uid) = locate(&s, &digest).await?.ok_or_else(unauthorized)?;
    let mut tx = s.db.begin().await?;
    let (role, status, email): (String, String, String) =
        sqlx::query_as("SELECT role,status,email FROM users WHERE id=? FOR UPDATE")
            .bind(&uid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(unauthorized)?;
    auth::check_account_status(&status)?;
    s.admin_emails.check(&role, &email)?;
    let (current, expires): (String, i64) = sqlx::query_as(
        "SELECT refresh_token_hash,expires_at FROM user_sessions WHERE id=? FOR UPDATE",
    )
    .bind(&sid)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(unauthorized)?;
    let issued = now();
    if expires <= issued {
        return Err(unauthorized());
    }
    if current != digest {
        let used: i64 = sqlx::query_scalar(
            "SELECT used_at FROM spent_refresh_tokens WHERE token_hash=? AND session_id=?",
        )
        .bind(&digest)
        .bind(&sid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(unauthorized)?;
        // Duplicate in-flight requests (e.g. two web tabs) never receive credentials.
        if issued - used <= 5 {
            return Err(ApiError::conflict("Session refresh already in progress"));
        }
        sqlx::query("DELETE FROM user_sessions WHERE id=?")
            .bind(&sid)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(unauthorized());
    }
    let next = random_token()?;
    sqlx::query("INSERT INTO spent_refresh_tokens(token_hash,session_id,used_at) VALUES (?,?,?)")
        .bind(&digest)
        .bind(&sid)
        .bind(issued)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE user_sessions SET refresh_token_hash=?,last_used_at=? WHERE id=?")
        .bind(hash(&next))
        .bind(issued)
        .bind(&sid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(response(
        &s,
        &jwt(&s, &uid, &sid, issued),
        &next,
        ACCESS_SECONDS.min(expires - issued),
        expires - issued,
    ))
}

pub async fn logout(State(s): State<App>, headers: HeaderMap) -> Result<Response> {
    if let Some(t) = auth::session_token(&headers) {
        if let Some(c) = check_jwt(&s, t.secret(), true) {
            sqlx::query("DELETE FROM user_sessions WHERE id=? AND user_id=?")
                .bind(c.sid)
                .bind(c.sub)
                .execute(&s.db)
                .await?;
        } else {
            sqlx::query("DELETE FROM sessions WHERE token_hash=?")
                .bind(hash(t.secret()))
                .execute(&s.db)
                .await?;
        }
    }
    if let Some(token) = refresh_cookie(&headers)
        && let Some((sid, _)) = locate(&s, &hash(&token)).await?
    {
        sqlx::query("DELETE FROM user_sessions WHERE id=?")
            .bind(sid)
            .execute(&s.db)
            .await?;
    }
    Ok(response(&s, "", "", 0, 0))
}

pub async fn list(
    State(s): State<App>,
    user: User,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceSession>>> {
    let current = auth::session_token(&headers)
        .and_then(|t| claims(&s, t.secret()))
        .map(|c| c.sid)
        .unwrap_or_default();
    let rows: Vec<(String,String,i64,i64,i64)> = sqlx::query_as("SELECT id,device_name,created_at,last_used_at,expires_at FROM user_sessions WHERE user_id=? AND expires_at>? ORDER BY last_used_at DESC LIMIT 20")
        .bind(&user.id).bind(now()).fetch_all(&s.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, device_name, created_at, last_used_at, expires_at)| DeviceSession {
                    current: id == current,
                    id,
                    device_name,
                    created_at,
                    last_used_at,
                    expires_at,
                },
            )
            .collect(),
    ))
}

pub async fn revoke(
    State(s): State<App>,
    user: User,
    Path(sid): Path<String>,
) -> Result<Json<Acknowledged>> {
    let deleted = sqlx::query("DELETE FROM user_sessions WHERE id=? AND user_id=?")
        .bind(sid)
        .bind(&user.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Session not found".into()));
    }
    Ok(Json(Acknowledged { ok: true }))
}
