use crate::{ApiError, App, Result, auth};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const MAX_KEYS: usize = 10_000;

#[derive(Default)]
pub struct RateLimits(Mutex<HashMap<String, VecDeque<Instant>>>);

impl RateLimits {
    pub fn upload(&self, user_id: &str) -> Result<()> {
        self.check(format!("upload:{user_id}"), 10)
    }

    pub fn rider(&self, token: &str) -> Result<()> {
        self.check(format!("rider:{}", crate::hash(token)), 120)
    }

    pub fn eta(&self, user_id: &str) -> Result<()> {
        self.check(format!("eta:{user_id}"), 2)
    }

    fn check(&self, key: String, limit: usize) -> Result<()> {
        let mut windows = self.0.lock().map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Rate limiter unavailable".into(),
            )
        })?;
        let now = Instant::now();
        windows.retain(|_, entries| {
            while entries
                .front()
                .is_some_and(|t| now.duration_since(*t) >= WINDOW)
            {
                entries.pop_front();
            }
            !entries.is_empty()
        });
        if !windows.contains_key(&key) && windows.len() >= MAX_KEYS {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Rate limiter at capacity".into(),
            ));
        }
        let entries = windows.entry(key).or_default();
        if entries.len() >= limit {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many requests; retry later".into(),
            ));
        }
        entries.push_back(now);
        Ok(())
    }
}

pub async fn enforce(State(s): State<App>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    let auth_path = path.starts_with("/api/auth/");
    let upload_path = (path.starts_with("/api/upload/")
        && matches!(method, Method::POST | Method::PUT | Method::PATCH))
        || (path == "/api/vendor/goods" && method == Method::POST);
    let rider_path = path.starts_with("/api/rider/")
        || (path.starts_with("/api/orders/")
            && path.ends_with("/location")
            && method == Method::PUT);
    let result = async {
        if auth_path {
            // Never trust forwarded headers from an untrusted client.
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Client address unavailable".into(),
                ))?;
            s.rate_limits.check(format!("auth:{}", peer.0.ip()), 5)?;
        }
        if upload_path || rider_path {
            let user = auth::authenticate(&s, req.headers()).await?;
            if upload_path {
                if path.starts_with("/api/vendor/goods")
                    || path.starts_with("/api/upload/products/")
                {
                    user.require(&["vendor"])?;
                }
                s.rate_limits.upload(&user.id)?;
            }
            if rider_path && user.role == "courier" {
                let token = auth::session_token(req.headers()).ok_or_else(ApiError::forbidden)?;
                s.rate_limits.rider(token.secret())?;
            }
            req.extensions_mut().insert(user);
        }
        Ok::<(), ApiError>(())
    }
    .await;
    match result {
        Ok(()) => next.run(req).await,
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enforces_exact_limit_and_keeps_identities_separate() {
        let limiter = RateLimits::default();
        for _ in 0..5 {
            assert!(limiter.check("a".into(), 5).is_ok());
        }
        assert_eq!(
            limiter.check("a".into(), 5).unwrap_err().0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(limiter.check("b".into(), 5).is_ok());
        let mut windows = limiter.0.lock().unwrap();
        windows.get_mut("a").unwrap().clear();
        windows
            .get_mut("a")
            .unwrap()
            .push_back(Instant::now() - WINDOW);
        drop(windows);
        assert!(limiter.check("a".into(), 5).is_ok());
    }
}
