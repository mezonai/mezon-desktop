use std::collections::HashMap;
use std::sync::OnceLock;

fn data(locale: &str) -> &HashMap<String, String> {
    match locale {
        "vi" => {
            static VI: OnceLock<HashMap<String, String>> = OnceLock::new();
            VI.get_or_init(|| {
                serde_json::from_str(include_str!("../../../assets/i18n/vi.json"))
                    .expect("Invalid vi.json")
            })
        }
        _ => {
            static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
            EN.get_or_init(|| {
                serde_json::from_str(include_str!("../../../assets/i18n/en.json"))
                    .expect("Invalid en.json")
            })
        }
    }
}

/// Look up a translation key for the given locale.
///
/// Falls back to English if the key is missing in the requested locale,
/// and returns `key` itself if not found in English either.
pub fn t(locale: &str, key: &str) -> String {
    if let Some(value) = data(locale).get(key) {
        return value.clone();
    }
    if locale != "en" && let Some(value) = data("en").get(key) {
        return value.clone();
    }
    key.to_string()
}
