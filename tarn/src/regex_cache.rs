use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn cache() -> &'static Mutex<HashMap<String, Regex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get(pattern: &str) -> Result<Regex, regex::Error> {
    let mut cache = cache().lock().unwrap();
    if let Some(regex) = cache.get(pattern) {
        return Ok(regex.clone());
    }

    let regex = Regex::new(pattern)?;
    cache.insert(pattern.to_string(), regex.clone());
    Ok(regex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_and_reuses_compiled_regex() {
        let first = get(r"^\d+$").unwrap();
        let second = get(r"^\d+$").unwrap();

        assert!(first.is_match("123"));
        assert_eq!(first.as_str(), second.as_str());
    }
}
