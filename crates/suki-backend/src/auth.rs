use crate::*;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, request::Parts},
    middleware::Next,
};
use suki_shared::auth::AccountStatus;
pub use suki_shared::auth::{Account, Login, Registration};

use suki_shared::auth::{
    ARGON2_ITERATIONS, ARGON2_LANES, ARGON2_MEMORY_KIB, PASSWORD_MAX_BYTES, PASSWORD_MIN_BYTES,
    RegistrationResult, Role, SESSION_COOKIE, SessionAccount, SessionToken,
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

    pub(crate) fn check(&self, role: &str, address: &str) -> Result<()> {
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
    s.rate_limits.admin(&user.id)?;
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
    crate::sessions::cookie(h, SESSION_COOKIE).map(SessionToken::new)
}
pub async fn authenticate(s: &App, h: &HeaderMap) -> Result<User> {
    let token =
        session_token(h).ok_or(ApiError(StatusCode::UNAUTHORIZED, "Please sign in".into()))?;
    authenticate_token(s, &token, false).await
}

// An established socket rechecks the revocable device session. Its handshake
// still requires a fresh access token, just like any new authenticated request.
pub(crate) async fn authenticate_token(
    s: &App,
    token: &SessionToken,
    established_socket: bool,
) -> Result<User> {
    let account = if let Some(claims) = crate::sessions::check_jwt(s, token.secret(), established_socket) {
        sqlx::query_as::<_, Account>("SELECT u.id,u.name,u.email,u.phone,u.role,u.status FROM users u JOIN user_sessions s ON s.user_id=u.id WHERE s.id=? AND u.id=? AND s.expires_at>? AND u.status='active'")
            .bind(claims.sid).bind(claims.sub).bind(now()).fetch_optional(&s.db).await?
    } else if !token.secret().contains('.') {
        // Honor legacy sessions until their original expiration; issue no new ones.
        sqlx::query_as::<_, Account>("SELECT u.id,u.name,u.email,u.phone,u.role,u.status FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=? AND s.expires_at>? AND u.status='active'")
            .bind(hash(token.secret())).bind(now()).fetch_optional(&s.db).await?
    } else { None }.ok_or(ApiError(StatusCode::UNAUTHORIZED,"Session expired or account unavailable".into()))?;
    s.admin_emails.check(&account.role, &account.email)?;
    Ok(User(account))
}
pub(crate) fn rider_session_key(s: &App, token: &SessionToken) -> String {
    crate::sessions::check_jwt(s, token.secret(), true)
        .map_or_else(|| hash(token.secret()), |claims| claims.sid)
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
    let role = match input.role.as_str() {
        "customer" => Role::Customer,
        "vendor" => Role::Vendor,
        "courier" | "rider" => Role::Rider,
        _ => {
            return Err(ApiError::bad(
                "Registration supports customers, vendors and riders",
            ));
        }
    };
    let name = text(&input.name, 2, 100)?;
    let email = email(&input.email)?;
    let phone = text(&input.phone, 7, 30)?;
    text(&input.password, PASSWORD_MIN_BYTES, PASSWORD_MAX_BYTES)?;
    throttle(&s, &hash(&format!("register:{email}"))).await?;
    let shop = if role == Role::Vendor {
        Some(text(input.shop_name.as_deref().unwrap_or(""), 2, 120)?)
    } else {
        None
    };
    let password = password_hash(input.password).await?;
    let user_id = id();
    let status = match role {
        Role::Customer => AccountStatus::Active,
        _ => AccountStatus::PendingApproval,
    }
    .as_str();
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO users(id,name,email,phone,password_hash,role,status,created_at) VALUES(?,?,?,?,?,?,?,?)").bind(&user_id).bind(name).bind(email).bind(phone).bind(password).bind(role.as_str()).bind(status).bind(now()).execute(&mut *tx).await?;
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
pub async fn login(
    State(s): State<App>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<impl IntoResponse> {
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
    check_account_status(&status)?;
    s.admin_emails.check(&role, &stored_email)?;
    crate::sessions::issue(&s, &uid, &headers, &key).await
}

pub(crate) fn check_account_status(status: &str) -> Result<()> {
    if status == AccountStatus::PendingApproval.as_str() {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Account pending admin approval.".into(),
        ));
    }
    if status != AccountStatus::Active.as_str() {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            if status == AccountStatus::Rejected.as_str() {
                "Account application was rejected."
            } else {
                "Account is inactive."
            }
            .into(),
        ));
    }
    Ok(())
}

pub async fn me(State(s): State<App>, user: User) -> Result<Json<SessionAccount>> {
    let approved_at = sqlx::query_scalar(
        "SELECT decided_at FROM onboarding_decisions WHERE user_id=? AND decision='active'",
    )
    .bind(&user.id)
    .fetch_optional(&s.db)
    .await?;
    Ok(Json(SessionAccount {
        admin_access: user.has_role(Role::Admin),
        account: user.0,
        approved_at,
    }))
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
            sqlx::query("INSERT INTO users (id,name,email,phone,password_hash,role,status,created_at) VALUES (?,?,?,?,?,'admin','active',?)")
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
