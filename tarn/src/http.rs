use crate::error::TarnError;
use crate::model::{HttpTransportConfig, HttpVersionPreference, MultipartBody};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Response from an HTTP request.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub url: String,
    pub redirect_count: u32,
    pub headers: HashMap<String, String>,
    /// Raw headers preserving duplicates (e.g., multiple Set-Cookie)
    pub raw_headers: Vec<(String, String)>,
    /// Raw response body bytes before any text/JSON normalization.
    pub body_bytes: Vec<u8>,
    pub body: serde_json::Value,
    pub duration_ms: u64,
    pub timings: ResponseTimings,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResponseTimings {
    pub total_ms: u64,
    pub ttfb_ms: u64,
    pub body_read_ms: u64,
    pub connect_ms: Option<u64>,
    pub tls_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestTransportOptions {
    pub timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub follow_redirects: Option<bool>,
    pub max_redirs: Option<u32>,
}

#[derive(Debug)]
pub struct HttpClient {
    config: HttpTransportConfig,
    inner: reqwest::blocking::Client,
}

impl HttpClient {
    pub fn new(config: &HttpTransportConfig) -> Result<Self, TarnError> {
        if !config.has_custom_transport() {
            return Ok(Self {
                config: config.clone(),
                inner: shared_client()?.clone(),
            });
        }

        Ok(Self {
            config: config.clone(),
            inner: build_blocking_client(config, RequestTransportOptions::default(), None)?,
        })
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        options: RequestTransportOptions,
    ) -> Result<(reqwest::blocking::RequestBuilder, Arc<AtomicU32>), TarnError> {
        let request_client = self.client_for_request_options(options)?;
        let builder = request_client
            .client
            .request(parse_http_method(method)?, url);

        Ok((
            apply_http_version(builder, self.config.http_version),
            request_client.redirect_count,
        ))
    }

    fn multipart_request(
        &self,
        method: &str,
        url: &str,
        options: RequestTransportOptions,
    ) -> Result<(reqwest::blocking::RequestBuilder, Arc<AtomicU32>), TarnError> {
        let request_client = self.client_for_request_options(options)?;
        let builder = request_client
            .client
            .request(parse_http_method(method)?, url);

        Ok((
            apply_http_version(builder, self.config.http_version),
            request_client.redirect_count,
        ))
    }

    fn client_for_request_options(
        &self,
        options: RequestTransportOptions,
    ) -> Result<RequestClient, TarnError> {
        let redirect_count = Arc::new(AtomicU32::new(0));

        // Reuse the existing client only when redirects are disabled at the request
        // level and no client-level transport rebuild is required. Otherwise we
        // need a per-request client so redirect_count stays accurate.
        if can_reuse_client_for_request_options(options) {
            return Ok(RequestClient {
                client: self.inner.clone(),
                redirect_count,
            });
        }

        Ok(RequestClient {
            client: build_blocking_client(&self.config, options, Some(redirect_count.clone()))?,
            redirect_count,
        })
    }
}

struct RequestClient {
    client: reqwest::blocking::Client,
    redirect_count: Arc<AtomicU32>,
}

fn can_reuse_client_for_request_options(options: RequestTransportOptions) -> bool {
    options.follow_redirects.is_none()
        && options.connect_timeout_ms.is_none()
        && options.max_redirs.is_none()
        && options.timeout_ms.is_none()
}

fn shared_client() -> Result<&'static reqwest::blocking::Client, TarnError> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            build_blocking_client(
                &HttpTransportConfig::default(),
                RequestTransportOptions::default(),
                None,
            )
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| TarnError::Http(e.clone()))
}

fn parse_http_method(method: &str) -> Result<reqwest::Method, TarnError> {
    reqwest::Method::from_bytes(method.trim().as_bytes())
        .map_err(|e| TarnError::Http(format!("Invalid HTTP method '{}': {}", method, e)))
}

fn build_blocking_client(
    config: &HttpTransportConfig,
    options: RequestTransportOptions,
    redirect_count: Option<Arc<AtomicU32>>,
) -> Result<reqwest::blocking::Client, TarnError> {
    apply_transport_config(
        reqwest::blocking::Client::builder(),
        config,
        options,
        redirect_count,
    )?
    .build()
    .map_err(|e| TarnError::Http(format!("Failed to build HTTP client: {}", e)))
}

pub fn build_async_client(config: &HttpTransportConfig) -> Result<reqwest::Client, TarnError> {
    build_async_client_with_timeout(config, None)
}

pub fn build_async_client_with_timeout(
    config: &HttpTransportConfig,
    timeout: Option<std::time::Duration>,
) -> Result<reqwest::Client, TarnError> {
    let builder = apply_transport_config(
        reqwest::Client::builder(),
        config,
        RequestTransportOptions::default(),
        None,
    )?;
    let builder = if let Some(timeout) = timeout {
        builder.timeout(timeout)
    } else {
        builder
    };

    builder
        .build()
        .map_err(|e| TarnError::Http(format!("Failed to build HTTP client: {}", e)))
}

fn apply_proxy_to_builder<B>(builder: B, config: &HttpTransportConfig) -> Result<B, TarnError>
where
    B: TransportConfigurable,
{
    if let Some(proxy_url) = config.proxy.as_deref() {
        let mut proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| TarnError::Config(format!("Invalid proxy URL '{}': {}", proxy_url, e)))?;
        if let Some(no_proxy) = config.no_proxy.as_deref() {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(no_proxy));
        }
        Ok(builder.proxy(proxy))
    } else if config.no_proxy.is_some() {
        Ok(builder.no_proxy())
    } else {
        Ok(builder)
    }
}

fn apply_transport_config<B>(
    builder: B,
    config: &HttpTransportConfig,
    options: RequestTransportOptions,
    redirect_count: Option<Arc<AtomicU32>>,
) -> Result<B, TarnError>
where
    B: TransportConfigurable,
{
    let builder = apply_proxy_to_builder(
        builder.redirect(redirect_policy(options, redirect_count)),
        config,
    )?;
    let builder = if let Some(connect_timeout) = options.connect_timeout_ms {
        builder.connect_timeout(std::time::Duration::from_millis(connect_timeout))
    } else {
        builder
    };
    apply_tls_to_builder(builder, config)
}

fn redirect_policy(
    options: RequestTransportOptions,
    redirect_count: Option<Arc<AtomicU32>>,
) -> reqwest::redirect::Policy {
    if matches!(options.follow_redirects, Some(false)) {
        reqwest::redirect::Policy::none()
    } else if let Some(redirect_count) = redirect_count {
        let max_redirs = options.max_redirs.unwrap_or(10) as usize;
        reqwest::redirect::Policy::custom(move |attempt| {
            let followed = attempt.previous().len() as u32;
            redirect_count.store(followed, Ordering::Relaxed);
            if attempt.previous().len() > max_redirs {
                attempt.error("too many redirects")
            } else {
                attempt.follow()
            }
        })
    } else {
        reqwest::redirect::Policy::limited(options.max_redirs.unwrap_or(10) as usize)
    }
}

fn apply_http_version(
    builder: reqwest::blocking::RequestBuilder,
    version: Option<HttpVersionPreference>,
) -> reqwest::blocking::RequestBuilder {
    match version {
        Some(HttpVersionPreference::Http1_1) => builder.version(reqwest::Version::HTTP_11),
        Some(HttpVersionPreference::Http2) => builder.version(reqwest::Version::HTTP_2),
        None => builder,
    }
}

fn apply_tls_to_builder<B>(mut builder: B, config: &HttpTransportConfig) -> Result<B, TarnError>
where
    B: TransportConfigurable,
{
    if let Some(cacert_path) = config.cacert.as_deref() {
        let pem = read_transport_file(cacert_path, "cacert")?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
            TarnError::Config(format!(
                "Invalid CA certificate bundle '{}': {}",
                cacert_path, e
            ))
        })?;
        if certs.is_empty() {
            return Err(TarnError::Config(format!(
                "Invalid CA certificate bundle '{}': no PEM certificates found",
                cacert_path
            )));
        }

        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }

    match (config.cert.as_deref(), config.key.as_deref()) {
        (Some(cert_path), Some(key_path)) => {
            let cert_pem = read_transport_file(cert_path, "cert")?;
            let key_pem = read_transport_file(key_path, "key")?;
            let identity = identity_from_cert_and_key(&cert_pem, &key_pem, cert_path, key_path)?;
            builder = builder.identity(identity);
        }
        (Some(cert_path), None) => {
            let cert_pem = read_transport_file(cert_path, "cert")?;
            let identity = reqwest::Identity::from_pem(&cert_pem).map_err(|e| {
                TarnError::Config(format!(
                    "Invalid client identity PEM '{}': {}",
                    cert_path, e
                ))
            })?;
            builder = builder.identity(identity);
        }
        (None, Some(_)) => {
            return Err(TarnError::Config(
                "TLS key requires a matching client certificate (`cert`)".into(),
            ));
        }
        (None, None) => {}
    }

    if config.insecure {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }

    Ok(builder)
}

fn identity_from_cert_and_key(
    cert_pem: &[u8],
    key_pem: &[u8],
    cert_path: &str,
    key_path: &str,
) -> Result<reqwest::Identity, TarnError> {
    let mut combined = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
    combined.extend_from_slice(cert_pem);
    if !combined.ends_with(b"\n") {
        combined.push(b'\n');
    }
    combined.extend_from_slice(key_pem);

    reqwest::Identity::from_pem(&combined).map_err(|e| {
        TarnError::Config(format!(
            "Invalid client certificate/key pair ('{}', '{}'): {}",
            cert_path, key_path, e
        ))
    })
}

fn read_transport_file(path: &str, kind: &str) -> Result<Vec<u8>, TarnError> {
    fs::read(path)
        .map_err(|e| TarnError::Config(format!("Failed to read {} file '{}': {}", kind, path, e)))
}

trait TransportConfigurable: Sized {
    fn proxy(self, proxy: reqwest::Proxy) -> Self;
    fn no_proxy(self) -> Self;
    fn redirect(self, policy: reqwest::redirect::Policy) -> Self;
    fn connect_timeout(self, timeout: std::time::Duration) -> Self;
    fn add_root_certificate(self, cert: reqwest::Certificate) -> Self;
    fn identity(self, identity: reqwest::Identity) -> Self;
    fn danger_accept_invalid_certs(self, accept_invalid_certs: bool) -> Self;
    fn danger_accept_invalid_hostnames(self, accept_invalid_hostnames: bool) -> Self;
}

impl TransportConfigurable for reqwest::blocking::ClientBuilder {
    fn proxy(self, proxy: reqwest::Proxy) -> Self {
        self.proxy(proxy)
    }

    fn no_proxy(self) -> Self {
        self.no_proxy()
    }

    fn redirect(self, policy: reqwest::redirect::Policy) -> Self {
        self.redirect(policy)
    }

    fn connect_timeout(self, timeout: std::time::Duration) -> Self {
        self.connect_timeout(timeout)
    }

    fn add_root_certificate(self, cert: reqwest::Certificate) -> Self {
        self.add_root_certificate(cert)
    }

    fn identity(self, identity: reqwest::Identity) -> Self {
        self.identity(identity)
    }

    fn danger_accept_invalid_certs(self, accept_invalid_certs: bool) -> Self {
        self.danger_accept_invalid_certs(accept_invalid_certs)
    }

    fn danger_accept_invalid_hostnames(self, accept_invalid_hostnames: bool) -> Self {
        self.danger_accept_invalid_hostnames(accept_invalid_hostnames)
    }
}

impl TransportConfigurable for reqwest::ClientBuilder {
    fn proxy(self, proxy: reqwest::Proxy) -> Self {
        self.proxy(proxy)
    }

    fn no_proxy(self) -> Self {
        self.no_proxy()
    }

    fn redirect(self, policy: reqwest::redirect::Policy) -> Self {
        self.redirect(policy)
    }

    fn connect_timeout(self, timeout: std::time::Duration) -> Self {
        self.connect_timeout(timeout)
    }

    fn add_root_certificate(self, cert: reqwest::Certificate) -> Self {
        self.add_root_certificate(cert)
    }

    fn identity(self, identity: reqwest::Identity) -> Self {
        self.identity(identity)
    }

    fn danger_accept_invalid_certs(self, accept_invalid_certs: bool) -> Self {
        self.danger_accept_invalid_certs(accept_invalid_certs)
    }

    fn danger_accept_invalid_hostnames(self, accept_invalid_hostnames: bool) -> Self {
        self.danger_accept_invalid_hostnames(accept_invalid_hostnames)
    }
}

/// Execute an HTTP request and return the response.
pub fn execute_request(
    client: &HttpClient,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&serde_json::Value>,
    options: RequestTransportOptions,
) -> Result<HttpResponse, TarnError> {
    let (mut builder, redirect_count) = client.request(method, url, options)?;

    // Apply timeout
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }

    // Apply headers
    for (key, value) in headers {
        builder = builder.header(key, value);
    }

    // Apply body
    if let Some(body_value) = body {
        builder = builder.json(body_value);
    }

    let start = Instant::now();
    let response = builder.send().map_err(|e| enhance_http_error(url, &e))?;
    let ttfb_ms = start.elapsed().as_millis() as u64;

    parse_response(response, ttfb_ms, redirect_count.load(Ordering::Relaxed))
}

/// Encode a URL-encoded form body.
pub fn encode_form_body(form: &IndexMap<String, String>) -> Result<String, TarnError> {
    serde_urlencoded::to_string(form)
        .map_err(|e| TarnError::Http(format!("Failed to encode form body: {}", e)))
}

/// Execute an HTTP request with application/x-www-form-urlencoded data.
pub fn execute_form_request(
    client: &HttpClient,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    form: &IndexMap<String, String>,
    options: RequestTransportOptions,
) -> Result<HttpResponse, TarnError> {
    let (mut builder, redirect_count) = client.request(method, url, options)?;

    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }

    for (key, value) in headers {
        builder = builder.header(key, value);
    }

    if !headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("content-type"))
    {
        builder = builder.header("Content-Type", "application/x-www-form-urlencoded");
    }

    builder = builder.body(encode_form_body(form)?);

    let start = Instant::now();
    let response = builder.send().map_err(|e| enhance_http_error(url, &e))?;
    let ttfb_ms = start.elapsed().as_millis() as u64;

    parse_response(response, ttfb_ms, redirect_count.load(Ordering::Relaxed))
}

/// Execute an HTTP request with multipart form data.
pub fn execute_multipart_request(
    client: &HttpClient,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    multipart: &MultipartBody,
    options: RequestTransportOptions,
    base_dir: &Path,
) -> Result<HttpResponse, TarnError> {
    let (mut builder, redirect_count) = client.multipart_request(method, url, options)?;

    // Apply timeout
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }

    // Apply headers (except Content-Type — reqwest sets it for multipart)
    for (key, value) in headers {
        if !key.eq_ignore_ascii_case("content-type") {
            builder = builder.header(key, value);
        }
    }

    // Build multipart form
    let mut form = reqwest::blocking::multipart::Form::new();

    for field in &multipart.fields {
        form = form.text(field.name.clone(), field.value.clone());
    }

    for file in &multipart.files {
        let file_path = base_dir.join(&file.path);
        let mut part = reqwest::blocking::multipart::Part::file(&file_path).map_err(|e| {
            TarnError::Http(format!(
                "Failed to read file '{}': {}",
                file_path.display(),
                e
            ))
        })?;

        if let Some(ref ct) = file.content_type {
            part = part
                .mime_str(ct)
                .map_err(|e| TarnError::Http(format!("Invalid content type '{}': {}", ct, e)))?;
        }

        if let Some(ref filename) = file.filename {
            part = part.file_name(filename.clone());
        }

        form = form.part(file.name.clone(), part);
    }

    builder = builder.multipart(form);

    let start = Instant::now();
    let response = builder.send().map_err(|e| enhance_http_error(url, &e))?;
    let ttfb_ms = start.elapsed().as_millis() as u64;

    parse_response(response, ttfb_ms, redirect_count.load(Ordering::Relaxed))
}

/// Parse a reqwest response into our HttpResponse.
fn parse_response(
    response: reqwest::blocking::Response,
    ttfb_ms: u64,
    redirect_count: u32,
) -> Result<HttpResponse, TarnError> {
    let final_url = response.url().to_string();
    let status = response.status().as_u16();

    let mut response_headers = HashMap::new();
    let mut raw_headers = Vec::new();
    for (name, value) in response.headers() {
        if let Ok(v) = value.to_str() {
            let name_str = name.as_str().to_string();
            let value_str = v.to_string();
            raw_headers.push((name_str.clone(), value_str.clone()));
            // HashMap keeps last value for duplicate keys
            response_headers.insert(name_str, value_str);
        }
    }

    let body_start = Instant::now();
    let body_bytes = response
        .bytes()
        .map_err(|e| TarnError::Http(format!("Failed to read response body: {}", e)))?;
    let body_read_ms = body_start.elapsed().as_millis() as u64;
    let body_bytes = body_bytes.to_vec();
    let total_ms = ttfb_ms.saturating_add(body_read_ms);

    let body: serde_json::Value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        let body_text = String::from_utf8_lossy(&body_bytes).into_owned();
        serde_json::from_str(&body_text).unwrap_or(serde_json::Value::String(body_text))
    };

    Ok(HttpResponse {
        status,
        url: final_url,
        redirect_count,
        headers: response_headers,
        raw_headers,
        body_bytes,
        body,
        duration_ms: total_ms,
        timings: ResponseTimings {
            total_ms,
            ttfb_ms,
            body_read_ms,
            connect_ms: None,
            tls_ms: None,
        },
    })
}

/// Produce actionable error messages from reqwest errors.
fn enhance_http_error(url: &str, err: &reqwest::Error) -> TarnError {
    let details = error_chain_text(err);
    let lower = details.to_ascii_lowercase();

    if lower.contains("certificate")
        || lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("unknown issuer")
        || lower.contains("bad certificate")
    {
        TarnError::Http(format!(
            "TLS verification failed for {}. Check the server certificate or trust settings. ({})",
            url, details
        ))
    } else if err.is_connect() {
        TarnError::Http(format!(
            "Connection refused to {} — is the server running? ({})",
            url, details
        ))
    } else if err.is_timeout() {
        TarnError::Http(format!(
            "Request to {} timed out. Consider increasing the step timeout.",
            url
        ))
    } else if err.is_redirect() {
        TarnError::Http(format!(
            "Too many redirects for {}. Check the URL or server configuration.",
            url
        ))
    } else {
        TarnError::Http(format!("Request to {} failed: {}", url, details))
    }
}

fn error_chain_text(err: &reqwest::Error) -> String {
    use std::error::Error as _;

    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(next) = source {
        parts.push(next.to_string());
        source = next.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn default_client() -> HttpClient {
        HttpClient::new(&HttpTransportConfig::default()).unwrap()
    }

    fn write_temp_file(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path.display().to_string()
    }

    #[test]
    fn custom_method_is_accepted() {
        let method = parse_http_method("PROPFIND").unwrap();
        assert_eq!(method.as_str(), "PROPFIND");
    }

    #[test]
    fn invalid_method_returns_error() {
        let err = parse_http_method("BAD METHOD").unwrap_err().to_string();
        assert!(err.contains("Invalid HTTP method"));
    }

    #[test]
    fn reuse_decision_only_applies_when_no_overrides_exist() {
        // Default options → reuse shared client
        assert!(can_reuse_client_for_request_options(
            RequestTransportOptions::default()
        ));
        // Any transport override → build new client
        assert!(!can_reuse_client_for_request_options(
            RequestTransportOptions {
                follow_redirects: Some(false),
                ..RequestTransportOptions::default()
            }
        ));
        assert!(!can_reuse_client_for_request_options(
            RequestTransportOptions {
                follow_redirects: Some(true),
                ..RequestTransportOptions::default()
            }
        ));
        assert!(!can_reuse_client_for_request_options(
            RequestTransportOptions {
                connect_timeout_ms: Some(250),
                ..RequestTransportOptions::default()
            }
        ));
        assert!(!can_reuse_client_for_request_options(
            RequestTransportOptions {
                timeout_ms: Some(1000),
                ..RequestTransportOptions::default()
            }
        ));
        assert!(!can_reuse_client_for_request_options(
            RequestTransportOptions {
                max_redirs: Some(5),
                ..RequestTransportOptions::default()
            }
        ));
    }

    #[test]
    fn connection_refused_returns_actionable_error() {
        let client = default_client();
        let result = execute_request(
            &client,
            "GET",
            "http://127.0.0.1:1",
            &HashMap::new(),
            None,
            RequestTransportOptions {
                timeout_ms: Some(1000),
                ..RequestTransportOptions::default()
            },
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TarnError::Http(_)));
        let msg = err.to_string();
        // On Windows, connecting to a closed port may timeout instead of
        // returning connection refused due to OS-level TCP differences.
        assert!(
            msg.contains("Connection refused")
                || msg.contains("is the server running")
                || msg.contains("timed out"),
            "Expected actionable error, got: {}",
            msg
        );
    }

    #[test]
    fn timeout_returns_actionable_error() {
        // Use a very short timeout against a non-routable address
        let client = default_client();
        let result = execute_request(
            &client,
            "GET",
            "http://192.0.2.1:1", // TEST-NET, guaranteed non-routable
            &HashMap::new(),
            None,
            RequestTransportOptions {
                timeout_ms: Some(100),
                ..RequestTransportOptions::default()
            },
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        // Depending on OS, this might be a timeout or connection error
        assert!(
            msg.contains("timed out") || msg.contains("Connection refused"),
            "Expected timeout or connection error, got: {}",
            msg
        );
    }

    #[test]
    fn multipart_allows_custom_method_tokens() {
        let client = default_client();
        let mp = MultipartBody {
            fields: vec![],
            files: vec![],
        };
        let result = execute_multipart_request(
            &client,
            "PURGE",
            "http://localhost:1",
            &HashMap::new(),
            &mp,
            RequestTransportOptions::default(),
            Path::new("."),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Connection refused") || err.contains("is the server running"));
    }

    #[test]
    fn form_body_is_urlencoded() {
        let form = IndexMap::from([
            ("email".to_string(), "user@test.com".to_string()),
            ("redirect".to_string(), "/a path".to_string()),
        ]);

        let encoded = encode_form_body(&form).unwrap();

        assert_eq!(encoded, "email=user%40test.com&redirect=%2Fa+path");
    }

    #[test]
    fn invalid_proxy_url_returns_config_error() {
        let result = HttpClient::new(&HttpTransportConfig {
            proxy: Some("http://".into()),
            ..HttpTransportConfig::default()
        });

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TarnError::Config(_)));
    }

    #[test]
    fn key_without_cert_returns_config_error() {
        let result = HttpClient::new(&HttpTransportConfig {
            key: Some("client-key.pem".into()),
            ..HttpTransportConfig::default()
        });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("TLS key requires"));
    }

    #[test]
    fn invalid_cacert_returns_config_error() {
        let dir = TempDir::new().unwrap();
        let cacert = write_temp_file(&dir, "ca.pem", b"not pem");

        let result = HttpClient::new(&HttpTransportConfig {
            cacert: Some(cacert),
            ..HttpTransportConfig::default()
        });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid CA certificate bundle"));
    }

    #[test]
    fn valid_client_cert_and_key_build_identity() {
        let dir = TempDir::new().unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_path = write_temp_file(&dir, "client.pem", cert.cert.pem().as_bytes());
        let key_path = write_temp_file(
            &dir,
            "client-key.pem",
            cert.key_pair.serialize_pem().as_bytes(),
        );

        let result = HttpClient::new(&HttpTransportConfig {
            cert: Some(cert_path),
            key: Some(key_path),
            ..HttpTransportConfig::default()
        });

        assert!(result.is_ok());
    }
}
