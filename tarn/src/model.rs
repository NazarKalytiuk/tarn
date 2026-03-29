use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::HashMap;

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

    /// Cookie handling mode: "auto" (default) or "off"
    pub cookies: Option<String>,
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

    /// Delay before executing this step (e.g., "500ms", "2s")
    pub delay: Option<String>,

    /// Polling configuration: re-execute until condition is met
    pub poll: Option<PollConfig>,

    /// Lua script to run after HTTP response for custom validation
    pub script: Option<String>,
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

/// Extended capture specification supporting headers and regex extraction.
#[derive(Debug, Deserialize, Clone)]
pub struct ExtendedCapture {
    /// Capture from a response header (case-insensitive lookup)
    pub header: Option<String>,
    /// Capture from body via JSONPath (explicit form)
    pub jsonpath: Option<String>,
    /// Optional regex to extract a sub-match (capture group 1)
    pub regex: Option<String>,
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

    /// Request body — can be any JSON-compatible value
    pub body: Option<serde_json::Value>,

    /// GraphQL query (syntactic sugar; translates to JSON POST body)
    pub graphql: Option<GraphqlRequest>,

    /// Multipart form data for file uploads
    pub multipart: Option<MultipartBody>,
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

    /// Default timeout in milliseconds
    pub timeout: Option<u64>,

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

    /// Header assertions
    pub headers: Option<HashMap<String, String>>,

    /// Body assertions via JSONPath
    pub body: Option<IndexMap<String, serde_yaml::Value>>,
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
        assert!(matches!(assertions.status, Some(StatusAssertion::Exact(201))));
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
                assert_eq!(ext.regex.as_deref(), Some("session_token=([^;]+)"));
            }
            other => panic!("Expected Extended capture, got {:?}", other),
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
        assert_eq!(tf.cookies.as_deref(), Some("off"));
    }
}
