use futures_util::{Stream, StreamExt};
use reqwest::{Client, Method, Url};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fmt,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
#[cfg(not(target_arch = "wasm32"))]
use suki_shared::auth::REFRESH_COOKIE;
use suki_shared::{
    Acknowledged, ErrorResponse,
    auth::{Login, SessionAccount},
    realtime::OrderEvent,
};

#[derive(Clone)]
pub struct Api {
    client: Client,
    origin: Url,
    auth_lock: Arc<futures_util::lock::Mutex<()>>,
    generation: Arc<AtomicU64>,
    identity: Arc<AtomicU64>,
    #[cfg(not(target_arch = "wasm32"))]
    refresh_token: Arc<std::sync::Mutex<Option<String>>>,
    #[cfg(not(target_arch = "wasm32"))]
    cookies: std::sync::Arc<reqwest::cookie::Jar>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RememberedCredential {
    origin: String,
    token: String,
}

#[derive(Debug)]
pub struct Failure {
    pub status: Option<u16>,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug)]
pub enum Live {
    Connected,
    Event(OrderEvent),
}

type EventStream = Pin<Box<dyn Stream<Item = Live>>>;

impl Api {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        let base = web_sys::window()
            .ok_or("Missing browser window")?
            .location()
            .origin()
            .map_err(|_| "Missing preview origin")?;
        #[cfg(not(target_arch = "wasm32"))]
        let base = option_env!("SUKI_API_URL").unwrap_or("http://localhost:8080");
        #[cfg(target_arch = "wasm32")]
        let base = base.as_str();
        let origin = Url::parse(base).map_err(|_| "Invalid Suki API URL")?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err("Suki API needs a web origin without a path".into());
        }
        if origin.scheme() != "https"
            && !matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        {
            return Err("Use HTTPS for the marketplace API".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cookies = std::sync::Arc::new(reqwest::cookie::Jar::default());
            // Use the same public roots as the native WebSocket transport.
            // Reqwest's Android system verifier would need a separate Java runtime bridge.
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let tls = rustls::ClientConfig::builder_with_provider(
                rustls::crypto::aws_lc_rs::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .map_err(|_| "Could not initialize TLS versions")?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let client = Client::builder()
                .tls_backend_preconfigured(tls)
                .cookie_provider(cookies.clone())
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|_| "Could not initialize the Suki API client")?;
            Ok(Self {
                client,
                origin,
                cookies,
                auth_lock: Default::default(),
                generation: Default::default(),
                identity: Default::default(),
                refresh_token: Default::default(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        Ok(Self {
            client: Client::new(),
            origin,
            auth_lock: Default::default(),
            generation: Default::default(),
            identity: Default::default(),
        })
    }

    /// R2 URLs are supplied by the server, never by product form inputs.
    pub fn product_image(&self, product: &suki_shared::catalog::Product) -> Option<String> {
        product
            .image_url
            .as_deref()
            .and_then(|url| {
                let parsed = Url::parse(url).ok()?;
                (parsed.scheme() == "https"
                    && parsed.host_str().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.fragment().is_none())
                .then(|| parsed.to_string())
            })
            .or_else(|| self.image(&product.image))
    }

    pub fn image(&self, path: &str) -> Option<String> {
        if !path.starts_with("/images/") || path.contains("..") || path.contains('\\') {
            return None;
        }
        let url = self.origin.join(path).ok()?;
        (url.origin() == self.origin.origin() && url.path().starts_with("/images/"))
            .then(|| url.to_string())
    }

    pub async fn get<T: DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
    ) -> Result<T, Failure> {
        self.request::<T, ()>(Method::GET, path, None).await
    }

    pub async fn send<T, B>(&self, method: Method, path: &str, body: &B) -> Result<T, Failure>
    where
        T: DeserializeOwned + Send + 'static,
        B: Serialize,
    {
        self.request(method, path, Some(body)).await
    }

    fn builder(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = self
            .origin
            .join(&format!("/api{path}"))
            .expect("internal API path");
        self.client
            .request(method, url)
            .header("X-Suki-Request", "1")
            .timeout(std::time::Duration::from_secs(15))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_url(&self) -> Url {
        self.origin
            .join("/api/auth/refresh")
            .expect("internal refresh URL")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn put_refresh_cookie(&self) {
        if let Some(token) = self
            .refresh_token
            .lock()
            .expect("session vault mutex")
            .as_ref()
        {
            self.cookies.add_cookie_str(
                &format!("{REFRESH_COOKIE}={token}; Path=/api/auth"),
                &self.refresh_url(),
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn remember_refresh(&self, password_fallback: bool) -> Result<Option<String>, Failure> {
        use reqwest::cookie::CookieStore;
        let url = self.refresh_url();
        let cookie = self
            .cookies
            .cookies(&url)
            .and_then(|value| {
                value.to_str().ok().and_then(|s| {
                    s.split(';').find_map(|pair| {
                        pair.trim()
                            .strip_prefix(&format!("{REFRESH_COOKIE}="))
                            .map(str::to_owned)
                    })
                })
            })
            .ok_or_else(|| Failure {
                status: None,
                message: "The server did not return a device credential.".into(),
            })?;
        *self.refresh_token.lock().expect("session vault mutex") = Some(cookie.clone());
        if crate::vault::NATIVE {
            let was_locked = password_fallback
                && crate::vault::operate("enabled", None)
                    .await
                    .map_err(storage_failure)?
                    == "1";
            let record = serde_json::to_string(&RememberedCredential {
                origin: self.origin.origin().ascii_serialization(),
                token: cookie,
            })
            .expect("credential serialization");
            let state = crate::vault::operate(
                if password_fallback {
                    "password_save"
                } else {
                    "save"
                },
                Some(record),
            )
            .await
            .map_err(storage_failure)?;
            if was_locked && state == "0" {
                return Ok(Some(
                    "Signed in with your password. You can re-enable biometric login in Account."
                        .into(),
                ));
            }
        }
        Ok(None)
    }

    pub async fn restore(&self) -> Result<SessionAccount, Failure> {
        if crate::vault::NATIVE {
            let token = crate::vault::operate("load", None)
                .await
                .map_err(storage_failure)?;
            if token.is_empty() {
                return Err(Failure {
                    status: Some(401),
                    message: "Please sign in.".into(),
                });
            }
            let credential: RememberedCredential = serde_json::from_str(&token).map_err(|_| {
                storage_failure("Could not read the saved session. Please sign in.".into())
            })?;
            if credential.origin != self.origin.origin().ascii_serialization()
                || credential.token.len() != 43
                || !credential
                    .token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(Failure {
                    status: Some(401),
                    message: "Please sign in to this Suki service.".into(),
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                *self.refresh_token.lock().expect("session vault mutex") = Some(credential.token);
            }
        }
        self.get("/auth/me").await
    }

    pub async fn login(&self, login: &Login) -> Result<(SessionAccount, Option<String>), Failure> {
        let _guard = self.auth_lock.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        self.put_refresh_cookie();
        let request = self.builder(Method::POST, "/auth/login").json(login);
        #[cfg(not(target_arch = "wasm32"))]
        let request = request.header(
            "X-Suki-Device",
            if cfg!(target_os = "android") {
                "Android"
            } else if cfg!(target_os = "ios") {
                "iPhone or iPad"
            } else {
                "Suki desktop"
            },
        );
        Self::execute::<Acknowledged>(request).await?;
        self.generation.fetch_add(1, Ordering::Release);
        self.identity.fetch_add(1, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        let warning = self
            .remember_refresh(true)
            .await
            .unwrap_or_else(|e| Some(e.message));
        #[cfg(target_arch = "wasm32")]
        let warning = None;
        let account = Self::execute(self.builder(Method::GET, "/auth/me")).await?;
        Ok((account, warning))
    }

    pub async fn logout(&self) -> Result<Option<String>, Failure> {
        let _guard = self.auth_lock.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        self.put_refresh_cookie();
        Self::execute::<Acknowledged>(self.builder(Method::POST, "/auth/logout").json(&())).await?;
        self.generation.fetch_add(1, Ordering::Release);
        self.identity.fetch_add(1, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        {
            *self.refresh_token.lock().expect("session vault mutex") = None;
        }
        if crate::vault::NATIVE {
            return Ok(crate::vault::operate("clear", None).await.err());
        }
        Ok(None)
    }

    pub async fn biometric_enabled(&self) -> Result<bool, Failure> {
        let _guard = self.auth_lock.lock().await;
        crate::vault::operate("enabled", None)
            .await
            .map(|v| v == "1")
            .map_err(storage_failure)
    }

    pub async fn set_biometrics(&self, enabled: bool) -> Result<(), Failure> {
        let _guard = self.auth_lock.lock().await;
        crate::vault::operate(if enabled { "enable" } else { "disable" }, None)
            .await
            .map(|_| ())
            .map_err(storage_failure)
    }

    pub async fn confirm_sensitive_action(&self) -> Result<(), Failure> {
        if crate::vault::NATIVE {
            let _guard = self.auth_lock.lock().await;
            if crate::vault::operate("enabled", None)
                .await
                .map_err(storage_failure)?
                == "1"
            {
                crate::vault::operate("confirm", None)
                    .await
                    .map_err(storage_failure)?;
            }
        }
        Ok(())
    }

    async fn refresh(&self, previous: u64, identity: u64) -> Result<(), Failure> {
        let _guard = self.auth_lock.lock().await;
        if self.identity.load(Ordering::Acquire) != identity {
            return Err(Failure {
                status: Some(401),
                message: "Account changed; retry in the new session.".into(),
            });
        }
        if self.generation.load(Ordering::Acquire) != previous {
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self
                .refresh_token
                .lock()
                .expect("session vault mutex")
                .is_none()
            {
                return Err(Failure {
                    status: Some(401),
                    message: "Please sign in.".into(),
                });
            }
            self.put_refresh_cookie();
        }
        let request = || self.builder(Method::POST, "/auth/refresh").json(&());
        let first = Self::execute::<Acknowledged>(request()).await;
        #[cfg(target_arch = "wasm32")]
        if matches!(&first, Err(e) if e.status == Some(409)) {
            // Another tab may have updated the browser's HttpOnly cookie.
            delay(250).await;
            Self::execute::<Acknowledged>(request()).await?;
        } else {
            first?;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            first?;
            self.remember_refresh(false).await?;
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    async fn session_request<T: DeserializeOwned + Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
        allow_refresh: bool,
    ) -> Result<T, Failure> {
        let generation = self.generation.load(Ordering::Acquire);
        let identity = self.identity.load(Ordering::Acquire);
        let retry = allow_refresh.then(|| request.try_clone()).flatten();
        match Self::execute(request).await {
            Err(e) if e.status == Some(401) && retry.is_some() => {
                self.refresh(generation, identity).await?;
                Self::execute(retry.expect("clone available")).await
            }
            response => response,
        }
    }

    async fn request<T, B>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, Failure>
    where
        T: DeserializeOwned + Send + 'static,
        B: Serialize,
    {
        let mut request = self.builder(method, path);
        if let Some(body) = body {
            request = request.json(body);
        }
        self.session_request(
            request,
            !matches!(
                path,
                "/auth/login" | "/auth/register" | "/auth/logout" | "/auth/refresh"
            ),
        )
        .await
    }

    pub async fn upload<T: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        product: &suki_shared::catalog::CreateProductDto,
        photo: Option<Vec<u8>>,
    ) -> Result<T, Failure> {
        let url = self
            .origin
            .join(&format!("/api{path}"))
            .expect("internal API path");
        self.get::<SessionAccount>("/auth/me").await?;
        let mut form = reqwest::multipart::Form::new().text(
            "product",
            serde_json::to_string(product).expect("serializable product"),
        );
        if let Some(photo) = photo {
            form = form.part(
                "photo",
                reqwest::multipart::Part::bytes(photo).file_name("product-photo"),
            );
        }
        Self::execute(
            self.client
                .request(method, url)
                .header("X-Suki-Request", "1")
                .timeout(std::time::Duration::from_secs(30))
                .multipart(form),
        )
        .await
    }

    pub async fn upload_avatar(
        &self,
        photo: Vec<u8>,
    ) -> Result<suki_shared::catalog::AvatarImage, Failure> {
        let url = self
            .origin
            .join("/api/upload/avatar")
            .expect("internal API path");
        self.get::<SessionAccount>("/auth/me").await?;
        let form = reqwest::multipart::Form::new().part(
            "photo",
            reqwest::multipart::Part::bytes(photo).file_name("avatar"),
        );
        Self::execute(
            self.client
                .post(url)
                .header("X-Suki-Request", "1")
                .timeout(std::time::Duration::from_secs(30))
                .multipart(form),
        )
        .await
    }

    async fn execute<T: DeserializeOwned + Send + 'static>(
        request: reqwest::RequestBuilder,
    ) -> Result<T, Failure> {
        let call = async move {
            let response = request.send().await.map_err(|_| unavailable())?;
            let status = response.status();
            if !status.is_success() {
                return Err(Failure {
                    status: Some(status.as_u16()),
                    message: response
                        .json::<ErrorResponse>()
                        .await
                        .map(|error| error.error)
                        .unwrap_or_else(|_| format!("Request failed ({status})")),
                });
            }
            response.json().await.map_err(|_| Failure {
                status: Some(status.as_u16()),
                message: "The API returned an unreadable response".into(),
            })
        };
        #[cfg(not(target_arch = "wasm32"))]
        return runtime().spawn(call).await.map_err(|_| unavailable())?;
        #[cfg(target_arch = "wasm32")]
        call.await
    }

    pub fn events(&self, order_id: &str) -> EventStream {
        self.event_stream(&format!("/api/orders/{order_id}/chat/ws"))
    }

    pub fn tracking_events(&self, order_id: &str) -> EventStream {
        self.event_stream(&format!("/api/rider/location/stream?order_id={order_id}"))
    }

    fn event_stream(&self, path: &str) -> EventStream {
        let mut url = self.origin.join(path).expect("internal event path");
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .expect("WebSocket scheme");

        #[cfg(target_arch = "wasm32")]
        {
            use gloo_net::websocket::{Message, State, futures::WebSocket};
            let Ok(socket) = WebSocket::open(url.as_str()) else {
                return Box::pin(futures_util::stream::empty());
            };
            Box::pin(futures_util::stream::unfold(
                (socket, false),
                |(mut socket, opened)| async move {
                    if !opened {
                        for _ in 0..100 {
                            match socket.state() {
                                State::Open => return Some((Live::Connected, (socket, true))),
                                State::Connecting => delay(100).await,
                                _ => return None,
                            }
                        }
                        return None;
                    }
                    while let Some(Ok(frame)) = socket.next().await {
                        if let Message::Text(text) = frame
                            && let Some(event) = decode(&text)
                        {
                            return Some((event, (socket, true)));
                        }
                    }
                    None
                },
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use reqwest::cookie::CookieStore;
            use tokio_tungstenite::{
                connect_async,
                tungstenite::{Message, client::IntoClientRequest},
            };
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            let mut request = url
                .as_str()
                .into_client_request()
                .expect("valid WebSocket URL");
            let origin = self.origin.origin().ascii_serialization();
            let origin = option_env!("SUKI_APP_ORIGIN").unwrap_or(&origin);
            let Ok(origin) = origin.parse() else {
                return Box::pin(futures_util::stream::empty());
            };
            request.headers_mut().insert("origin", origin);
            if let Some(cookie) = self.cookies.cookies(&self.origin) {
                request.headers_mut().insert("cookie", cookie);
            }
            runtime().spawn(async move {
                let connect = tokio::time::timeout(std::time::Duration::from_secs(10), connect_async(request));
                let connection = tokio::select! {
                    _ = tx.closed() => return,
                    connected = connect => connected,
                };
                let Ok(Ok((mut socket, _))) = connection else { return };
                if tx.send(Live::Connected).await.is_err() { return; }
                loop {
                    tokio::select! {
                        _ = tx.closed() => return,
                        next = socket.next() => match next {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(event) = decode(&text) && tx.send(event).await.is_err() {
                                    return;
                                }
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                            _ => {}
                        }
                    }
                }
            });
            Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            }))
        }
    }
    pub fn location_sender(
        &self,
        order_id: &str,
    ) -> futures_channel::mpsc::UnboundedSender<String> {
        let mut url = self
            .origin
            .join(&format!("/api/rider/location/stream?order_id={order_id}"))
            .expect("internal tracking path");
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .expect("WebSocket scheme");
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<String>();
        #[cfg(target_arch = "wasm32")]
        {
            use futures_util::SinkExt;
            use gloo_net::websocket::{Message, State, futures::WebSocket};
            let Ok(socket) = WebSocket::open(url.as_str()) else {
                return tx;
            };
            dioxus::prelude::spawn(async move {
                for _ in 0..100 {
                    match socket.state() {
                        State::Open => break,
                        State::Connecting => delay(100).await,
                        _ => return,
                    }
                }
                if !matches!(socket.state(), State::Open) {
                    return;
                }
                let (mut output, input) = socket.split();
                let mut input = input.fuse();
                loop {
                    futures_util::select! {
                        payload = rx.next() => match payload { Some(body) => if output.send(Message::Text(body)).await.is_err() { break; }, None => break },
                        frame = input.next() => match frame { Some(Ok(Message::Text(text))) if text.contains("\"error\"") => break, Some(Ok(_)) => {}, _ => break },
                    }
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use futures_util::SinkExt;
            use reqwest::cookie::CookieStore;
            use tokio_tungstenite::{
                connect_async,
                tungstenite::{Message, client::IntoClientRequest},
            };
            let mut request = url
                .as_str()
                .into_client_request()
                .expect("valid WebSocket URL");
            let origin = self.origin.origin().ascii_serialization();
            let origin = option_env!("SUKI_APP_ORIGIN").unwrap_or(&origin);
            let Ok(origin) = origin.parse() else {
                return tx;
            };
            request.headers_mut().insert("origin", origin);
            if let Some(cookie) = self.cookies.cookies(&self.origin) {
                request.headers_mut().insert("cookie", cookie);
            }
            runtime().spawn(async move {
                let Ok(Ok((socket, _))) = tokio::time::timeout(std::time::Duration::from_secs(10), connect_async(request)).await else { return; };
                let (mut output, mut input) = socket.split();
                loop {
                    tokio::select! {
                        payload = rx.next() => match payload { Some(body) => if output.send(Message::Text(body.into())).await.is_err() { break; }, None => break },
                        frame = input.next() => match frame { Some(Ok(Message::Text(text))) if text.contains("\"error\"") => break, Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break, _ => {} },
                    }
                }
            });
        }
        tx
    }
}

fn decode(text: &str) -> Option<Live> {
    serde_json::from_str(text).ok().map(Live::Event)
}

fn storage_failure(message: String) -> Failure {
    Failure {
        status: None,
        message,
    }
}

fn unavailable() -> Failure {
    Failure {
        status: None,
        message: "Cannot reach Suki. Check your connection and retry.".into(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Suki network runtime")
    })
}

pub async fn delay(milliseconds: u32) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(milliseconds).await;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = runtime()
            .spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(milliseconds.into())).await;
            })
            .await;
    }
}
