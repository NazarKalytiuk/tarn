use crate::error::TarnError;
use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Cookie jar with basic RFC-aware domain/path/secure/expiry handling.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
    pub expiry: Option<DateTime<Utc>>,
    pub host_only: bool,
}

#[derive(Debug, Default)]
struct ParsedCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    same_site: Option<SameSite>,
    max_age: Option<i64>,
    expiry: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CookieJarFile {
    schema_version: u8,
    #[serde(default)]
    jars: HashMap<String, Vec<Cookie>>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Parse Set-Cookie headers from the response URL and store matching cookies.
    pub fn capture_from_response(&mut self, response_url: &str, raw_headers: &[(String, String)]) {
        let Ok(url) = Url::parse(response_url) else {
            return;
        };
        let Some(host) = normalized_host(&url) else {
            return;
        };
        let default_path = default_cookie_path(url.path());

        for (name, value) in raw_headers {
            if !name.eq_ignore_ascii_case("set-cookie") {
                continue;
            }

            let Some(parsed) = parse_set_cookie(value) else {
                continue;
            };

            let (domain, host_only) = match parsed.domain.as_deref() {
                Some(domain) => {
                    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
                    if domain.is_empty() || !domain_matches(&host, &domain) {
                        continue;
                    }
                    (domain, false)
                }
                None => (host.clone(), true),
            };

            let path = parsed
                .path
                .filter(|path| path.starts_with('/'))
                .unwrap_or_else(|| default_path.clone());
            let expiry = parsed.max_age.map(max_age_deadline).or(parsed.expiry);

            if parsed.max_age.is_some_and(|age| age <= 0)
                || expiry.is_some_and(|deadline| deadline <= Utc::now())
            {
                self.remove_cookie(&parsed.name, &domain, &path);
                continue;
            }

            self.insert_cookie(Cookie {
                name: parsed.name,
                value: parsed.value,
                domain,
                path,
                secure: parsed.secure,
                http_only: parsed.http_only,
                same_site: parsed.same_site,
                host_only,
                expiry,
            });
        }
    }

    /// Generate a Cookie header value for a specific request URL.
    pub fn cookie_header(&self, request_url: &str) -> Option<String> {
        let Ok(url) = Url::parse(request_url) else {
            return None;
        };
        let host = normalized_host(&url)?;
        let request_path = normalized_request_path(&url);
        let is_secure = url.scheme().eq_ignore_ascii_case("https");
        let now = Utc::now();

        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|cookie| !cookie.is_expired(now))
            .filter(|cookie| domain_match_for_request(cookie, &host))
            .filter(|cookie| path_matches(&request_path, &cookie.path))
            .filter(|cookie| !cookie.secure || is_secure)
            .collect();

        if matching.is_empty() {
            return None;
        }

        matching.sort_by(|left, right| {
            right
                .path
                .len()
                .cmp(&left.path.len())
                .then_with(|| left.name.cmp(&right.name))
        });

        Some(
            matching
                .into_iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    /// Check if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn insert_cookie(&mut self, cookie: Cookie) {
        let Some(cookie) = normalize_cookie(cookie) else {
            return;
        };

        if cookie.expiry.is_some_and(|expiry| expiry <= Utc::now()) {
            self.remove_cookie(&cookie.name, &cookie.domain, &cookie.path);
            return;
        }

        self.remove_cookie(&cookie.name, &cookie.domain, &cookie.path);
        self.cookies.push(cookie);
    }

    fn remove_cookie(&mut self, name: &str, domain: &str, path: &str) {
        self.cookies.retain(|cookie| {
            !(cookie.name == name && cookie.domain == domain && cookie.path == path)
        });
    }
}

pub fn load_named_jars(path: &Path) -> Result<HashMap<String, CookieJar>, TarnError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(path).map_err(|e| {
        TarnError::Config(format!(
            "Failed to read cookie jar file '{}': {}",
            path.display(),
            e
        ))
    })?;
    let file: CookieJarFile = serde_json::from_str(&content).map_err(|e| {
        TarnError::Config(format!(
            "Invalid cookie jar file '{}': {}",
            path.display(),
            e
        ))
    })?;

    if file.schema_version != 1 {
        return Err(TarnError::Config(format!(
            "Unsupported cookie jar schema version {} in '{}'",
            file.schema_version,
            path.display()
        )));
    }

    let mut jars = HashMap::new();
    for (name, cookies) in file.jars {
        let mut jar = CookieJar::new();
        for cookie in cookies {
            jar.insert_cookie(cookie);
        }
        jars.insert(name, jar);
    }

    Ok(jars)
}

pub fn save_named_jars(path: &Path, jars: &HashMap<String, CookieJar>) -> Result<(), TarnError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            TarnError::Config(format!(
                "Failed to create cookie jar directory '{}': {}",
                parent.display(),
                e
            ))
        })?;
    }

    let file = CookieJarFile {
        schema_version: 1,
        jars: jars
            .iter()
            .map(|(name, jar)| (name.clone(), jar.cookies.clone()))
            .collect(),
    };
    let content = serde_json::to_string_pretty(&file).map_err(|e| {
        TarnError::Config(format!(
            "Failed to serialize cookie jar file '{}': {}",
            path.display(),
            e
        ))
    })?;

    fs::write(path, content).map_err(|e| {
        TarnError::Config(format!(
            "Failed to write cookie jar file '{}': {}",
            path.display(),
            e
        ))
    })
}

impl Cookie {
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expiry.is_some_and(|expiry| expiry <= now)
    }
}

fn parse_set_cookie(header: &str) -> Option<ParsedCookie> {
    let mut segments = header.split(';');
    let first = segments.next()?.trim();
    let (name, value) = first.split_once('=')?;
    if name.trim().is_empty() {
        return None;
    }

    let mut parsed = ParsedCookie {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        ..ParsedCookie::default()
    };

    for segment in segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }

        let (attr_name, attr_value) = segment
            .split_once('=')
            .map(|(name, value)| (name.trim(), Some(value.trim())))
            .unwrap_or((segment, None));

        match attr_name.to_ascii_lowercase().as_str() {
            "domain" => parsed.domain = attr_value.map(str::to_string),
            "path" => parsed.path = attr_value.map(str::to_string),
            "secure" => parsed.secure = true,
            "httponly" => parsed.http_only = true,
            "samesite" => parsed.same_site = attr_value.and_then(parse_same_site),
            "max-age" => {
                parsed.max_age = attr_value.and_then(|value| value.parse::<i64>().ok());
            }
            "expires" => {
                parsed.expiry = attr_value.and_then(parse_cookie_datetime);
            }
            _ => {}
        }
    }

    Some(parsed)
}

fn parse_cookie_datetime(value: &str) -> Option<DateTime<Utc>> {
    const FORMATS: [&str; 3] = [
        "%a, %d %b %Y %H:%M:%S GMT",
        "%A, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ];

    FORMATS.iter().find_map(|format| {
        chrono::DateTime::parse_from_str(value, format)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    })
}

fn max_age_deadline(seconds: i64) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(seconds)
}

fn parse_same_site(value: &str) -> Option<SameSite> {
    match value.to_ascii_lowercase().as_str() {
        "strict" => Some(SameSite::Strict),
        "lax" => Some(SameSite::Lax),
        "none" => Some(SameSite::None),
        _ => None,
    }
}

fn normalize_cookie(mut cookie: Cookie) -> Option<Cookie> {
    if cookie.name.trim().is_empty() {
        return None;
    }

    cookie.domain = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
    if cookie.domain.is_empty() {
        return None;
    }

    if cookie.path.is_empty() || !cookie.path.starts_with('/') {
        cookie.path = "/".to_string();
    }

    Some(cookie)
}

fn normalized_host(url: &Url) -> Option<String> {
    url.host_str().map(|host| host.to_ascii_lowercase())
}

fn normalized_request_path(url: &Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn default_cookie_path(path: &str) -> String {
    if path.is_empty() || !path.starts_with('/') {
        return "/".to_string();
    }
    if path == "/" {
        return "/".to_string();
    }

    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

fn domain_match_for_request(cookie: &Cookie, request_host: &str) -> bool {
    if cookie.host_only {
        request_host == cookie.domain
    } else {
        domain_matches(request_host, &cookie.domain)
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{}", domain))
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/')
        || request_path
            .as_bytes()
            .get(cookie_path.len())
            .is_some_and(|next| *next == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_jar() {
        let jar = CookieJar::new();
        assert!(jar.is_empty());
        assert!(jar.cookie_header("https://example.com/").is_none());
    }

    #[test]
    fn capture_single_cookie() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Path=/; HttpOnly".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);
        assert_eq!(
            jar.cookie_header("https://example.com/account"),
            Some("session=abc123".to_string())
        );
    }

    #[test]
    fn host_only_cookie_does_not_leak_to_subdomains() {
        let mut jar = CookieJar::new();
        let headers = vec![("set-cookie".to_string(), "session=abc123".to_string())];
        jar.capture_from_response("https://example.com/login", &headers);

        assert_eq!(
            jar.cookie_header("https://example.com/profile"),
            Some("session=abc123".to_string())
        );
        assert!(jar
            .cookie_header("https://api.example.com/profile")
            .is_none());
    }

    #[test]
    fn domain_cookie_matches_subdomains() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Domain=example.com; Path=/".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);

        assert_eq!(
            jar.cookie_header("https://api.example.com/profile"),
            Some("session=abc123".to_string())
        );
    }

    #[test]
    fn path_scoping_is_respected() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Path=/api".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);

        assert_eq!(
            jar.cookie_header("https://example.com/api/users"),
            Some("session=abc123".to_string())
        );
        assert!(jar.cookie_header("https://example.com/app").is_none());
    }

    #[test]
    fn default_path_uses_response_directory() {
        let mut jar = CookieJar::new();
        let headers = vec![("set-cookie".to_string(), "session=abc123".to_string())];
        jar.capture_from_response("https://example.com/api/login", &headers);

        assert_eq!(
            jar.cookie_header("https://example.com/api/profile"),
            Some("session=abc123".to_string())
        );
        assert!(jar.cookie_header("https://example.com/").is_none());
    }

    #[test]
    fn secure_cookie_requires_https() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Secure; Path=/".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);

        assert!(jar.cookie_header("http://example.com/").is_none());
        assert_eq!(
            jar.cookie_header("https://example.com/"),
            Some("session=abc123".to_string())
        );
    }

    #[test]
    fn expired_cookie_is_removed() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Path=/".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);

        let deletion = vec![(
            "set-cookie".to_string(),
            "session=gone; Max-Age=0; Path=/".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &deletion);

        assert!(jar.cookie_header("https://example.com/").is_none());
    }

    #[test]
    fn captures_cookie_metadata_attributes() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Domain=example.com; Path=/api; Max-Age=3600; Secure; HttpOnly; SameSite=Strict".to_string(),
        )];

        jar.capture_from_response("https://example.com/login", &headers);

        let cookie = &jar.cookies()[0];
        assert_eq!(cookie.domain, "example.com");
        assert_eq!(cookie.path, "/api");
        assert!(cookie.secure);
        assert!(cookie.http_only);
        assert_eq!(cookie.same_site, Some(SameSite::Strict));
        assert!(cookie.expiry.is_some());
        assert!(!cookie.host_only);
    }

    #[test]
    fn insert_cookie_normalizes_and_exposes_metadata() {
        let mut jar = CookieJar::new();
        jar.insert_cookie(Cookie {
            name: "session".to_string(),
            value: "abc123".to_string(),
            domain: ".example.com".to_string(),
            path: "".to_string(),
            secure: true,
            http_only: true,
            same_site: Some(SameSite::Lax),
            expiry: None,
            host_only: false,
        });

        let cookie = &jar.cookies()[0];
        assert_eq!(cookie.domain, "example.com");
        assert_eq!(cookie.path, "/");
        assert!(cookie.secure);
        assert!(cookie.http_only);
        assert_eq!(cookie.same_site, Some(SameSite::Lax));
    }

    #[test]
    fn load_named_jars_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.json");

        let jars = load_named_jars(&path).unwrap();
        assert!(jars.is_empty());
    }

    #[test]
    fn save_and_load_named_jars_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.json");

        let mut default_jar = CookieJar::new();
        default_jar.insert_cookie(Cookie {
            name: "session".to_string(),
            value: "abc123".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: Some(SameSite::Strict),
            expiry: None,
            host_only: false,
        });

        let mut admin_jar = CookieJar::new();
        admin_jar.insert_cookie(Cookie {
            name: "admin".to_string(),
            value: "yes".to_string(),
            domain: "example.com".to_string(),
            path: "/admin".to_string(),
            secure: false,
            http_only: false,
            same_site: Some(SameSite::Lax),
            expiry: None,
            host_only: true,
        });

        let jars = HashMap::from([
            ("default".to_string(), default_jar),
            ("admin".to_string(), admin_jar),
        ]);

        save_named_jars(&path, &jars).unwrap();
        let loaded = load_named_jars(&path).unwrap();

        assert_eq!(
            loaded.get("default").unwrap().cookies(),
            jars["default"].cookies()
        );
        assert_eq!(
            loaded.get("admin").unwrap().cookies(),
            jars["admin"].cookies()
        );
    }

    #[test]
    fn rejects_domain_cookie_for_unrelated_host() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Domain=other.com".to_string(),
        )];
        jar.capture_from_response("https://example.com/login", &headers);
        assert!(jar.is_empty());
    }

    #[test]
    fn ignores_non_cookie_headers() {
        let mut jar = CookieJar::new();
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("set-cookie".to_string(), "token=abc; Path=/".to_string()),
        ];
        jar.capture_from_response("https://example.com/login", &headers);
        assert_eq!(
            jar.cookie_header("https://example.com/"),
            Some("token=abc".to_string())
        );
    }

    #[test]
    fn case_insensitive_header_name() {
        let mut jar = CookieJar::new();
        let headers = vec![("Set-Cookie".to_string(), "token=abc; Path=/".to_string())];
        jar.capture_from_response("https://example.com/login", &headers);
        assert_eq!(
            jar.cookie_header("https://example.com/"),
            Some("token=abc".to_string())
        );
    }
}
