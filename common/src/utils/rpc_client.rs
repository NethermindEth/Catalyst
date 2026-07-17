use super::retry::backoff_retry_with_timeout;
use anyhow::Error;
use http::{HeaderMap, HeaderValue};
use jsonrpsee::{
    core::client::{ClientT, Error as JsonRpcError},
    http_client::{CustomCertStore, HttpClient, HttpClientBuilder},
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    iat: usize,
    exp: usize,
}

const JWT_TOKEN_EXPIRATION_TIME_SECONDS: usize = 3600;

fn create_jwt_token(secret: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let now: usize = chrono::Utc::now().timestamp().try_into()?;
    let claims = Claims {
        iat: now,
        exp: now + JWT_TOKEN_EXPIRATION_TIME_SECONDS,
    };

    Ok(encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )?)
}

pub struct TlsConfig {
    pub ca_cert: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

pub struct JSONRPCClient {
    url: String,
    timeout: Duration,
    jwt_secret: Option<[u8; 32]>,
    client: RwLock<HttpClient>,
    tls_config: Option<TlsConfig>,
}

impl JSONRPCClient {
    /// Creates a new RpcClient with JWT authentication.
    pub fn new_with_timeout_and_jwt(
        url: &str,
        timeout: Duration,
        jwt_secret: &[u8],
    ) -> Result<Self, Error> {
        if url.is_empty() {
            return Err(anyhow::anyhow!("URL is empty"));
        }

        let jwt_secret: [u8; 32] = jwt_secret
            .try_into()
            .map_err(|e| anyhow::anyhow!("Invalid JWT secret: {e}"))?;
        let client = Self::create_client_with_jwt(url, timeout, &jwt_secret)?;

        Ok(Self {
            url: url.to_string(),
            timeout,
            jwt_secret: Some(jwt_secret),
            client: RwLock::new(client),
            tls_config: None,
        })
    }

    fn create_client_with_jwt(
        url: &str,
        timeout: Duration,
        jwt_secret: &[u8; 32],
    ) -> Result<HttpClient, Error> {
        let jwt_token = create_jwt_token(jwt_secret)
            .map_err(|e| anyhow::anyhow!("Failed to create JWT token: {e}"))?;

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {jwt_token}")).map_err(|e| {
                anyhow::anyhow!("Failed to create header value from jwt token: {e}")
            })?,
        );

        let client = HttpClientBuilder::default()
            .request_timeout(timeout)
            .set_headers(headers)
            .build(url)
            .map_err(|e| anyhow::anyhow!("Failed to create authenticated HTTP client: {e}"))?;

        Ok(client)
    }

    pub fn new_with_timeout(url: &str, timeout: Duration) -> Result<Self, Error> {
        let client = Self::create_client(url, timeout)?;
        Ok(Self {
            url: url.to_string(),
            timeout,
            jwt_secret: None,
            client: RwLock::new(client),
            tls_config: None,
        })
    }

    fn create_client(url: &str, timeout: Duration) -> Result<HttpClient, Error> {
        let client = HttpClientBuilder::default()
            .request_timeout(timeout)
            .build(url)
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {e}"))?;
        Ok(client)
    }

    pub fn new_with_tls_and_timeout(
        url: &str,
        timeout: Duration,
        tls_config: TlsConfig,
    ) -> Result<Self, Error> {
        let client = Self::create_client_with_tls(url, timeout, &tls_config)?;
        Ok(Self {
            url: url.to_string(),
            timeout,
            jwt_secret: None,
            client: RwLock::new(client),
            tls_config: Some(tls_config),
        })
    }

    fn load_certs(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
        Ok(CertificateDer::pem_file_iter(path)?.collect::<Result<Vec<_>, _>>()?)
    }

    fn load_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
        Ok(PrivateKeyDer::from_pem_file(path)?)
    }

    fn create_client_with_tls(
        url: &str,
        timeout: Duration,
        tls_config: &TlsConfig,
    ) -> Result<HttpClient, Error> {
        // rustls 0.23 needs a process-wide default crypto provider installed once.
        // jsonrpsee's own tls feature uses "ring", so match that here to avoid conflicts.
        let _ = rustls::crypto::ring::default_provider().install_default();

        // Trust anchor used to verify the *server's* certificate.
        let mut roots = RootCertStore::empty();
        for cert in Self::load_certs(&tls_config.ca_cert)? {
            roots.add(cert)?;
        }

        // Our own identity, presented to the server for mTLS.
        let client_certs = Self::load_certs(&tls_config.client_cert)?;
        let client_key = Self::load_key(&tls_config.client_key)?;

        let tls_config: CustomCertStore = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(client_certs, client_key)?;

        let client = HttpClientBuilder::new()
            .request_timeout(timeout)
            .with_custom_cert_store(tls_config)
            .build(url)
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client with TLS: {e}"))?;

        Ok(client)
    }

    pub async fn call_method(&self, method: &str, params: Vec<Value>) -> Result<Value, Error> {
        let result = {
            let client_guard = self.client.read().await;
            client_guard.request(method, params.clone()).await
        };

        match result {
            Ok(result) => Ok(result),
            Err(JsonRpcError::Transport(err)) => {
                if err.to_string().contains("401") {
                    tracing::trace!("401 error, JWT token expired, recreating client");
                    self.recreate_client().await?;
                    return self
                        .client
                        .read()
                        .await
                        .request(method, params)
                        .await
                        .map_err(Error::from);
                }
                Err(anyhow::anyhow!("Http transport error: {err}."))
            }
            Err(err) => Err(Error::from(err)),
        }
    }

    pub async fn call_method_with_retry(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<Value, Error> {
        let result = backoff_retry_with_timeout(
            || async { self.call_method(method, params.clone()).await },
            Duration::from_millis(10),
            Duration::from_secs(1),
            self.timeout,
        )
        .await;

        result.map_err(|e| {
            anyhow::anyhow!("JSONRPCClient: Failed to call method {method} with retry: {e}")
        })
    }

    async fn recreate_client(&self) -> Result<(), Error> {
        let new_client = (if let Some(tls_config) = &self.tls_config {
            Self::create_client_with_tls(&self.url, self.timeout, tls_config)
        } else if let Some(jwt_secret) = &self.jwt_secret {
            Self::create_client_with_jwt(&self.url, self.timeout, jwt_secret)
        } else {
            Self::create_client(&self.url, self.timeout)
        })
        .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {e}"))?;

        tracing::trace!("Created new client");
        *self.client.write().await = new_client;
        Ok(())
    }
}

/// A direct HTTP client that doesn't use JSON-RPC
pub struct HttpRPCClient {
    client: RwLock<reqwest::Client>,
    base_url: String,
    timeout: Duration,
    jwt_secret: [u8; 32],
}

impl HttpRPCClient {
    /// Creates a new DirectHttpClient with JWT authentication
    pub fn new_with_jwt(
        base_url: &str,
        timeout: Duration,
        jwt_secret: &[u8],
    ) -> Result<Self, Error> {
        if base_url.is_empty() {
            return Err(anyhow::anyhow!("URL is empty"));
        }

        let jwt_secret_bytes: [u8; 32] = jwt_secret
            .try_into()
            .map_err(|e| anyhow::anyhow!("Invalid JWT secret: {e}"))?;

        let client = Self::create_client(timeout, &jwt_secret_bytes)?;

        Ok(Self {
            client: RwLock::new(client),
            base_url: base_url.to_string(),
            timeout,
            jwt_secret: jwt_secret_bytes,
        })
    }

    fn create_client(timeout: Duration, jwt_secret: &[u8; 32]) -> Result<reqwest::Client, Error> {
        let jwt_token = create_jwt_token(jwt_secret)
            .map_err(|e| anyhow::anyhow!("Failed to create JWT token: {e}"))?;

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .default_headers({
                let mut headers = HeaderMap::new();
                headers.insert(
                    "authorization",
                    HeaderValue::from_str(&format!("Bearer {jwt_token}")).map_err(|e| {
                        anyhow::anyhow!("Failed to create header value from jwt token: {e}")
                    })?,
                );
                headers.insert("content-type", HeaderValue::from_static("application/json"));
                headers
            })
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {e}"))?;

        Ok(client)
    }

    pub async fn retry_request_with_timeout<T>(
        &self,
        method: http::Method,
        endpoint: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<Value, Error>
    where
        T: serde::Serialize,
    {
        let result = backoff_retry_with_timeout(
            || async {
                let response = self.request_json(method.clone(), endpoint, payload).await;

                if let Err(ref e) = response {
                    tracing::error!(
                        "Failed to call driver RPC for API '{}': {}. Retrying...",
                        endpoint,
                        e
                    );
                    self.recreate_client().await?;
                }

                response
            },
            Duration::from_millis(10),
            Duration::from_secs(1),
            timeout,
        )
        .await;

        result.map_err(|err| {
            anyhow::anyhow!("Failed to call driver RPC for API '{}': {}", endpoint, err)
        })
    }

    /// Send a request to the specified endpoint with the given method and payload
    pub async fn request_json<T: Serialize>(
        &self,
        method: http::Method,
        endpoint: &str,
        payload: &T,
    ) -> Result<Value, Error> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        let mut response = self
            .client
            .read()
            .await
            .request(method.clone(), &url)
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow::anyhow!("HttpRPCClient: request timed out: {e}")
                } else {
                    anyhow::anyhow!("HttpRPCClient: failed to send HTTP request: {e}")
                }
            })?;

        if response.status() == http::StatusCode::UNAUTHORIZED {
            tracing::debug!("HttpRPCClient 401 error, recreating client");
            self.recreate_client().await?;
            response = self
                .client
                .read()
                .await
                .request(method, &url)
                .json(payload)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send HTTP request: {e}"))?;
        }

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "HTTP request failed with status: {}, body: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }

        response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response as JSON: {e}"))
    }

    pub async fn recreate_client(&self) -> Result<(), Error> {
        let new_client = Self::create_client(self.timeout, &self.jwt_secret)
            .map_err(|e| anyhow::anyhow!("Failed to create HttpRPCClient: {e}"))?;

        tracing::debug!("Created new HttpRPCClient client");
        *self.client.write().await = new_client;
        Ok(())
    }
}
