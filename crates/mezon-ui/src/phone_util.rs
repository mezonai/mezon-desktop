pub const LOGIN_COUNTRIES: [(&str, &str); 1] = [("Vietnam", "+84")];

pub const COUNTRIES: [(&str, &str); 3] = [("Vietnam", "+84"), ("Japan", "+81"), ("US", "+1")];

pub fn parse_phone_vn(phone: &str) -> String {
    if let Some(rest) = phone.strip_prefix('0') {
        format!("+84{rest}")
    } else if phone.starts_with("+84") {
        phone.to_string()
    } else {
        format!("+84{phone}")
    }
}

pub fn is_valid_phone_vn(phone: &str) -> bool {
    let parsed = parse_phone_vn(phone);
    let Some(national) = parsed.strip_prefix("+84") else {
        return false;
    };
    let mut chars = national.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '3' | '5' | '7' | '8' | '9') {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    rest.len() == 8 && rest.iter().all(|c| c.is_ascii_digit())
}

pub fn build_login_phone(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !is_valid_phone_vn(trimmed) {
        return None;
    }
    Some(parse_phone_vn(trimmed))
}

pub fn phone_error_key(raw: &str) -> Option<&'static str> {
    if raw.trim().is_empty() {
        Some("common.login.enterPhoneToLogin")
    } else if build_login_phone(raw).is_none() {
        Some("common.login.invalidPhoneNumber")
    } else {
        None
    }
}

pub fn mask_phone(phone: &str) -> String {
    let count = phone.chars().count();
    if count < 3 {
        return phone.to_string();
    }
    let tail: String = phone.chars().skip(count - 2).collect();
    format!("xxx-xxx{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vn_phone_validation_matches_modal_rules() {
        for prefix in ['3', '5', '7', '8', '9'] {
            assert!(is_valid_phone_vn(&format!("0{prefix}12345678")));
        }
        assert!(is_valid_phone_vn("+84912345678"));
        assert!(!is_valid_phone_vn("0112345678"));
        assert!(!is_valid_phone_vn("091234567"));
    }

    #[test]
    fn build_login_phone_accepts_only_vietnam_numbers() {
        assert_eq!(
            build_login_phone("0912345678").as_deref(),
            Some("+84912345678")
        );
        assert!(build_login_phone("123").is_none());
        assert!(build_login_phone("+819012345678").is_none());
    }
}
