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

pub fn normalize_diacritics(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.nfd().flat_map(char::to_lowercase) {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        push_compatibility_folded(ch, &mut out);
    }
    out
}
