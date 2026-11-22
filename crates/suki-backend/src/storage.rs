use crate::{ApiError, Result};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
};
use axum::http::StatusCode;

#[derive(Clone)]
pub struct R2 {
    client: Client,
    bucket: String,
    public_url: String,
}

impl R2 {
    pub fn from_env() -> std::result::Result<Option<Self>, Box<dyn std::error::Error>> {
        let values = [
            "R2_ACCOUNT_ID",
            "R2_ACCESS_KEY_ID",
            "R2_SECRET_ACCESS_KEY",
            "R2_BUCKET_NAME",
            "R2_PUBLIC_DOMAIN",
        ]
        .map(std::env::var);
        if values
            .iter()
            .all(|v| matches!(v, Err(std::env::VarError::NotPresent)))
        {
            return Ok(None);
        }
        let [account, access, secret, bucket, public] = values;
        let (account, access, secret, bucket, public) =
            (account?, access?, secret?, bucket?, public?);
        if account.is_empty()
            || !account.chars().all(|c| c.is_ascii_alphanumeric())
            || access.is_empty()
            || secret.is_empty()
            || bucket.is_empty()
            || !bucket
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        {
            return Err("Invalid R2 configuration".into());
        }
        let public = reqwest::Url::parse(&public)?;
        if public.scheme() != "https"
            || public.host_str().is_none()
            || !public.username().is_empty()
            || public.password().is_some()
            || public.query().is_some()
            || public.fragment().is_some()
            || public.path() != "/"
        {
            return Err("R2_PUBLIC_DOMAIN must be an HTTPS origin without a path".into());
        }
        let endpoint = std::env::var("R2_ENDPOINT_URL")
            .unwrap_or_else(|_| format!("https://{account}.r2.cloudflarestorage.com"));
        let endpoint_url = reqwest::Url::parse(&endpoint)?;
        if endpoint_url.host_str().is_none()
            || !endpoint_url.username().is_empty()
            || endpoint_url.password().is_some()
            || endpoint_url.query().is_some()
            || endpoint_url.fragment().is_some()
            || endpoint_url.path() != "/"
        {
            return Err("Invalid R2 endpoint".into());
        }
        if endpoint_url.scheme() != "https"
            && !(endpoint_url.scheme() == "http"
                && matches!(
                    endpoint_url.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]")
                ))
        {
            return Err("R2 endpoint must use HTTPS except for loopback testing".into());
        }
        Self::new(&endpoint, &bucket, &public, &access, &secret).map(Some)
    }

    pub fn new(
        endpoint: &str,
        bucket: &str,
        public: &reqwest::Url,
        access: &str,
        secret: &str,
    ) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .credentials_provider(Credentials::new(
                access.to_owned(),
                secret.to_owned(),
                None,
                None,
                "suki-r2",
            ))
            .endpoint_url(endpoint.to_owned())
            .force_path_style(true)
            .timeout_config(
                aws_sdk_s3::config::timeout::TimeoutConfig::builder()
                    .connect_timeout(std::time::Duration::from_secs(5))
                    .operation_timeout(std::time::Duration::from_secs(20))
                    .operation_attempt_timeout(std::time::Duration::from_secs(10))
                    .build(),
            )
            .build();
        Ok(Self {
            client: Client::from_conf(config),
            bucket: bucket.to_owned(),
            public_url: public.as_str().trim_end_matches('/').to_owned(),
        })
    }

    pub async fn put(&self, key: &str, data: Vec<u8>) -> Result<String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type("image/jpeg")
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "R2 upload failed");
                ApiError(StatusCode::BAD_GATEWAY, "Image storage unavailable".into())
            })?;
        Ok(format!("{}/{key}", self.public_url))
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "R2 read failed");
                ApiError(StatusCode::BAD_GATEWAY, "Image storage unavailable".into())
            })?;
        // Re-encoded uploads are limited to 5 MiB; never buffer unbounded remote objects.
        if object
            .content_length()
            .is_none_or(|length| !(0..=5 * 1024 * 1024).contains(&length))
        {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "Invalid stored image".into(),
            ));
        }
        let mut stream = object.body;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "Image storage unavailable".into()))?
        {
            if bytes.len().saturating_add(chunk.len()) > 5 * 1024 * 1024 {
                return Err(ApiError(
                    StatusCode::BAD_GATEWAY,
                    "Invalid stored image".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
