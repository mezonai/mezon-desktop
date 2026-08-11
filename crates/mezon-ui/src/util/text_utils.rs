use unicode_normalization::UnicodeNormalization;

fn push_compatibility_folded(ch: char, out: &mut String) {
    match ch {
        '\u{0111}' | '\u{00f0}' => out.push('d'),
        '\u{0142}' => out.push('l'),
        '\u{00f8}' => out.push('o'),
        '\u{00df}' => out.push_str("ss"),
        '\u{00e6}' => out.push_str("ae"),
        '\u{0153}' => out.push_str("oe"),
        '\u{00fe}' => out.push_str("th"),
        _ => out.push(ch),
    }
}

pub(crate) fn normalize_diacritics(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.nfd().flat_map(char::to_lowercase) {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        push_compatibility_folded(ch, &mut out);
    }
    out
}

pub(crate) fn normalize_string(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(value.len());
    for ch in value.nfd() {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        out.extend(ch.to_uppercase());
    }
    out
}

fn push_search_normalized_part(part: char, out: &mut String) {
    if ('\u{0300}'..='\u{036f}').contains(&part) {
        return;
    }
    match part {
        '-' | '_' | '+' => out.push(' '),
        _ => out.extend(part.to_uppercase()),
    }
}

pub(crate) fn push_search_normalized(ch: char, out: &mut String) {
    for part in ch.nfd() {
        push_search_normalized_part(part, out);
    }
}

pub(crate) fn normalize_search_string(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(value.len());
    for part in value.nfd() {
        push_search_normalized_part(part, &mut out);
    }
    out
}

pub fn compute_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .take(2)
        .filter_map(|s| s.chars().next())
        .collect::<String>()
        .to_uppercase();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_folds_diacritics_and_non_decomposing_letters() {
        assert_eq!(normalize_diacritics("Đà Nẵng"), "da nang");
        assert_eq!(normalize_diacritics("Ð ł ø ß æ œ þ"), "d l o ss ae oe th");
    }
}
