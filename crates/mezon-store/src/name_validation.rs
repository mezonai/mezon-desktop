use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

pub const DISPLAY_NAME_MAX_BYTES: usize = 32;
pub const QUICK_MENU_NAME_MAX_RUNES: usize = 64;
pub const CLAN_NAME_MAX_CHARS: usize = 64;

static NAME_CHAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}\p{So}_\ \-\.\+]$").expect("name char regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayNameError {
    TooLong,
    InvalidChars,
}

pub fn prepare_display_name_for_update(name: &str) -> Result<Option<String>, DisplayNameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let normalized: String = trimmed.nfc().collect();
    if normalized.len() > DISPLAY_NAME_MAX_BYTES {
        return Err(DisplayNameError::TooLong);
    }
    if !is_valid_name_content(&normalized) {
        return Err(DisplayNameError::InvalidChars);
    }
    Ok(Some(normalized))
}

pub fn is_valid_name_content(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return false;
    };
    if first == '_' || first == '-' {
        return false;
    }
    name.chars().all(is_valid_name_char)
}

pub fn is_valid_menu_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= QUICK_MENU_NAME_MAX_RUNES
        && is_valid_name_content(name)
}

pub fn is_valid_clan_name(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > CLAN_NAME_MAX_CHARS {
        return false;
    }
    let Some(first) = name.chars().next() else {
        return false;
    };
    if matches!(first, '_' | '-' | ' ') {
        return false;
    }
    name.chars()
        .all(|c| is_valid_clan_name_char(c) && c != '\'')
}

fn is_valid_clan_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | ' ')
}

fn is_valid_name_char(c: char) -> bool {
    if is_name_emoji(c) {
        return true;
    }
    let mut buf = [0u8; 4];
    NAME_CHAR.is_match(c.encode_utf8(&mut buf))
}

fn is_name_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x1F600..=0x1F64F
            | 0x1F300..=0x1F5FF
            | 0x1F680..=0x1F6FF
            | 0x1F700..=0x1F77F
            | 0x1F780..=0x1F7FF
            | 0x1F800..=0x1F8FF
            | 0x1F900..=0x1F9FF
            | 0x1FA00..=0x1FA6F
            | 0x1FA70..=0x1FAFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_display_name_rejects_over_32_bytes_and_whitespace_only() {
        assert_eq!(prepare_display_name_for_update("").unwrap(), None);
        assert_eq!(prepare_display_name_for_update("   ").unwrap(), None);
        assert_eq!(
            prepare_display_name_for_update("Alice").unwrap(),
            Some("Alice".into())
        );
        assert_eq!(
            prepare_display_name_for_update(&"a".repeat(32)).unwrap(),
            Some("a".repeat(32))
        );
        assert!(matches!(
            prepare_display_name_for_update(&"a".repeat(33)),
            Err(DisplayNameError::TooLong)
        ));
    }

    #[test]
    fn prepare_display_name_rejects_invalid_chars() {
        assert!(matches!(
            prepare_display_name_for_update("_Alice"),
            Err(DisplayNameError::InvalidChars)
        ));
        assert!(matches!(
            prepare_display_name_for_update("-Alice"),
            Err(DisplayNameError::InvalidChars)
        ));
        assert!(matches!(
            prepare_display_name_for_update("Alice!"),
            Err(DisplayNameError::InvalidChars)
        ));
    }

    #[test]
    fn prepare_display_name_normalizes_decomposed_vietnamese_to_nfc() {
        let decomposed = "e\u{0302}\u{0303}";
        let prepared = prepare_display_name_for_update(decomposed).unwrap();
        assert_eq!(prepared, Some("\u{1EC5}".into()));
    }

    #[test]
    fn prepare_display_name_rejects_eleven_precomposed_vietnamese_chars_as_33_bytes() {
        let name = "\u{1EC5}".repeat(11);
        assert_eq!(name.len(), 33);
        assert!(matches!(
            prepare_display_name_for_update(&name),
            Err(DisplayNameError::TooLong)
        ));
    }

    #[test]
    fn menu_name_rejects_empty_leading_underscore_dash_and_over_64() {
        assert!(!is_valid_menu_name(""));
        assert!(!is_valid_menu_name("_hello"));
        assert!(!is_valid_menu_name("-hello"));
        assert!(is_valid_menu_name("hello"));
        assert!(is_valid_menu_name(&"a".repeat(64)));
        assert!(!is_valid_menu_name(&"a".repeat(65)));
    }

    #[test]
    fn clan_name_rejects_leading_space_and_apostrophe() {
        assert!(!is_valid_clan_name(" hello"));
        assert!(!is_valid_clan_name("it's"));
        assert!(is_valid_clan_name("hello world"));
    }
}
