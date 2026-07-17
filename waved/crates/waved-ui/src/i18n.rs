//! JSON string table with fallback to key name.

use std::collections::HashMap;

pub struct I18n {
    strings: HashMap<String, String>,
}

impl I18n {
    /// Load from `/data/waved/lang/{lang}.json`. Falls back to empty map if missing.
    pub fn load(lang: &str) -> Self {
        let path = format!("/data/waved/lang/{lang}.json");
        let strings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();
        Self { strings }
    }

    /// Look up key. Returns the key itself if not found — never panics.
    pub fn tr<'a>(&'a self, key: &'a str) -> &'a str {
        self.strings.get(key).map(|s| s.as_str()).unwrap_or(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_returns_key_name() {
        let i18n = I18n {
            strings: Default::default(),
        };
        assert_eq!(i18n.tr("some.key"), "some.key");
    }

    #[test]
    fn found_key_returns_translation() {
        let mut strings = std::collections::HashMap::new();
        strings.insert("hello".into(), "Hello World".into());
        let i18n = I18n { strings };
        assert_eq!(i18n.tr("hello"), "Hello World");
    }
}
