use std::collections::HashMap;

/// Simple cookie jar that automatically captures Set-Cookie headers
/// and sends stored cookies on subsequent requests.
#[derive(Debug, Clone)]
pub struct CookieJar {
    /// Cookies stored as name -> value pairs.
    cookies: HashMap<String, String>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            cookies: HashMap::new(),
        }
    }

    /// Parse Set-Cookie headers from the response and store cookies.
    /// Accepts raw headers (Vec of name-value pairs) to handle multiple Set-Cookie headers.
    pub fn capture_from_response(&mut self, raw_headers: &[(String, String)]) {
        for (name, value) in raw_headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                // Parse "name=value; path=/; ..." — take only the name=value part
                if let Some(cookie_part) = value.split(';').next() {
                    if let Some((cookie_name, cookie_val)) = cookie_part.split_once('=') {
                        self.cookies.insert(
                            cookie_name.trim().to_string(),
                            cookie_val.trim().to_string(),
                        );
                    }
                }
            }
        }
    }

    /// Generate a Cookie header value from all stored cookies.
    /// Returns None if no cookies are stored.
    pub fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = self
            .cookies
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        parts.sort(); // deterministic output
        Some(parts.join("; "))
    }

    /// Check if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_jar() {
        let jar = CookieJar::new();
        assert!(jar.is_empty());
        assert!(jar.cookie_header().is_none());
    }

    #[test]
    fn capture_single_cookie() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "set-cookie".to_string(),
            "session=abc123; Path=/; HttpOnly".to_string(),
        )];
        jar.capture_from_response(&headers);
        assert_eq!(jar.cookie_header(), Some("session=abc123".to_string()));
    }

    #[test]
    fn capture_multiple_cookies() {
        let mut jar = CookieJar::new();
        let headers = vec![
            (
                "set-cookie".to_string(),
                "session=abc123; Path=/".to_string(),
            ),
            (
                "set-cookie".to_string(),
                "csrf=xyz789; Path=/".to_string(),
            ),
        ];
        jar.capture_from_response(&headers);
        assert!(!jar.is_empty());
        let header = jar.cookie_header().unwrap();
        assert!(header.contains("session=abc123"));
        assert!(header.contains("csrf=xyz789"));
    }

    #[test]
    fn overwrite_existing_cookie() {
        let mut jar = CookieJar::new();
        let headers1 = vec![(
            "set-cookie".to_string(),
            "session=old; Path=/".to_string(),
        )];
        jar.capture_from_response(&headers1);

        let headers2 = vec![(
            "set-cookie".to_string(),
            "session=new; Path=/".to_string(),
        )];
        jar.capture_from_response(&headers2);

        assert_eq!(jar.cookie_header(), Some("session=new".to_string()));
    }

    #[test]
    fn ignores_non_cookie_headers() {
        let mut jar = CookieJar::new();
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            (
                "set-cookie".to_string(),
                "token=abc; Path=/".to_string(),
            ),
        ];
        jar.capture_from_response(&headers);
        assert_eq!(jar.cookie_header(), Some("token=abc".to_string()));
    }

    #[test]
    fn case_insensitive_header_name() {
        let mut jar = CookieJar::new();
        let headers = vec![(
            "Set-Cookie".to_string(),
            "token=abc; Path=/".to_string(),
        )];
        jar.capture_from_response(&headers);
        assert_eq!(jar.cookie_header(), Some("token=abc".to_string()));
    }
}
