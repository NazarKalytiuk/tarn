use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Source location pointing at a YAML node (a step's `name:` key or an
/// assertion operator key). All fields are 1-based so they line up with
/// what editors and JSON reports already use elsewhere.
///
/// The field name and shape are fixed by the public JSON report schema and
/// consumed by the VS Code extension via `schemaGuards.ts`; do not rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    /// Absolute path of the source file (matches `FileResult.file`).
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
}

/// Runtime HTTP transport settings shared by run and bench commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpTransportConfig {
    pub proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub cacert: Option<String>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub insecure: bool,
    pub http_version: Option<HttpVersionPreference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersionPreference {
    Http1_1,
    Http2,
}

impl HttpTransportConfig {
    /// Merge project defaults with CLI overrides. CLI wins when provided.
    pub fn merge(project: &Self, cli: &Self) -> Self {
        Self {
            proxy: cli.proxy.clone().or_else(|| project.proxy.clone()),
            no_proxy: cli.no_proxy.clone().or_else(|| project.no_proxy.clone()),
            cacert: cli.cacert.clone().or_else(|| project.cacert.clone()),
            cert: cli.cert.clone().or_else(|| project.cert.clone()),
            key: cli.key.clone().or_else(|| project.key.clone()),
            insecure: cli.insecure || project.insecure,
            http_version: cli.http_version.or(project.http_version),
        }
    }

    pub fn has_custom_transport(&self) -> bool {
        self.proxy.is_some()
            || self.no_proxy.is_some()
            || self.cacert.is_some()
            || self.cert.is_some()
            || self.key.is_some()
            || self.insecure
            || self.http_version.is_some()
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct RedactionConfig {
    #[serde(default = "default_redacted_headers")]
    pub headers: Vec<String>,
    #[serde(default = "default_redaction_replacement")]
    pub replacement: String,
    #[serde(default, rename = "env")]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub captures: Vec<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            headers: default_redacted_headers(),
            replacement: default_redaction_replacement(),
            env_vars: Vec::new(),
            captures: Vec::new(),
        }
    }
}

impl RedactionConfig {
    /// Append extra header names to the effective redaction list without
    /// removing any existing entries. All names are normalized to lowercase
    /// so header matching stays case-insensitive, and duplicates (by
    /// lowercase comparison) are skipped so the list stays tidy.
    ///
    /// This is the single merge point used by the `--redact-header` CLI
    /// flag: callers can widen an already-resolved `RedactionConfig`
    /// (from defaults + `tarn.config.yaml` + test file) without mutating
    /// any persisted configuration.
    pub fn merge_headers<I, S>(&mut self, extra: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for name in extra {
            let trimmed = name.as_ref().trim();
            if trimmed.is_empty() {
                continue;
            }
            let normalized = trimmed.to_ascii_lowercase();
            if !self
                .headers
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&normalized))
            {
                self.headers.push(normalized);
            }
        }
    }
}

fn default_redacted_headers() -> Vec<String> {
    vec![
        "authorization".into(),
        "cookie".into(),
        "set-cookie".into(),
        "x-api-key".into(),
        "x-auth-token".into(),
    ]
}

fn default_redaction_replacement() -> String {
    "***".into()
}

/// Top-level test file structure matching .tarn.yaml format.
///
/// Supports two modes:
/// 1. Simple (flat steps): `steps:` at the top level
/// 2. Full (named tests): `tests:` map with named test groups
#[derive(Debug, Deserialize, Clone)]
pub struct TestFile {
    /// Schema version (optional, defaults to "1")
    pub version: Option<String>,

    /// Human-readable name for this test file
    pub name: String,

    /// Optional description
    pub description: Option<String>,

    /// Tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,

    /// Inline environment variables with defaults
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Report-time redaction settings for sensitive headers
    #[serde(alias = "redact")]
    pub redaction: Option<RedactionConfig>,

    /// Default headers/settings applied to every request
    pub defaults: Option<Defaults>,

    /// Setup steps run once before all tests
    #[serde(default)]
    pub setup: Vec<Step>,

    /// Teardown steps run once after all tests (even on failure)
    #[serde(default)]
    pub teardown: Vec<Step>,

    /// Named test groups (full format)
    #[serde(default)]
    pub tests: IndexMap<String, TestGroup>,

    /// Flat steps (simple format — mutually exclusive with `tests`)
    #[serde(default)]
    pub steps: Vec<Step>,

    /// Cookie handling mode: "auto" (default), "off", or "per-test"
    #[serde(default)]
    pub cookies: Option<CookieMode>,
}

/// File-level cookie handling mode.
///
/// - `Auto` (default) — single file-scoped jar shared across setup, tests, teardown.
/// - `Off` — cookies disabled entirely for the file.
/// - `PerTest` — the default jar is cleared between named tests so subset runs
///   and flaky suites never see session state from a prior test. Setup and
///   teardown still share the file-level jar. Named jars are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CookieMode {
    #[default]
    Auto,
    Off,
    PerTest,
}

impl<'de> Deserialize<'de> for CookieMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "auto" => Ok(CookieMode::Auto),
            "off" => Ok(CookieMode::Off),
            "per-test" => Ok(CookieMode::PerTest),
            other => Err(serde::de::Error::custom(format!(
                "cookies must be \"auto\", \"off\", or \"per-test\" (got \"{}\")",
                other
            ))),
        }
    }
}

/// A named group of test steps.
#[derive(Debug, Deserialize, Clone)]
pub struct TestGroup {
    pub description: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A single test step: one HTTP request + optional capture + assertions.
#[derive(Debug, Deserialize, Clone)]
pub struct Step {
    pub name: String,
    pub request: Request,

    /// Captures from the response (JSONPath or header with optional regex)
    #[serde(default)]
    pub capture: HashMap<String, CaptureSpec>,

    /// Assertions on the response
    #[serde(rename = "assert")]
    pub assertions: Option<Assertion>,

    /// Number of retries on failure (0 = no retries)
    #[serde(default)]
    pub retries: Option<u32>,

    /// Step-level timeout in milliseconds (overrides defaults)
    pub timeout: Option<u64>,

    /// Step-level connect timeout in milliseconds (overrides defaults)
    #[serde(alias = "connect-timeout")]
    pub connect_timeout: Option<u64>,

    /// Whether this step should follow redirects (overrides defaults)
    #[serde(alias = "follow-redirects")]
    pub follow_redirects: Option<bool>,

    /// Maximum redirects to follow for this step (overrides defaults)
    #[serde(alias = "max-redirs")]
    pub max_redirs: Option<u32>,

    /// Delay before executing this step (e.g., "500ms", "2s")
    pub delay: Option<String>,

    /// Polling configuration: re-execute until condition is met
    pub poll: Option<PollConfig>,

    /// Lua script to run after HTTP response for custom validation
    pub script: Option<String>,

    /// Per-step cookie control:
    /// - omitted or `true`: use the default cookie jar
    /// - `false`: skip cookies entirely for this step
    /// - `"jar-name"`: use a named cookie jar (for multi-user scenarios)
    pub cookies: Option<StepCookies>,

    /// Source location of the step's `name:` node in the original YAML.
    /// Populated by `parser::parse_str` after deserialization so downstream
    /// consumers can anchor runtime results on the exact source range.
    #[serde(skip)]
    pub location: Option<Location>,

    /// Source locations of individual assertion keys, indexed by the same
    /// string used in `AssertionResult::assertion` (e.g. `"status"`,
    /// `"duration"`, `"headers.content-type"`, `"body $.name"`).
    /// Populated by `parser::parse_str` after deserialization.
    #[serde(skip)]
    pub assertion_locations: HashMap<String, Location>,
}

/// Step-level cookie control.
#[derive(Debug, Clone, PartialEq)]
pub enum StepCookies {
    /// Enable (true) or disable (false) the default cookie jar.
    Enabled(bool),
    /// Use a named cookie jar.
    Named(String),
}

impl<'de> Deserialize<'de> for StepCookies {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        match value {
            serde_yaml::Value::Bool(b) => Ok(StepCookies::Enabled(b)),
            serde_yaml::Value::String(s) => Ok(StepCookies::Named(s)),
            _ => Err(serde::de::Error::custom(
                "cookies must be true, false, or a jar name string",
            )),
        }
    }
}

/// Capture specification: either a simple JSONPath string or an extended capture.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CaptureSpec {
    /// Simple JSONPath: "$.token"
    JsonPath(String),
    /// Extended capture: from header or JSONPath with optional regex
    Extended(ExtendedCapture),
}

/// Extended capture specification supporting multiple response sources with optional regex extraction.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ExtendedCapture {
    /// Capture from a response header (case-insensitive lookup)
    pub header: Option<String>,
    /// Capture from a response cookie by cookie name
    pub cookie: Option<String>,
    /// Capture from body via JSONPath (explicit form)
    pub jsonpath: Option<String>,
    /// Capture from the whole response body string
    pub body: Option<bool>,
    /// Capture from the HTTP response status code
    pub status: Option<bool>,
    /// Capture from the final response URL after redirects
    pub url: Option<bool>,
    /// Optional regex to extract a sub-match (capture group 1)
    pub regex: Option<String>,
    /// Predicate filter for arrays: when `jsonpath` yields an array,
    /// keep only elements whose every field equals (or satisfies the
    /// nested operator map for) the corresponding entry in this
    /// mapping. Combined with `first`/`last`/`count` transforms this
    /// replaces brittle `$[0]` captures from shared list endpoints with
    /// identity-based selection (NAZ-341).
    #[serde(default, rename = "where")]
    pub where_predicate: Option<serde_yaml::Value>,
}

/// Polling configuration: re-execute step until a condition is met.
#[derive(Debug, Deserialize, Clone)]
pub struct PollConfig {
    /// Assertions that must pass for polling to stop
    pub until: Assertion,
    /// Time between attempts (e.g., "2s", "500ms")
    pub interval: String,
    /// Maximum number of polling attempts
    pub max_attempts: u32,
}

/// HTTP request definition.
#[derive(Debug, Deserialize, Clone)]
pub struct Request {
    pub method: String,
    pub url: String,

    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Optional auth helper. Explicit Authorization headers still win.
    pub auth: Option<AuthConfig>,

    /// Request body — can be any JSON-compatible value
    pub body: Option<serde_json::Value>,

    /// URL-encoded form body sent as application/x-www-form-urlencoded
    #[serde(default)]
    pub form: Option<IndexMap<String, String>>,

    /// GraphQL query (syntactic sugar; translates to JSON POST body)
    pub graphql: Option<GraphqlRequest>,

    /// Multipart form data for file uploads
    pub multipart: Option<MultipartBody>,
}

/// First-class auth helper for common bearer/basic cases.
#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    /// Bearer token value (without the `Bearer ` prefix)
    pub bearer: Option<String>,
    /// Basic auth credentials
    pub basic: Option<BasicAuthConfig>,
}

/// Basic auth credentials.
#[derive(Debug, Deserialize, Clone)]
pub struct BasicAuthConfig {
    pub username: String,
    pub password: String,
}

/// GraphQL query definition.
#[derive(Debug, Deserialize, Clone)]
pub struct GraphqlRequest {
    /// The GraphQL query or mutation string
    pub query: String,
    /// Variables to pass to the query
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
    /// Operation name (when query contains multiple operations)
    pub operation_name: Option<String>,
}

/// Multipart form data body for file uploads.
#[derive(Debug, Deserialize, Clone)]
pub struct MultipartBody {
    /// Text form fields
    #[serde(default)]
    pub fields: Vec<FormField>,
    /// File upload fields
    #[serde(default)]
    pub files: Vec<FileField>,
}

/// A text field in a multipart form.
#[derive(Debug, Deserialize, Clone)]
pub struct FormField {
    pub name: String,
    pub value: String,
}

/// A file field in a multipart form.
#[derive(Debug, Deserialize, Clone)]
pub struct FileField {
    /// Form field name
    pub name: String,
    /// Path to the file (relative to test file)
    pub path: String,
    /// MIME content type (e.g., "image/jpeg")
    pub content_type: Option<String>,
    /// Override filename sent in the form
    pub filename: Option<String>,
}

/// Default settings applied to every request in a file.
#[derive(Debug, Deserialize, Clone)]
pub struct Defaults {
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Default auth helper applied when a step does not set request.auth.
    pub auth: Option<AuthConfig>,

    /// Default timeout in milliseconds
    pub timeout: Option<u64>,

    /// Default connect timeout in milliseconds
    #[serde(alias = "connect-timeout")]
    pub connect_timeout: Option<u64>,

    /// Default redirect-following behavior
    #[serde(alias = "follow-redirects")]
    pub follow_redirects: Option<bool>,

    /// Default maximum redirects to follow
    #[serde(alias = "max-redirs")]
    pub max_redirs: Option<u32>,

    /// Default retries for all steps
    pub retries: Option<u32>,

    /// Default delay before each request (e.g., "100ms", "1s")
    pub delay: Option<String>,
}

/// Assertion block for a step.
#[derive(Debug, Deserialize, Clone)]
pub struct Assertion {
    /// Expected HTTP status code (exact, shorthand range, or complex)
    pub status: Option<StatusAssertion>,

    /// Response time assertion (e.g., "< 500ms")
    pub duration: Option<String>,

    /// Redirect assertions against the final response URL and redirect count
    pub redirect: Option<RedirectAssertion>,

    /// Header assertions
    pub headers: Option<HashMap<String, String>>,

    /// Body assertions via JSONPath
    pub body: Option<IndexMap<String, serde_yaml::Value>>,
}

/// Redirect assertions for a followed response chain.
#[derive(Debug, Deserialize, Clone)]
pub struct RedirectAssertion {
    /// Expected final URL after redirects
    pub url: Option<String>,
    /// Expected number of followed redirects
    pub count: Option<u32>,
}

/// Status code assertion: exact match, shorthand range, or complex spec.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum StatusAssertion {
    /// Exact match: `status: 200`
    Exact(u16),
    /// Shorthand range: `status: "2xx"`, `status: "4xx"`
    Shorthand(String),
    /// Complex: `status: { in: [200, 201] }` or `status: { gte: 400, lt: 500 }`
    Complex(StatusSpec),
}

/// Complex status code specification with ranges and sets.
#[derive(Debug, Deserialize, Clone)]
pub struct StatusSpec {
    /// Set of allowed status codes
    #[serde(rename = "in")]
    pub in_set: Option<Vec<u16>>,
    /// Greater than or equal
    pub gte: Option<u16>,
    /// Greater than
    pub gt: Option<u16>,
    /// Less than or equal
    pub lte: Option<u16>,
    /// Less than
    pub lt: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_minimal_test_file() {
        let yaml = r#"
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "http://localhost:3000/health"
    assert:
      status: 200
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.name, "Health check");
        assert_eq!(tf.steps.len(), 1);
        assert_eq!(tf.steps[0].name, "GET /health");
        assert_eq!(tf.steps[0].request.method, "GET");
        assert_eq!(tf.steps[0].request.url, "http://localhost:3000/health");
        assert!(matches!(
            tf.steps[0].assertions.as_ref().unwrap().status,
            Some(StatusAssertion::Exact(200))
        ));
    }

    #[test]
    fn deserialize_full_test_file() {
        let yaml = r#"
version: "1"
name: "User CRUD"
description: "Tests CRUD lifecycle"
tags: [crud, users]
env:
  base_url: "http://localhost:3000"
defaults:
  headers:
    Content-Type: "application/json"
  timeout: 5000
setup:
  - name: Auth
    request:
      method: POST
      url: "http://localhost:3000/auth"
      body:
        email: "admin@test.com"
    capture:
      token: "$.token"
    assert:
      status: 200
teardown:
  - name: Cleanup
    request:
      method: POST
      url: "http://localhost:3000/cleanup"
tests:
  create_user:
    description: "Create a user"
    tags: [smoke]
    steps:
      - name: Create
        request:
          method: POST
          url: "http://localhost:3000/users"
          headers:
            Authorization: "Bearer token"
          body:
            name: "Jane"
        capture:
          user_id: "$.id"
        assert:
          status: 201
          duration: "< 500ms"
          headers:
            content-type: contains "application/json"
          body:
            "$.name": "Jane"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.version, Some("1".into()));
        assert_eq!(tf.name, "User CRUD");
        assert_eq!(tf.description, Some("Tests CRUD lifecycle".into()));
        assert_eq!(tf.tags, vec!["crud", "users"]);
        assert_eq!(tf.env.get("base_url").unwrap(), "http://localhost:3000");

        // Defaults
        let defaults = tf.defaults.as_ref().unwrap();
        assert_eq!(
            defaults.headers.get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(defaults.timeout, Some(5000));

        // Setup
        assert_eq!(tf.setup.len(), 1);
        assert_eq!(tf.setup[0].name, "Auth");
        assert!(matches!(
            tf.setup[0].capture.get("token"),
            Some(CaptureSpec::JsonPath(p)) if p == "$.token"
        ));

        // Teardown
        assert_eq!(tf.teardown.len(), 1);

        // Tests
        assert_eq!(tf.tests.len(), 1);
        let test = tf.tests.get("create_user").unwrap();
        assert_eq!(test.description, Some("Create a user".into()));
        assert_eq!(test.tags, vec!["smoke"]);
        assert_eq!(test.steps.len(), 1);

        let step = &test.steps[0];
        assert_eq!(step.name, "Create");
        assert_eq!(step.request.method, "POST");
        assert!(step.request.body.is_some());
        assert!(matches!(
            step.capture.get("user_id"),
            Some(CaptureSpec::JsonPath(p)) if p == "$.id"
        ));

        let assertions = step.assertions.as_ref().unwrap();
        assert!(matches!(
            assertions.status,
            Some(StatusAssertion::Exact(201))
        ));
        assert_eq!(assertions.duration, Some("< 500ms".into()));
        assert!(assertions.headers.is_some());
        assert!(assertions.body.is_some());
    }

    #[test]
    fn deserialize_step_without_assertions() {
        let yaml = r#"
name: Fire and forget
steps:
  - name: Trigger webhook
    request:
      method: POST
      url: "http://localhost:3000/webhook"
      body:
        event: "deploy"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.steps.len(), 1);
        assert!(tf.steps[0].assertions.is_none());
    }

    #[test]
    fn deserialize_redirect_assertion() {
        let yaml = r#"
name: Redirect assertions
steps:
  - name: Follow chain
    request:
      method: GET
      url: "http://localhost:3000/redirect"
    assert:
      redirect:
        url: "http://localhost:3000/final"
        count: 2
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let redirect = tf.steps[0]
            .assertions
            .as_ref()
            .and_then(|a| a.redirect.as_ref())
            .unwrap();
        assert_eq!(redirect.url.as_deref(), Some("http://localhost:3000/final"));
        assert_eq!(redirect.count, Some(2));
    }

    #[test]
    fn deserialize_empty_optional_fields() {
        let yaml = r#"
name: Minimal
steps:
  - name: Simple GET
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert!(tf.version.is_none());
        assert!(tf.description.is_none());
        assert!(tf.tags.is_empty());
        assert!(tf.env.is_empty());
        assert!(tf.defaults.is_none());
        assert!(tf.setup.is_empty());
        assert!(tf.teardown.is_empty());
        assert!(tf.tests.is_empty());
    }

    #[test]
    fn deserialize_request_with_headers_and_body() {
        let yaml = r#"
name: test
steps:
  - name: POST with JSON body
    request:
      method: POST
      url: "http://localhost:3000/users"
      headers:
        Authorization: "Bearer xyz"
        X-Custom: "hello"
      body:
        name: "Alice"
        tags: ["a", "b"]
        nested:
          key: "value"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let req = &tf.steps[0].request;
        assert_eq!(req.headers.get("Authorization").unwrap(), "Bearer xyz");
        assert_eq!(req.headers.get("X-Custom").unwrap(), "hello");

        let body = req.body.as_ref().unwrap();
        assert_eq!(body["name"], "Alice");
        assert_eq!(body["tags"][0], "a");
        assert_eq!(body["nested"]["key"], "value");
    }

    #[test]
    fn deserialize_request_with_auth_helper() {
        let yaml = r#"
name: auth
steps:
  - name: GET
    request:
      method: GET
      url: "http://localhost:3000/me"
      auth:
        bearer: "{{ env.token }}"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let auth = tf.steps[0].request.auth.as_ref().unwrap();
        assert_eq!(auth.bearer.as_deref(), Some("{{ env.token }}"));
        assert!(auth.basic.is_none());
    }

    #[test]
    fn deserialize_defaults_with_basic_auth_helper() {
        let yaml = r#"
name: auth
defaults:
  auth:
    basic:
      username: "demo"
      password: "{{ env.password }}"
steps:
  - name: GET
    request:
      method: GET
      url: "http://localhost:3000/me"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let auth = tf.defaults.as_ref().unwrap().auth.as_ref().unwrap();
        let basic = auth.basic.as_ref().unwrap();
        assert_eq!(basic.username, "demo");
        assert_eq!(basic.password, "{{ env.password }}");
    }

    #[test]
    fn tests_preserve_insertion_order() {
        let yaml = r#"
name: Order test
tests:
  third_test:
    steps:
      - name: step
        request:
          method: GET
          url: "http://localhost:3000"
  first_test:
    steps:
      - name: step
        request:
          method: GET
          url: "http://localhost:3000"
  second_test:
    steps:
      - name: step
        request:
          method: GET
          url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let keys: Vec<&String> = tf.tests.keys().collect();
        assert_eq!(keys, vec!["third_test", "first_test", "second_test"]);
    }

    #[test]
    fn deserialize_body_assertions_with_various_types() {
        let yaml = r#"
name: Assertion types
steps:
  - name: Check types
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status: 200
      body:
        "$.string": "hello"
        "$.number": 42
        "$.bool": true
        "$.null_field": null
        "$.complex":
          type: string
          contains: "sub"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let body = tf.steps[0]
            .assertions
            .as_ref()
            .unwrap()
            .body
            .as_ref()
            .unwrap();
        assert_eq!(body.len(), 5);
        assert!(body.contains_key("$.string"));
        assert!(body.contains_key("$.number"));
        assert!(body.contains_key("$.bool"));
        assert!(body.contains_key("$.null_field"));
        assert!(body.contains_key("$.complex"));
    }

    // --- New tests for extended captures ---

    #[test]
    fn deserialize_header_capture() {
        let yaml = r#"
name: Header capture test
steps:
  - name: Login
    request:
      method: POST
      url: "http://localhost:3000/login"
    capture:
      session_token:
        header: "set-cookie"
        regex: "session_token=([^;]+)"
      user_id: "$.id"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let cap = &tf.steps[0].capture;
        assert!(matches!(cap.get("user_id"), Some(CaptureSpec::JsonPath(p)) if p == "$.id"));
        match cap.get("session_token") {
            Some(CaptureSpec::Extended(ext)) => {
                assert_eq!(ext.header.as_deref(), Some("set-cookie"));
                assert_eq!(ext.cookie, None);
                assert_eq!(ext.body, None);
                assert_eq!(ext.status, None);
                assert_eq!(ext.url, None);
                assert_eq!(ext.regex.as_deref(), Some("session_token=([^;]+)"));
            }
            other => panic!("Expected Extended capture, got {:?}", other),
        }
    }

    #[test]
    fn deserialize_status_capture() {
        let yaml = r#"
name: Status capture test
steps:
  - name: Health
    request:
      method: GET
      url: "http://localhost:3000/health"
    capture:
      status_code:
        status: true
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let cap = &tf.steps[0].capture;
        match cap.get("status_code") {
            Some(CaptureSpec::Extended(ext)) => {
                assert_eq!(ext.header, None);
                assert_eq!(ext.cookie, None);
                assert_eq!(ext.jsonpath, None);
                assert_eq!(ext.body, None);
                assert_eq!(ext.status, Some(true));
                assert_eq!(ext.url, None);
                assert_eq!(ext.regex, None);
            }
            other => panic!("Expected Extended capture, got {:?}", other),
        }
    }

    #[test]
    fn deserialize_url_capture() {
        let yaml = r#"
name: URL capture test
steps:
  - name: Follow redirect
    request:
      method: GET
      url: "http://localhost:3000/redirect"
    capture:
      final_url:
        url: true
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let cap = &tf.steps[0].capture;
        match cap.get("final_url") {
            Some(CaptureSpec::Extended(ext)) => {
                assert_eq!(ext.header, None);
                assert_eq!(ext.cookie, None);
                assert_eq!(ext.jsonpath, None);
                assert_eq!(ext.body, None);
                assert_eq!(ext.status, None);
                assert_eq!(ext.url, Some(true));
                assert_eq!(ext.regex, None);
            }
            other => panic!("Expected Extended capture, got {:?}", other),
        }
    }

    #[test]
    fn deserialize_cookie_and_body_capture() {
        let yaml = r#"
name: Cookie capture test
steps:
  - name: Cookies
    request:
      method: GET
      url: "http://localhost:3000/cookies"
    capture:
      session_cookie:
        cookie: "session"
      body_word:
        body: true
        regex: "plain (text)"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let cap = &tf.steps[0].capture;
        match cap.get("session_cookie") {
            Some(CaptureSpec::Extended(ext)) => {
                assert_eq!(ext.header, None);
                assert_eq!(ext.cookie.as_deref(), Some("session"));
                assert_eq!(ext.jsonpath, None);
                assert_eq!(ext.body, None);
                assert_eq!(ext.status, None);
                assert_eq!(ext.url, None);
                assert_eq!(ext.regex, None);
            }
            other => panic!("Expected cookie Extended capture, got {:?}", other),
        }
        match cap.get("body_word") {
            Some(CaptureSpec::Extended(ext)) => {
                assert_eq!(ext.header, None);
                assert_eq!(ext.cookie, None);
                assert_eq!(ext.jsonpath, None);
                assert_eq!(ext.body, Some(true));
                assert_eq!(ext.status, None);
                assert_eq!(ext.url, None);
                assert_eq!(ext.regex.as_deref(), Some("plain (text)"));
            }
            other => panic!("Expected body Extended capture, got {:?}", other),
        }
    }

    // --- New tests for status assertion variants ---

    #[test]
    fn deserialize_status_shorthand() {
        let yaml = r#"
name: Status range
steps:
  - name: Check 2xx
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status: "2xx"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(
            tf.steps[0].assertions.as_ref().unwrap().status,
            Some(StatusAssertion::Shorthand(ref s)) if s == "2xx"
        ));
    }

    #[test]
    fn deserialize_status_complex_in() {
        let yaml = r#"
name: Status set
steps:
  - name: Check set
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status:
        in: [200, 201, 204]
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        match &tf.steps[0].assertions.as_ref().unwrap().status {
            Some(StatusAssertion::Complex(spec)) => {
                assert_eq!(spec.in_set.as_ref().unwrap(), &vec![200, 201, 204]);
            }
            other => panic!("Expected Complex status, got {:?}", other),
        }
    }

    #[test]
    fn deserialize_status_complex_range() {
        let yaml = r#"
name: Status range
steps:
  - name: Check 4xx range
    request:
      method: GET
      url: "http://localhost:3000"
    assert:
      status:
        gte: 400
        lt: 500
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        match &tf.steps[0].assertions.as_ref().unwrap().status {
            Some(StatusAssertion::Complex(spec)) => {
                assert_eq!(spec.gte, Some(400));
                assert_eq!(spec.lt, Some(500));
            }
            other => panic!("Expected Complex status, got {:?}", other),
        }
    }

    // --- Multipart ---

    #[test]
    fn deserialize_multipart_request() {
        let yaml = r#"
name: Upload test
steps:
  - name: Upload photo
    request:
      method: POST
      url: "http://localhost:3000/upload"
      multipart:
        fields:
          - name: "title"
            value: "My Photo"
        files:
          - name: "photo"
            path: "./fixtures/test.jpg"
            content_type: "image/jpeg"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let mp = tf.steps[0].request.multipart.as_ref().unwrap();
        assert_eq!(mp.fields.len(), 1);
        assert_eq!(mp.fields[0].name, "title");
        assert_eq!(mp.fields[0].value, "My Photo");
        assert_eq!(mp.files.len(), 1);
        assert_eq!(mp.files[0].name, "photo");
        assert_eq!(mp.files[0].path, "./fixtures/test.jpg");
        assert_eq!(mp.files[0].content_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn deserialize_form_request() {
        let yaml = r#"
name: Form test
steps:
  - name: Submit form
    request:
      method: POST
      url: "http://localhost:3000/login"
      form:
        email: "user@example.com"
        password: "secret"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let form = tf.steps[0].request.form.as_ref().unwrap();
        assert_eq!(
            form.get("email").map(String::as_str),
            Some("user@example.com")
        );
        assert_eq!(form.get("password").map(String::as_str), Some("secret"));
    }

    // --- Default delay ---

    #[test]
    fn deserialize_defaults_with_delay() {
        let yaml = r#"
name: Delay test
defaults:
  delay: "100ms"
  timeout: 5000
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            tf.defaults.as_ref().unwrap().delay.as_deref(),
            Some("100ms")
        );
    }

    // --- Cookies ---

    #[test]
    fn deserialize_step_cookies_false() {
        let yaml = r#"
name: Step cookies test
steps:
  - name: No cookies step
    cookies: false
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.steps[0].cookies, Some(StepCookies::Enabled(false)));
    }

    #[test]
    fn deserialize_step_cookies_true() {
        let yaml = r#"
name: Step cookies test
steps:
  - name: With cookies
    cookies: true
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.steps[0].cookies, Some(StepCookies::Enabled(true)));
    }

    #[test]
    fn deserialize_step_cookies_named_jar() {
        let yaml = r#"
name: Step cookies test
steps:
  - name: Admin step
    cookies: "admin"
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            tf.steps[0].cookies,
            Some(StepCookies::Named("admin".to_string()))
        );
    }

    #[test]
    fn deserialize_step_cookies_default_none() {
        let yaml = r#"
name: Step cookies test
steps:
  - name: Default step
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.steps[0].cookies, None);
    }

    #[test]
    fn deserialize_cookies_off() {
        let yaml = r#"
name: No cookies
cookies: "off"
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.cookies, Some(CookieMode::Off));
    }

    #[test]
    fn deserialize_cookies_auto() {
        let yaml = r#"
name: Auto cookies
cookies: "auto"
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.cookies, Some(CookieMode::Auto));
    }

    #[test]
    fn deserialize_cookies_per_test() {
        let yaml = r#"
name: Per-test cookies
cookies: "per-test"
tests:
  login:
    steps:
      - name: test
        request:
          method: GET
          url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.cookies, Some(CookieMode::PerTest));
    }

    #[test]
    fn deserialize_cookies_invalid_value_is_rejected() {
        let yaml = r#"
name: Bad cookies
cookies: "sometimes"
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let err = serde_yaml::from_str::<TestFile>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("per-test"),
            "error should mention the valid options, got: {err}"
        );
    }

    #[test]
    fn deserialize_cookies_default_is_none() {
        let yaml = r#"
name: Default cookies
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;
        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tf.cookies, None);
    }

    #[test]
    fn deserialize_redaction_config() {
        let yaml = r#"
name: Redaction config
redaction:
  headers:
    - authorization
    - x-session-token
  env:
    - api_token
  captures:
    - session
  replacement: "[redacted]"
steps:
  - name: test
    request:
      method: GET
      url: "http://localhost:3000"
"#;

        let tf: TestFile = serde_yaml::from_str(yaml).unwrap();
        let redaction = tf.redaction.unwrap();
        assert_eq!(redaction.headers, vec!["authorization", "x-session-token"]);
        assert_eq!(redaction.env_vars, vec!["api_token"]);
        assert_eq!(redaction.captures, vec!["session"]);
        assert_eq!(redaction.replacement, "[redacted]");
    }

    #[test]
    fn merge_headers_widens_list_case_insensitively() {
        let mut redaction = RedactionConfig {
            headers: vec!["authorization".into()],
            ..RedactionConfig::default()
        };
        redaction.merge_headers(["X-Custom-Token", "x-debug"]);
        assert_eq!(
            redaction.headers,
            vec!["authorization", "x-custom-token", "x-debug"]
        );
    }

    #[test]
    fn merge_headers_skips_duplicates_ignoring_case() {
        let mut redaction = RedactionConfig {
            headers: vec!["authorization".into(), "x-api-key".into()],
            ..RedactionConfig::default()
        };
        redaction.merge_headers(["Authorization", "X-API-KEY", "x-new"]);
        assert_eq!(
            redaction.headers,
            vec!["authorization", "x-api-key", "x-new"]
        );
    }

    #[test]
    fn merge_headers_trims_and_drops_empty_entries() {
        let mut redaction = RedactionConfig::default();
        let baseline_len = redaction.headers.len();
        redaction.merge_headers(["", "   ", "  X-Trim  "]);
        assert_eq!(redaction.headers.len(), baseline_len + 1);
        assert!(redaction.headers.iter().any(|h| h == "x-trim"));
    }

    #[test]
    fn merge_headers_never_narrows_existing_list() {
        let mut redaction = RedactionConfig {
            headers: vec!["authorization".into(), "cookie".into()],
            ..RedactionConfig::default()
        };
        // Empty merge must not drop anything.
        redaction.merge_headers(std::iter::empty::<String>());
        assert_eq!(redaction.headers, vec!["authorization", "cookie"]);
    }
}
