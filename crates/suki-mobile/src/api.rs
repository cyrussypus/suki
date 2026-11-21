use futures_util::{Stream, StreamExt};
use reqwest::{Client, Method, Url};
use serde::{Serialize, de::DeserializeOwned};
use std::{fmt, pin::Pin};
use suki_shared::{ErrorResponse, realtime::OrderEvent};

#[derive(Clone)]
pub struct Api {
    client: Client,
    origin: Url,
    #[cfg(not(target_arch = "wasm32"))]
    cookies: std::sync::Arc<reqwest::cookie::Jar>,
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            if origin.scheme() != "https"
                && !matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            {
                return Err("Use HTTPS for the marketplace API".into());
            }
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
            })
        }
        #[cfg(target_arch = "wasm32")]
        Ok(Self {
            client: Client::new(),
            origin,
        })
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
        let url = self
            .origin
            .join(&format!("/api{path}"))
            .expect("internal API path");
        let mut request = self
            .client
            .request(method, url)
            .header("X-Suki-Request", "1");
        if let Some(body) = body {
            request = request.json(body);
        }
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
        let mut url = self
            .origin
            .join(&format!("/api/orders/{order_id}/events"))
            .expect("internal event path");
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
}

fn decode(text: &str) -> Option<Live> {
    serde_json::from_str(text).ok().map(Live::Event)
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
