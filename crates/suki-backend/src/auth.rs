use crate::*;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, header::SET_COOKIE, request::Parts},
    middleware::Next,
};
pub use suki_shared::auth::{Account, Login, Registration};

use suki_shared::auth::{
    ARGON2_ITERATIONS, ARGON2_LANES, ARGON2_MEMORY_KIB, PASSWORD_MAX_BYTES, PASSWORD_MIN_BYTES,
    RegistrationResult, Role, SESSION_COOKIE, SESSION_SECONDS, SessionAccount, SessionToken,
};

#[derive(Clone, Default)]
pub struct AdminEmails(std::collections::HashSet<String>);

impl AdminEmails {
    pub fn parse(value: &str) -> std::result::Result<Self, &'static str> {
        value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                email(s).map_err(|_| "ADMIN_EMAILS must contain comma-separated email addresses")
            })
            .collect::<std::result::Result<_, _>>()
            .map(Self)
    }

    fn contains(&self, address: &str) -> bool {
        self.0.contains(&address.trim().to_lowercase())
    }

    fn check(&self, role: &str, address: &str) -> Result<()> {
        if role == Role::Admin.as_str() && !self.contains(address) {
            return Err(ApiError::forbidden());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct User(Account);

pub async fn require_admin_auth(
    State(s): State<App>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    // authenticate checks the allowlist against the session owner's stored email.
    let user = authenticate(&s, request.headers()).await?;
    user.require(&[Role::Admin.as_str()])?;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

impl std::ops::Deref for User {
    type Target = Account;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl User {
    pub fn require(&self, roles: &[&str]) -> Result<()> {
        if roles.contains(&self.role.as_str()) {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}
impl FromRequestParts<App> for User {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, s: &App) -> Result<Self> {
        if let Some(user) = parts.extensions.get::<User>() {
            return Ok(user.clone());
        }
        authenticate(s, &parts.headers).await
    }
}
pub fn session_token(h: &HeaderMap) -> Option<SessionToken> {
    h.get("cookie")?.to_str().ok()?.split(';').find_map(|s| {
        s.trim()
            .strip_prefix(SESSION_COOKIE)
            .and_then(|s| s.strip_prefix('='))
            .map(|value| SessionToken::new(value.to_owned()))
    })
}
pub async fn authenticate(s: &App, h: &HeaderMap) -> Result<User> {
    let token =
        session_token(h).ok_or(ApiError(StatusCode::UNAUTHORIZED, "Please sign in".into()))?;
    let account = sqlx::query_as::<_,Account>("SELECT u.id,u.name,u.email,u.phone,u.role,u.status FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=? AND s.expires_at>? AND u.status='active'").bind(hash(token.secret())).bind(now()).fetch_optional(&s.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"Session expired or account unavailable".into()))?;
    s.admin_emails.check(&account.role, &account.email)?;
    Ok(User(account))
}
fn email(s: &str) -> Result<String> {
    let s = text(s, 5, 190)?.to_lowercase();
    if !s.contains('@') || s.contains(char::is_whitespace) {
        return Err(ApiError::bad("Enter a valid email address"));
    }
    Ok(s)
}
static PASSWORD_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
pub async fn password_hash(password: String) -> Result<String> {
    let permit = PASSWORD_JOBS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Authentication is busy; retry shortly".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        argon2id()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|_| ApiError::bad("Unable to hash password"))
    })
    .await
    .map_err(|_| ApiError::bad("Password service unavailable"))?
}
fn argon2id() -> Argon2<'static> {
    let params = argon2::Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_LANES, None)
        .expect("static Argon2 parameters must be valid");
    Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

async fn throttle(s: &App, key: &str) -> Result<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO login_attempts(email_hash,attempts,reset_at) VALUES (?,0,?) ON DUPLICATE KEY UPDATE attempts=IF(reset_at<?,0,attempts), reset_at=IF(reset_at<?,VALUES(reset_at),reset_at)").bind(key).bind(now()+900).bind(now()).bind(now()).execute(&mut *tx).await?;
    let attempts: i32 =
        sqlx::query_scalar("SELECT attempts FROM login_attempts WHERE email_hash=? FOR UPDATE")
            .bind(key)
            .fetch_one(&mut *tx)
            .await?;
    if attempts >= 10 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts; retry in 15 minutes".into(),
        ));
    }
    sqlx::query("UPDATE login_attempts SET attempts=attempts+1 WHERE email_hash=?")
        .bind(key)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn register(
    State(s): State<App>,
    Json(input): Json<Registration>,
) -> Result<Json<RegistrationResult>> {
    if input.role != "customer" && input.role != "vendor" {
        return Err(ApiError::bad("Registration supports customers and vendors"));
    }
    let name = text(&input.name, 2, 100)?;
    let email = email(&input.email)?;
    let phone = text(&input.phone, 7, 30)?;
    text(&input.password, PASSWORD_MIN_BYTES, PASSWORD_MAX_BYTES)?;
    throttle(&s, &hash(&format!("register:{email}"))).await?;
    let shop = if input.role == "vendor" {
        Some(text(input.shop_name.as_deref().unwrap_or(""), 2, 120)?)
    } else {
        None
    };
    let password = password_hash(input.password).await?;
    let user_id = id();
    let status = if shop.is_some() { "pending" } else { "active" };
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO users(id,name,email,phone,password_hash,role,status,created_at) VALUES(?,?,?,?,?,?,?,?)").bind(&user_id).bind(name).bind(email).bind(phone).bind(password).bind(&input.role).bind(status).bind(now()).execute(&mut *tx).await?;
    if let Some(shop) = shop {
        sqlx::query("INSERT INTO vendor_profiles(user_id,shop_name) VALUES (?,?)")
            .bind(&user_id)
            .bind(shop)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(RegistrationResult {
        id: user_id,
        status: status.into(),
    }))
}
pub async fn login(State(s): State<App>, Json(input): Json<Login>) -> Result<impl IntoResponse> {
    let email = email(&input.email)?;
    text(&input.password, 1, PASSWORD_MAX_BYTES)?;
    let key = hash(&email);
    throttle(&s, &key).await?;
    let row: Option<(String, String, String, String, String)> =
        sqlx::query_as("SELECT id,password_hash,status,role,email FROM users WHERE email=?")
            .bind(email)
            .fetch_optional(&s.db)
            .await?;
    // Verify a real Argon2 hash even for unknown users.
    static FALLBACK: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    let phc = match row.as_ref() {
        Some(r) => r.1.clone(),
        None => FALLBACK
            .get_or_try_init(|| password_hash("suki-invalid-login-placeholder".into()))
            .await?
            .clone(),
    };
    let password = input.password;
    let permit = PASSWORD_JOBS.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Authentication is busy; retry shortly".into(),
        )
    })?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&phc)
            .map(|h| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &h)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    let Some((uid, _, status, role, stored_email)) = row.filter(|_| valid) else {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid email or password".into(),
        ));
    };
    if status != "active" {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Account is pending approval or inactive".into(),
        ));
    }
    s.admin_emails.check(&role, &stored_email)?;
    let token = SessionToken::new(format!("{}{}", id(), id()));
    sqlx::query("INSERT INTO sessions VALUES (?,?,?)")
        .bind(hash(token.secret()))
        .bind(uid)
        .bind(now() + SESSION_SECONDS)
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM login_attempts WHERE email_hash=?")
        .bind(key)
        .execute(&s.db)
        .await?;
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECONDS}{}",
        token.secret(),
        if s.secure_cookies { "; Secure" } else { "" }
    );
    Ok(([(SET_COOKIE, cookie)], Json(Acknowledged { ok: true })))
}
pub async fn logout(State(s): State<App>, headers: HeaderMap) -> Result<impl IntoResponse> {
    if let Some(t) = session_token(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash=?")
            .bind(hash(t.secret()))
            .execute(&s.db)
            .await?;
    }
    Ok((
        [(
            SET_COOKIE,
            "suki_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        )],
        Json(Acknowledged { ok: true }),
    ))
}
pub async fn me(user: User) -> Json<SessionAccount> {
    Json(SessionAccount {
        admin_access: user.has_role(Role::Admin),
        account: user.0,
    })
}
pub async fn bootstrap(
    db: &MySqlPool,
    admin_emails: &AdminEmails,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if let Ok(address) = std::env::var("BOOTSTRAP_ADMIN_EMAIL") {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='admin'")
            .fetch_one(db)
            .await?;
        if count == 0 {
            let address = email(&address).map_err(|_| "Invalid BOOTSTRAP_ADMIN_EMAIL")?;
            if !admin_emails.contains(&address) {
                return Err("BOOTSTRAP_ADMIN_EMAIL must be included in ADMIN_EMAILS".into());
            }
            let pass = std::env::var("BOOTSTRAP_ADMIN_PASSWORD")?;
            text(&pass, 16, 128)
                .map_err(|_| "Bootstrap admin password must be at least 16 characters")?;
            let phc = password_hash(pass)
                .await
                .map_err(|_| "Password hash failed")?;
            sqlx::query("INSERT INTO users VALUES (?,?,?,?,?,'admin','active',?)")
                .bind(id())
                .bind("Suki Administrator")
                .bind(address)
                .bind("")
                .bind(phc)
                .bind(now())
                .execute(db)
                .await?;
        }
    }
    Ok(())
}
