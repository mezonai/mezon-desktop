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

const LETTERS_END: u8 = 0x00;
const SYMBOL_PREFIX: u8 = 0x01;
const ASCII_DIGIT_BASE: u8 = 0x10;
const OTHER_DIGIT_PREFIX: u8 = 0x1a;
const LOWERCASE_BASE: u8 = 0x20;
const UPPERCASE_BASE: u8 = 0x50;
const OUTSIDE_VIETNAMESE_ALPHABET: u8 = 33;
const COMBINING_BREVE: char = '\u{0306}';
const COMBINING_CIRCUMFLEX: char = '\u{0302}';
const COMBINING_HORN: char = '\u{031b}';

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Symbol,
    Digit,
    Lowercase,
    Uppercase,
}

#[derive(Clone, Copy)]
enum Tone {
    Grave = 1,
    HookAbove,
    Tilde,
    Acute,
    DotBelow,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VietnameseSortKey(Vec<u8>);

fn tone_of(mark: char) -> Option<Tone> {
    match mark {
        '\u{0300}' => Some(Tone::Grave),
        '\u{0309}' => Some(Tone::HookAbove),
        '\u{0303}' => Some(Tone::Tilde),
        '\u{0301}' => Some(Tone::Acute),
        '\u{0323}' => Some(Tone::DotBelow),
        _ => None,
    }
}

fn with_vowel_mark(letter: char, mark: char) -> char {
    match (letter, mark) {
        ('a', COMBINING_BREVE) => 'ă',
        ('a', COMBINING_CIRCUMFLEX) => 'â',
        ('e', COMBINING_CIRCUMFLEX) => 'ê',
        ('o', COMBINING_CIRCUMFLEX) => 'ô',
        ('o', COMBINING_HORN) => 'ơ',
        ('u', COMBINING_HORN) => 'ư',
        _ => letter,
    }
}

fn vietnamese_alphabet_rank(letter: char) -> Option<u8> {
    Some(match letter {
        'a' => 0,
        'ă' => 1,
        'â' => 2,
        'b'..='d' => letter as u8 - b'b' + 3,
        'đ' => 6,
        'e' => 7,
        'ê' => 8,
        'f'..='o' => letter as u8 - b'f' + 9,
        'ô' => 19,
        'ơ' => 20,
        'p'..='u' => letter as u8 - b'p' + 21,
        'ư' => 27,
        'v'..='z' => letter as u8 - b'v' + 28,
        _ => return None,
    })
}

fn classify_ascii(byte: u8) -> (CharClass, char) {
    match byte {
        b'0'..=b'9' => (CharClass::Digit, byte as char),
        b'a'..=b'z' => (CharClass::Lowercase, byte as char),
        b'A'..=b'Z' => (CharClass::Uppercase, byte.to_ascii_lowercase() as char),
        _ => (CharClass::Symbol, byte as char),
    }
}

fn classify(ch: char) -> (CharClass, char) {
    if ch.is_ascii() {
        classify_ascii(ch as u8)
    } else if ch.is_numeric() {
        (CharClass::Digit, ch)
    } else if ch.is_alphabetic() {
        let class = if ch.is_uppercase() {
            CharClass::Uppercase
        } else {
            CharClass::Lowercase
        };
        (class, ch.to_lowercase().next().unwrap_or(ch))
    } else {
        (CharClass::Symbol, ch)
    }
}

fn push_codepoint(out: &mut Vec<u8>, ch: char) {
    out.extend_from_slice(&(ch as u32).to_be_bytes()[1..]);
}

fn push_letter_code(out: &mut Vec<u8>, (class, ch): (CharClass, char)) {
    match class {
        CharClass::Symbol => {
            out.push(SYMBOL_PREFIX);
            push_codepoint(out, ch);
        }
        CharClass::Digit => match ch.to_digit(10) {
            Some(digit) => out.push(ASCII_DIGIT_BASE + digit as u8),
            None => {
                out.push(OTHER_DIGIT_PREFIX);
                push_codepoint(out, ch);
            }
        },
        CharClass::Lowercase | CharClass::Uppercase => {
            let base = if class == CharClass::Lowercase {
                LOWERCASE_BASE
            } else {
                UPPERCASE_BASE
            };
            match vietnamese_alphabet_rank(ch) {
                Some(rank) => out.push(base + rank),
                None => {
                    out.push(base + OUTSIDE_VIETNAMESE_ALPHABET);
                    push_codepoint(out, ch);
                }
            }
        }
    }
}

struct SortKeyBuilder {
    letters: Vec<u8>,
    tones: Vec<u8>,
    pending: Option<(CharClass, char)>,
}

impl SortKeyBuilder {
    fn push_base(&mut self, entry: (CharClass, char)) {
        if let Some(previous) = self.pending.replace(entry) {
            push_letter_code(&mut self.letters, previous);
        }
        self.tones.push(0);
    }

    fn push_decomposed(&mut self, ch: char) {
        if let Some(tone) = tone_of(ch) {
            if let Some(last) = self.tones.last_mut() {
                *last = tone as u8;
            }
        } else if ('\u{0300}'..='\u{036f}').contains(&ch) {
            if let Some((class, letter)) = self.pending.as_mut()
                && matches!(class, CharClass::Lowercase | CharClass::Uppercase)
            {
                *letter = with_vowel_mark(*letter, ch);
            }
        } else {
            self.push_base(classify(ch));
        }
    }

    fn finish(mut self) -> VietnameseSortKey {
        if let Some(last) = self.pending.take() {
            push_letter_code(&mut self.letters, last);
        }
        self.letters.push(LETTERS_END);
        self.letters.extend_from_slice(&self.tones);
        VietnameseSortKey(self.letters)
    }
}

pub fn vietnamese_sort_key(value: &str) -> VietnameseSortKey {
    let mut builder = SortKeyBuilder {
        letters: Vec::with_capacity(value.len() * 2 + 1),
        tones: Vec::with_capacity(value.len()),
        pending: None,
    };
    for ch in value.chars() {
        if ch.is_ascii() {
            builder.push_base(classify_ascii(ch as u8));
        } else {
            unicode_normalization::char::decompose_canonical(ch, |part| {
                builder.push_decomposed(part)
            });
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precomposed_and_decomposed_spellings_share_a_key() {
        let spellings = ["ệ", "e\u{0323}\u{0302}", "e\u{0302}\u{0323}", "ê\u{0323}"];
        for spelling in spellings {
            assert_eq!(vietnamese_sort_key(spelling), vietnamese_sort_key("ệ"));
        }
        assert_eq!(
            vietnamese_sort_key("Đà Nẵng"),
            vietnamese_sort_key("Đa\u{0300} Na\u{0306}\u{0303}ng")
        );
    }

    #[test]
    fn a_shorter_name_sorts_before_a_longer_one_it_starts() {
        assert!(vietnamese_sort_key("ab") < vietnamese_sort_key("abc"));
        assert!(vietnamese_sort_key("bàn") < vietnamese_sort_key("bàng"));
        assert!(vietnamese_sort_key("📅") < vietnamese_sort_key("📅 daily"));
    }
}
