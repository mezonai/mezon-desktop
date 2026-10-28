//! Context-Aware Personal Pronoun Extraction (Xưng - Hô) and Smart Quick Replies.
//! Supports reciprocal workplace relationships (anh, chị, em, sếp, cả nhà, cậu, etc.).

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

pub static SUPPORTED_PRONOUNS: [&str; 13] = [
    "anh",
    "chị",
    "em",
    "sếp",
    "cậu",
    "ông",
    "bà",
    "bác",
    "chú",
    "cô",
    "thầy",
    "bạn",
    "cả nhà",
];

pub fn normalize_pronoun(p: Option<&str>) -> Option<&'static str> {
    let p = p?.trim().to_lowercase();
    match p.as_str() {
        "anh" | "a" => Some("anh"),
        "chị" | "c" | "chi" => Some("chị"),
        "em" | "e" => Some("em"),
        "sếp" | "sep" | "boss" => Some("sếp"),
        "cậu" | "cau" => Some("cậu"),
        "ông" | "ong" => Some("ông"),
        "bà" | "ba" => Some("bà"),
        "bác" | "bac" => Some("bác"),
        "chú" | "chu" => Some("chú"),
        "cô" | "co" => Some("cô"),
        "thầy" | "thay" => Some("thầy"),
        "cả nhà" | "ca nha" | "team" | "mọi người" | "moi nguoi" => Some("cả nhà"),
        "bạn" | "ban" => Some("bạn"),
        _ => None,
    }
}

/// Strips accents for robust regex matching on informal Vietnamese chat.
fn strip_accents(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let mapped = match c {
            'à' | 'á' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ằ' | 'ắ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ầ' | 'ấ'
            | 'ẩ' | 'ẫ' | 'ậ' => 'a',
            'è' | 'é' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ề' | 'ế' | 'ể' | 'ễ' | 'ệ' => {
                'e'
            }
            'ì' | 'í' | 'ỉ' | 'ĩ' | 'ị' => 'i',
            'ò' | 'ó' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ồ' | 'ố' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ờ' | 'ớ'
            | 'ở' | 'ỡ' | 'ợ' => 'o',
            'ù' | 'ú' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ừ' | 'ứ' | 'ử' | 'ữ' | 'ự' => {
                'u'
            }
            'ỳ' | 'ý' | 'ỷ' | 'ỹ' | 'ỵ' => 'y',
            'đ' => 'd',
            'À' | 'Á' | 'Ả' | 'Ã' | 'Ạ' | 'Ă' | 'Ằ' | 'Ắ' | 'Ẳ' | 'Ẵ' | 'Ặ' | 'Â' | 'Ầ' | 'Ấ'
            | 'Ẩ' | 'Ẫ' | 'Ậ' => 'A',
            'È' | 'É' | 'Ẻ' | 'Ẽ' | 'Ẹ' | 'Ê' | 'Ề' | 'Ế' | 'Ể' | 'Ễ' | 'Ệ' => {
                'E'
            }
            'Ì' | 'Í' | 'Ỉ' | 'Ĩ' | 'Ị' => 'I',
            'Ò' | 'Ó' | 'Ỏ' | 'Õ' | 'Ọ' | 'Ô' | 'Ồ' | 'Ố' | 'Ổ' | 'Ỗ' | 'Ộ' | 'Ơ' | 'Ờ' | 'Ớ'
            | 'Ở' | 'Ỡ' | 'Ợ' => 'O',
            'Ù' | 'Ú' | 'Ủ' | 'Ũ' | 'Ụ' | 'Ư' | 'Ừ' | 'Ứ' | 'Ử' | 'Ữ' | 'Ự' => {
                'U'
            }
            'Ỳ' | 'Ý' | 'Ỷ' | 'Ỹ' | 'Ỵ' => 'Y',
            'Đ' => 'D',
            other => other,
        };
        out.push(mapped);
    }
    out
}

static RE_UNREPLYABLE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let raw = [
        // 1. Standalone laughter / emoji / emoticons
        r"(?i)^(:[\s\)]+|:\(+|:-?\)|:-?D|:v|:3|:<|😁|😀|😂|🤣|👍|🙏|❤️|🎉|:da:|:like:|:ok:|:clown:)+$",
        r"(?i)^(ha(ha)+|hi(hi)+|ka(ka)+|he(he)+|hu(hu)+|hic|lol|cười)(\s+(ha(ha)+|hi(hi)+|ka(ka)+|he(he)+|hu(hu)+|hic|lol))*$",
        // 2. Pure user tags / mentions
        r"^(@[a-zA-Z0-9_.-]+(\s+@[a-zA-Z0-9_.-]+)*)$",
        // 3. Standalone acknowledgements / closing phrases
        r"(?i)^(ok|okie|oke|okay)(\s+(nhe|nhé|nha|anh|em|ban|bạn|sep|sếp|chi|chị|chu|chú|bac|bác|ca\s+nha|cả\s+nhà))?(\s+(nhe|nhé|nha|a|ah|ạ))?$",
        r"(?i)^(vang|vâng|da|dạ|da\s+vang|dạ\s+vâng)(\s+(a|ah|ạ|anh|em|chi|chị|sep|sếp|bac|bác|chu|chú|co|cô|thay|thầy))?(\s+(a|ah|ạ))?$",
        r"(?i)^(yep|yeah|yes|ukm|um|ừm|uhm|uh|ừ|u|roi|rồi|roi\s+nhe|rồi\s+nhé|done|xong\s+roi|xong\s+rồi|the\s+nhe|thế\s+nhé|vay\s+nhe|vậy\s+nhé|nhat\s+tri|nhất\s+trí|chuan|chuẩn)(\s+(nhe|nhé|nha|a|ah|ạ))?$",
        // 4. Code / URLs / OTP / technical noise
        r"(?i)^(https?://|git\s+|npm\s+|curl\s+|docker\s+|error:|select\s+\*|const\s+|let\s+|var\s+)\b",
        r"(?i)^(thông báo:|thong bao:)",
        r"^(\d+[\s,.\d]*|\.\.\.|\?\?\?|[a-z0-9_./-]{30,})$",
    ];
    raw.into_iter().map(|p| Regex::new(p).unwrap()).collect()
});

static RE_SENSITIVE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(so tai khoan|ma otp|so dien thoai|cccd)\b").unwrap());

pub fn is_unreplyable_or_ack_message(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    for re in RE_UNREPLYABLE_PATTERNS.iter() {
        if re.is_match(trimmed) {
            return true;
        }
    }
    let stripped = strip_accents(&trimmed.to_lowercase());
    if RE_SENSITIVE_PREFIX.is_match(&stripped) {
        return true;
    }
    false
}

static RE_NON_PRONOUNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let raw = [
        r"(?i)\b(dia\s+chi|ho\s+chi\s+minh|chi\s+(co|la|can|dao|tieu|so|phi|nhanh|tiet|tieu))\b",
        r"(?i)\b(tieng\s+anh|(hinh|buc|file|chup)\s+anh|anh\s+chup)\b",
        r"(?i)\b((dong|tay|mien|phia)\s+bac|bac\s+(ninh|giang|kan|si|bo|ai))\b",
        r"(?i)\b(chu\s+y|cham\s+chu)\b",
        r"(?i)\b(cong\s+ty|co\s+gang|su\s+co|co\s+hoi)\b",
        r"(?i)\b((khong|ko|k|chua|se|da|dang|cung|van|neu)\s+co|co\s+(the|ve|phai|hay|chua|khong|ko|k|san|it|nhieu|the|gi|sao|dau))\b",
        r"(?i)\b(o\s+to|(nang|mua|noi|to)\s+to|to\s+(lon|qua))\b",
        r"(?i)\b(cau\s+(long|thu|nguyen|hoi|chuyen)|(yeu|nhu)\s+cau)\b",
        r"(?i)\b(ba\s+con|ban\s+bac|ban\s+ghe|ba\s+me|thu\s+ba)\b",
        r"(?i)\b(cong\s+cong|ong\s+ba)\b",
    ];
    raw.into_iter().map(|p| Regex::new(p).unwrap()).collect()
});

pub fn mask_non_pronouns(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut res = text.to_string();
    for re in RE_NON_PRONOUNS.iter() {
        res = re.replace_all(&res, " ").into_owned();
    }
    static RE_MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    RE_MULTI_SPACE.replace_all(&res, " ").trim().to_string()
}

static RE_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\.,!?…]+").unwrap());

static RE_MENTIONED_COLLECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(ca nha|moi nguoi|team|all|mng|mn)\s+oi\b|\b(chao|gui|chuc|cam on|xin loi|thong bao|nhan|nhe|nha)\s+(ca nha|moi nguoi|team|all|mng|mn)\b|@\s*(all|here|channel|team|everyone)\b").unwrap()
});

static RE_MENTIONED_AT_ROLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(anh|chi|em|chu|bac|co|sep|thay)\s+@|@[\w.-]+\s+(anh|chi|em|chu|bac|co|sep|thay)\b").unwrap()
});

static RE_MENTIONED_OI: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(?i)\b(sep|boss|leader)\s+oi\b").unwrap(), "sếp"),
        (Regex::new(r"(?i)\b(thay)\s+oi\b").unwrap(), "thầy"),
        (Regex::new(r"(?i)\b(bac)\s+oi\b").unwrap(), "bác"),
        (Regex::new(r"(?i)\b(chu)\s+oi\b").unwrap(), "chú"),
        (Regex::new(r"(?i)\b(co)\s+oi\b").unwrap(), "cô"),
        (Regex::new(r"(?i)\b(chau)\s+oi\b").unwrap(), "cháu"),
        (Regex::new(r"(?i)\b(cau)\s+oi\b").unwrap(), "cậu"),
        (Regex::new(r"(?i)\b(ong)\s+oi\b").unwrap(), "ông"),
        (Regex::new(r"(?i)\b(ba)\s+oi\b").unwrap(), "bà"),
        (Regex::new(r"(?i)\b(anh|a)\s+oi\b").unwrap(), "anh"),
        (Regex::new(r"(?i)\b(chi|c)\s+oi\b").unwrap(), "chị"),
        (Regex::new(r"(?i)\b(em|e)\s+oi\b").unwrap(), "em"),
    ]
});

static RE_MENTIONED_SPEECH_ACTS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let v = r"(chao|cam on|thanks|thank|tks|cmon|xin loi|sr|sorry|chuc|gui|hoi|nho|cho|dan)";
    vec![
        (Regex::new(&format!(r"(?i)\b{v}\s+(sep|boss|leader)\b")).unwrap(), "sếp"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(thay)\b")).unwrap(), "thầy"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(bac)\b")).unwrap(), "bác"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(chu)\b")).unwrap(), "chú"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(co)\b")).unwrap(), "cô"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(chau)\b")).unwrap(), "cháu"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(ong)\b")).unwrap(), "ông"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(ba)\b")).unwrap(), "bà"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(cau)\b")).unwrap(), "cậu"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(anh|a)\b")).unwrap(), "anh"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(chi|c)\b")).unwrap(), "chị"),
        (Regex::new(&format!(r"(?i)\b{v}\s+(em|e)\b")).unwrap(), "em"),
    ]
});

static RE_MENTIONED_DIRECTIVES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let roles = [
        (r"\b(sep|boss|leader)\b", "sếp"),
        (r"\b(thay)\b", "thầy"),
        (r"\b(bac)\b", "bác"),
        (r"\b(chu)\b", "chú"),
        (r"\b(cau)\b", "cậu"),
        (r"\b(anh|a)\b", "anh"),
        (r"\b(chi|c)\b", "chị"),
        (r"\b(em|e)\b", "em"),
    ];
    roles
        .into_iter()
        .map(|(role_pat, role_val)| {
            (
                Regex::new(&format!(
                    r"(?i){role_pat}\s+.*?(giup|gium|ho)\s+(em|e|minh|to|chau|anh|chi|sep|bac|chu|co)\b"
                ))
                .unwrap(),
                role_val,
            )
        })
        .collect()
});

static RE_MENTIONED_END_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(sep|boss|leader)\b|\b(sep)\s+a\b").unwrap());
static RE_MENTIONED_END_THAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(thay)\b|\b(thay)\s+a\b|\b(kinh|da)\s+(thay)\b").unwrap());
static RE_MENTIONED_END_BAC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(bac)\b|\b(bac)\s+a\b").unwrap());
static RE_MENTIONED_END_CHU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(chu)\b|\b(chu)\s+a\b").unwrap());
static RE_MENTIONED_END_CO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(co)\b|\b(co)\s+a\b").unwrap());
static RE_MENTIONED_END_CHAU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(chau)\b|\b(chau)\s+a\b").unwrap());
static RE_MENTIONED_END_CAU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(cau)\b").unwrap());
static RE_MENTIONED_END_ONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(ong)\b").unwrap());
static RE_MENTIONED_END_BA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(ba)\b").unwrap());
static RE_MENTIONED_END_ANH_POLITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(anh)\b|\b(anh)\s+a\b").unwrap());
static RE_MENTIONED_END_ANH_NORMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(anh|a)\b|\b(anh|a)\s+a\b").unwrap());
static RE_MENTIONED_END_CHI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(chi|c)\b|\b(chi|c)\s+a\b").unwrap());
static RE_MENTIONED_END_EM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(nhe|nha|ha|vay|the|chua|khong|ko|k)\s+(em|e)\b|\b(em|e)\s+a\b").unwrap());

static RE_MENTIONED_INQUIRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(chua|chx|khong|ko|k|the nao|sao roi|khoe khong|on khong|ok khong|dc khong)").unwrap());

static RE_MENTIONED_ASYM_ANH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(anh|a)\s+(review|check|xem|kiem tra|duyet|merge|chay|deploy|fix|sua)\b").unwrap());
static RE_MENTIONED_ASYM_CHI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(chi|c)\s+(review|check|xem|kiem tra|duyet|merge|chay|deploy|fix|sua)\b").unwrap());
static RE_MENTIONED_ASYM_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(sep)\s+(duyet|check|xem|ky|xac nhan)\b").unwrap());
static RE_MENTIONED_ASYM_EM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(em|e)\s+(review|check|xem|kiem tra|duyet|merge|chay|deploy|fix|sua)\b").unwrap());
static RE_MENTIONED_GIUP_ANH_CHI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(giup anh|giup chi|cho anh|cho chi)\b").unwrap());

static RE_MENTIONED_CO_SPECIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(co\s+oi|thua\s+co|chao\s+co|gui\s+co|hoi\s+co)\b").unwrap());
static RE_MENTIONED_CAC_CHU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(cac chu|chu em)\b").unwrap());

/// Extracts the personal pronoun of WHO IS MENTIONED / ADDRESSED in the incoming message.
/// Eliminates false positive non-pronoun homophones using syntactic extraction and masking.
pub fn extract_mentioned_pronoun(text: &str) -> &'static str {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "bạn";
    }

    let clean = strip_accents(&trimmed.to_lowercase());
    let clean_no_punc = RE_PUNCT.replace_all(&clean, " ");
    static RE_MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let clean_no_punc = RE_MULTI_SPACE.replace_all(&clean_no_punc, " ").trim().to_string();
    let clean_masked = mask_non_pronouns(&clean_no_punc);
    let norm_lower = trimmed.to_lowercase();
    let has_polite_a = Regex::new(r"\b(ạ|ah)\b").unwrap().is_match(&norm_lower);

    // 1. Collective mentions
    if RE_MENTIONED_COLLECTIVE.is_match(&clean_masked) {
        return "cả nhà";
    }

    // 2. @mentions accompanied by role vocative
    if let Some(caps) = RE_MENTIONED_AT_ROLE.captures(&clean_masked) {
        let role = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str()).unwrap_or("");
        return match role {
            "chi" => "chị",
            "thay" => "thầy",
            "bac" => "bác",
            "chu" => "chú",
            "co" => "cô",
            "sep" => "sếp",
            "anh" => "anh",
            "em" => "em",
            _ => "bạn",
        };
    }

    // 3. Direct Calls with "ơi": [Role] ơi
    for (pat, role) in RE_MENTIONED_OI.iter() {
        if pat.is_match(&clean_masked) {
            return role;
        }
    }

    // 4. Direct Speech Acts: (chào, cảm ơn, xin lỗi, gửi, hỏi, nhờ, chúc, cho, dặn) + [Addressee]
    for (pat, role) in RE_MENTIONED_SPEECH_ACTS.iter() {
        if pat.is_match(&clean_masked) {
            return role;
        }
    }

    // 5. Sentence endings: [Particle] [Role]
    if RE_MENTIONED_END_SEP.is_match(&clean_masked) { return "sếp"; }
    if RE_MENTIONED_END_THAY.is_match(&clean_masked) { return "thầy"; }
    if RE_MENTIONED_END_BAC.is_match(&clean_masked) { return "bác"; }
    if RE_MENTIONED_END_CHU.is_match(&clean_masked) { return "chú"; }
    if RE_MENTIONED_END_CO.is_match(&clean_masked) { return "cô"; }
    if RE_MENTIONED_END_CHAU.is_match(&clean_masked) { return "cháu"; }
    if RE_MENTIONED_END_CAU.is_match(&clean_masked) { return "cậu"; }
    if RE_MENTIONED_END_ONG.is_match(&clean_masked) { return "ông"; }
    if RE_MENTIONED_END_BA.is_match(&clean_masked) { return "bà"; }
    if has_polite_a {
        if RE_MENTIONED_END_ANH_POLITE.is_match(&clean_masked) { return "anh"; }
    } else {
        if RE_MENTIONED_END_ANH_NORMAL.is_match(&clean_masked) { return "anh"; }
    }
    if RE_MENTIONED_END_CHI.is_match(&clean_masked) { return "chị"; }
    if RE_MENTIONED_END_EM.is_match(&clean_masked) { return "em"; }

    // 6. Directives: [Addressee] [action] giúp [Sender]
    for (pat, role) in RE_MENTIONED_DIRECTIVES.iter() {
        if pat.is_match(&clean_masked) {
            return role;
        }
    }

    // 7. Inquiries about Addressee
    if RE_MENTIONED_INQUIRY.is_match(&clean_masked) {
        if Regex::new(r"\b(chu)\b").unwrap().is_match(&clean_masked) || RE_MENTIONED_CAC_CHU.is_match(&clean_masked) {
            return "chú";
        }
        if Regex::new(r"\b(bac)\b").unwrap().is_match(&clean_masked) {
            return "bác";
        }
        if Regex::new(r"\b(co)\b").unwrap().is_match(&clean_masked)
            && (norm_lower.contains("cô") || norm_lower.contains("co giao") || RE_MENTIONED_CO_SPECIAL.is_match(&clean_masked))
        {
            return "cô";
        }
        if Regex::new(r"\b(chau)\b").unwrap().is_match(&clean_masked) {
            return "cháu";
        }
        if Regex::new(r"\b(thay)\b").unwrap().is_match(&clean_masked) {
            return "thầy";
        }
        if Regex::new(r"\b(sep)\b").unwrap().is_match(&clean_masked) {
            return "sếp";
        }
        if Regex::new(r"\b(cau)\b").unwrap().is_match(&clean_masked) {
            return "cậu";
        }
        if Regex::new(r"\b(anh|a)\b").unwrap().is_match(&clean_masked) {
            return "anh";
        }
        if Regex::new(r"\b(chi|c)\b").unwrap().is_match(&clean_masked) {
            return "chị";
        }
        if Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean_masked) {
            return "em";
        }
    }

    // 8. Asymmetric Action Delegations
    if RE_MENTIONED_ASYM_ANH.is_match(&clean_masked) { return "anh"; }
    if RE_MENTIONED_ASYM_CHI.is_match(&clean_masked) { return "chị"; }
    if RE_MENTIONED_ASYM_SEP.is_match(&clean_masked) { return "sếp"; }
    if RE_MENTIONED_ASYM_EM.is_match(&clean_masked) && RE_MENTIONED_GIUP_ANH_CHI.is_match(&clean_masked) { return "em"; }

    // 9. Junior proposal
    let is_junior_proposal = Regex::new(r"\b(em thay|em nghi|em de xuat|theo em|em gop y|em bao cao|em xin y kien|em check thay|em thay ben|em hoi chut|em hoi ty)\b")
        .unwrap()
        .is_match(&clean_masked)
        || (Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean_masked)
            && (has_polite_a || Regex::new(r"\b(da|dạ)\b").unwrap().is_match(&norm_lower))
            && !Regex::new(r"\b(thay|bac|chu|co)\b").unwrap().is_match(&clean_masked));

    if is_junior_proposal {
        if Regex::new(r"\b(chi oi|c oi)\b").unwrap().is_match(&clean_masked)
            || (clean_masked.contains("chi") && !clean_masked.contains("anh"))
        {
            return "chị";
        }
        if Regex::new(r"\b(sep oi|boss oi|leader oi)\b").unwrap().is_match(&clean_masked) {
            return "sếp";
        }
        return "anh";
    }

    // Broadcast leader directive: "Anh em add vao suggest luon..."
    if Regex::new(r"\b(anh em)\b").unwrap().is_match(&clean_masked) {
        return "anh";
    }

    // 10. Self-referential statement (Absence / Status)
    let has_leave = Regex::new(r"(?i)\b(nghi|ban|vang|ve som|di muon|xin phep|om|met)\b").unwrap().is_match(&clean_masked);
    if has_leave {
        let has_em = Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean_masked);
        if Regex::new(r"\b(anh|a)\b").unwrap().is_match(&clean_masked) && !has_em {
            return "anh";
        }
        if Regex::new(r"\b(chi|c)\b").unwrap().is_match(&clean_masked) && !has_em {
            return "chị";
        }
        if has_em {
            return "em";
        }
        if Regex::new(r"\b(sep)\b").unwrap().is_match(&clean_masked) {
            return "sếp";
        }
    }

    // Standalone conversational mentions
    if Regex::new(r"\b(ca nha|moi nguoi)\b").unwrap().is_match(&clean_masked) {
        return "cả nhà";
    }
    if Regex::new(r"\b(sep|boss|leader)\b").unwrap().is_match(&clean_masked) {
        return "sếp";
    }
    if Regex::new(r"\b(thay)\b").unwrap().is_match(&clean_masked) && norm_lower.contains("thầy") {
        return "thầy";
    }
    if Regex::new(r"\b(bac)\b").unwrap().is_match(&clean_masked) && norm_lower.contains("bác") {
        return "bác";
    }
    if Regex::new(r"\b(chu)\b").unwrap().is_match(&clean_masked) && norm_lower.contains("chú") {
        return "chú";
    }
    if Regex::new(r"\b(co)\b").unwrap().is_match(&clean_masked) && norm_lower.contains("cô") {
        return "cô";
    }

    "bạn"
}

/// Context-aware reciprocal personal pronoun extraction.
pub fn extract_target_pronoun(previous_message: &str) -> &'static str {
    let clean_raw = strip_accents(&previous_message.to_lowercase());
    let clean = mask_non_pronouns(&clean_raw);
    let norm_lower = previous_message.to_lowercase();
    let has_polite_a = Regex::new(r"\b(ạ|ah)\b").unwrap().is_match(&norm_lower);

    // 0. Junior proposal check (e.g. "Chỗ assign manually, em thấy bên zoom đang như thế này ạ...")
    if Regex::new(r"\b(em thay|em nghi|em de xuat|theo em|em gop y|em bao cao|em xin y kien|em check thay|em thay ben|em hoi chut|em hoi ty)\b")
        .unwrap()
        .is_match(&clean)
        || (Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean)
            && (Regex::new(r"\b(a|ah|ạ)\b").unwrap().is_match(&norm_lower)
                || Regex::new(r"\b(da|dạ)\b").unwrap().is_match(&norm_lower))
            && !Regex::new(r"\b(thay oi|bac oi|chu oi|co oi)\b").unwrap().is_match(&clean))
    {
        if Regex::new(r"\b(chi oi|c oi)\b").unwrap().is_match(&clean)
            || (clean.contains("chi") && !clean.contains("anh"))
        {
            return "chị";
        }
        if Regex::new(r"\b(sep oi|boss oi|leader oi)\b").unwrap().is_match(&clean) {
            return "sếp";
        }
        return "anh";
    }

    // 1. Multi-participant / Team mentions
    let re_group = Regex::new(r"\b(ca nha|moi nguoi|team|mn|all|ca team|ae)\b").unwrap();
    let re_directive =
        Regex::new(r"\b(add|vao|luon|check|test|xong|nhe|gap|lam|trien khai)\b").unwrap();

    if re_group.is_match(&clean) && !clean.contains("anh em") {
        return "cả nhà";
    }

    // Directive broadcast: "Anh em add vao suggest luon" -> sender is older colleague / team lead ("anh")
    if clean.contains("anh em") && re_directive.is_match(&clean) {
        return "anh";
    }
    if clean.contains("anh em") {
        return "cả nhà";
    }

    // 2. Specific formal/elder vocatives
    if Regex::new(r"\b(sep|boss|leader|truong phong|giam doc)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "sếp";
    }
    if Regex::new(r"\b(thay|co giao|thay giao)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "thầy";
    }
    if Regex::new(r"\b(bac|bac oi)\b").unwrap().is_match(&clean) {
        return "bác";
    }
    if Regex::new(r"\b(chu|chu oi)\b").unwrap().is_match(&clean) {
        return "chú";
    }
    if (norm_lower.contains("cô")
        || norm_lower.contains("co giao")
        || Regex::new(r"\b(co oi|thua co|chao co|gui co|hoi co)\b")
            .unwrap()
            .is_match(&clean))
        && Regex::new(r"\b(co|co oi)\b").unwrap().is_match(&clean)
    {
        return "cô";
    }
    if Regex::new(r"\b(ong|ong oi)\b").unwrap().is_match(&clean) {
        return "ông";
    }
    if Regex::new(r"\b(ba|ba oi)\b").unwrap().is_match(&clean) {
        return "bà";
    }
    if Regex::new(r"\b(cau|cau oi|ban oi|may oi)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "cậu";
    }

    // 3. Reciprocal pairs ("Em ơi ... cho anh" -> address "anh")
    if Regex::new(r"\b(em oi|e oi|em)\b.*\b(cho anh|giup anh|gui anh|bao anh|anh)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "anh";
    }
    if Regex::new(r"\b(em oi|e oi|em)\b.*\b(cho chi|giup chi|gui chi|bao chi|chi)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "chị";
    }
    if Regex::new(r"\b(anh oi|a oi|anh)\b.*\b(cho em|giup em|gui em|bao em|em)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "em";
    }
    if Regex::new(r"\b(chi oi|c oi|chi)\b.*\b(cho em|giup em|gui em|bao em|em)\b")
        .unwrap()
        .is_match(&clean)
    {
        return "em";
    }

    // 4. Single vocatives
    if has_polite_a {
        if Regex::new(r"\b(anh oi)\b").unwrap().is_match(&clean) {
            return "em";
        }
    } else if Regex::new(r"\b(anh oi|a oi)\b").unwrap().is_match(&clean) {
        return "em";
    }
    if Regex::new(r"\b(chi oi|c oi)\b").unwrap().is_match(&clean) {
        return "em";
    }
    if Regex::new(r"\b(em oi|e oi)\b").unwrap().is_match(&clean) {
        return "anh";
    }
    if Regex::new(r"\b(xin chao|chao|hello|hi|alo|hey)\b")
        .unwrap()
        .is_match(&clean)
    {
        let calls_em = Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean);
        let calls_anh = if has_polite_a {
            Regex::new(r"\b(anh)\b").unwrap().is_match(&clean)
        } else {
            Regex::new(r"\b(anh|a)\b").unwrap().is_match(&clean)
        };
        let calls_chi = Regex::new(r"\b(chi|c)\b").unwrap().is_match(&clean);
        if calls_em && !calls_anh && !calls_chi {
            return "anh";
        }
        if calls_anh && !calls_em && !calls_chi {
            return "em";
        }
        if calls_chi && !calls_em && !calls_anh {
            return "em";
        }
    }
    if has_polite_a {
        if Regex::new(r"\b(anh)\b").unwrap().is_match(&clean) {
            return "anh";
        }
    } else if Regex::new(r"\b(anh|a)\b").unwrap().is_match(&clean) {
        return "anh";
    }
    if Regex::new(r"\b(chi|c)\b").unwrap().is_match(&clean) {
        return "chị";
    }
    if Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean) {
        return "em";
    }

    "bạn"
}

pub static PRONOUN_TEMPLATES: LazyLock<
    HashMap<&'static str, HashMap<&'static str, Vec<&'static str>>>,
> = LazyLock::new(|| {
    let mut root = HashMap::new();

    let mut anh = HashMap::new();
    anh.insert(
        "task_or_update",
        vec![
            "OK, anh.",
            "Dạ vâng anh.",
            "Để em add và test luôn.",
            "Nhất trí anh.",
            "Tuyệt vời anh ơi!",
        ],
    );
    anh.insert(
        "question",
        vec![
            "Dạ được anh.",
            "Dạ để em kiểm tra lại rồi báo anh nhé.",
            "Dạ chưa anh ơi.",
            "Dạ có anh nhé.",
        ],
    );
    anh.insert(
        "thanks",
        vec![
            "Dạ không có gì anh!",
            "Dạ không có chi anh.",
            "Em rất vui được hỗ trợ anh!",
        ],
    );
    anh.insert(
        "apology",
        vec![
            "Dạ không sao đâu anh!",
            "Dạ anh đừng bận tâm ạ.",
            "Không vấn đề gì đâu anh ạ.",
        ],
    );
    anh.insert(
        "greeting",
        vec!["Dạ em chào anh!", "Dạ em nghe đây anh.", "Chào anh ạ!"],
    );
    anh.insert(
        "general",
        vec![
            "OK, anh.",
            "Dạ vâng anh.",
            "Nhất trí anh.",
            "Dạ em rõ rồi anh.",
        ],
    );
    root.insert("anh", anh);

    let mut chi = HashMap::new();
    chi.insert(
        "task_or_update",
        vec![
            "OK, chị.",
            "Dạ vâng chị.",
            "Để em xử lý luôn chị nhé.",
            "Nhất trí chị.",
            "Dạ để em kiểm tra ngay.",
        ],
    );
    chi.insert(
        "question",
        vec![
            "Dạ được chị.",
            "Dạ để em kiểm tra lại rồi báo chị nhé.",
            "Dạ chưa chị ơi.",
            "Dạ có chị nhé.",
        ],
    );
    chi.insert(
        "thanks",
        vec![
            "Dạ không có gì chị!",
            "Dạ không có chi chị.",
            "Em rất vui được hỗ trợ chị!",
        ],
    );
    chi.insert(
        "apology",
        vec!["Dạ không sao đâu chị!", "Dạ chị đừng bận tâm ạ."],
    );
    chi.insert("greeting", vec!["Dạ em chào chị!", "Dạ em nghe đây chị."]);
    chi.insert("general", vec!["OK, chị.", "Dạ vâng chị.", "Nhất trí chị."]);
    root.insert("chị", chi);

    let mut em = HashMap::new();
    em.insert(
        "task_or_update",
        vec![
            "OK em.",
            "Để anh xem nhé.",
            "Nhất trí em.",
            "Được rồi em nhé.",
            "Làm tốt lắm em!",
        ],
    );
    em.insert(
        "question",
        vec![
            "Được em nhé.",
            "Để anh kiểm tra lại rồi báo em nhé.",
            "Chưa em ơi.",
            "Có em nhé.",
        ],
    );
    em.insert(
        "thanks",
        vec!["Không có gì em nhé!", "Giúp được em là tốt rồi!"],
    );
    em.insert(
        "apology",
        vec!["Không sao đâu em nhé!", "Đừng bận tâm em ơi."],
    );
    em.insert("greeting", vec!["Chào em nhé!", "Anh nghe đây em."]);
    em.insert(
        "general",
        vec!["OK em.", "Nhất trí em.", "Được rồi em nhé."],
    );
    root.insert("em", em);

    let mut sep = HashMap::new();
    sep.insert(
        "task_or_update",
        vec![
            "Dạ vâng sếp.",
            "Dạ em xử lý ngay ạ.",
            "Dạ em rõ rồi sếp.",
            "Dạ để em báo cáo sếp ngay khi xong ạ.",
        ],
    );
    sep.insert(
        "question",
        vec![
            "Dạ được sếp.",
            "Dạ để em kiểm tra lại rồi báo sếp ngay ạ.",
            "Dạ chưa xong ạ, em đang hoàn thiện.",
        ],
    );
    sep.insert(
        "thanks",
        vec!["Dạ không có gì sếp!", "Dạ là nhiệm vụ của em ạ."],
    );
    sep.insert(
        "apology",
        vec!["Dạ không sao đâu sếp ạ!", "Dạ sếp đừng bận tâm ạ."],
    );
    sep.insert("greeting", vec!["Dạ em chào sếp!", "Dạ em nghe đây sếp ạ."]);
    sep.insert(
        "general",
        vec!["Dạ vâng sếp.", "Dạ em rõ rồi sếp.", "Dạ vâng ạ."],
    );
    root.insert("sếp", sep);

    let mut ca_nha = HashMap::new();
    ca_nha.insert(
        "task_or_update",
        vec![
            "OK cả nhà.",
            "Nhất trí cả nhà.",
            "Tuyệt vời cả nhà ơi!",
            "Để mình xử lý luôn nhé.",
        ],
    );
    ca_nha.insert(
        "question",
        vec![
            "Được cả nhà nhé.",
            "Để mình kiểm tra rồi cập nhật cả nhà nhé.",
            "Chưa xong cả nhà ơi.",
        ],
    );
    ca_nha.insert(
        "thanks",
        vec![
            "Không có gì cả nhà ơi!",
            "Rất vui được đồng hành cùng cả nhà!",
        ],
    );
    ca_nha.insert(
        "apology",
        vec![
            "Không sao đâu cả nhà!",
            "Đừng lo nhé cả nhà, không vấn đề gì.",
        ],
    );
    ca_nha.insert(
        "greeting",
        vec!["Chào cả nhà!", "Chào mọi người nhé!", "Hello cả nhà!"],
    );
    ca_nha.insert(
        "general",
        vec!["OK cả nhà.", "Nhất trí cả nhà.", "Được cả nhà nhé."],
    );
    root.insert("cả nhà", ca_nha);

    let mut ban = HashMap::new();
    ban.insert(
        "task_or_update",
        vec![
            "OK bạn.",
            "Nhất trí.",
            "Để mình kiểm tra luôn.",
            "Tuyệt vời quá!",
        ],
    );
    ban.insert(
        "question",
        vec![
            "Được bạn nhé.",
            "Để mình kiểm tra lại nhé.",
            "Chưa bạn ơi.",
            "Có bạn nhé.",
        ],
    );
    ban.insert("thanks", vec!["Không có gì đâu bạn!", "Dạ không có chi."]);
    ban.insert(
        "apology",
        vec!["Không sao đâu bạn!", "Đừng lo nhé, không vấn đề gì."],
    );
    ban.insert(
        "greeting",
        vec!["Chào bạn!", "Mình đây, có chuyện gì thế?", "Alo bạn."],
    );
    ban.insert(
        "general",
        vec!["OK bạn.", "Dạ vâng.", "Nhất trí.", "Được bạn nhé."],
    );
    root.insert("bạn", ban);

    let mut cau = HashMap::new();
    cau.insert(
        "task_or_update",
        vec![
            "OK cậu.",
            "Nhất trí cậu.",
            "Để tớ xem luôn nhé.",
            "Được cậu nhé.",
        ],
    );
    cau.insert(
        "question",
        vec![
            "Được cậu nhé.",
            "Để tớ kiểm tra rồi báo cậu nhé.",
            "Chưa cậu ơi.",
        ],
    );
    cau.insert(
        "thanks",
        vec!["Không có gì đâu cậu!", "Giúp được cậu là vui rồi!"],
    );
    cau.insert(
        "apology",
        vec!["Không sao đâu cậu!", "Đừng lo nhé cậu, không vấn đề gì."],
    );
    cau.insert(
        "greeting",
        vec!["Chào cậu nhé!", "Tớ đây, có chuyện gì thế?"],
    );
    cau.insert("general", vec!["OK cậu.", "Nhất trí cậu.", "Được cậu nhé."]);
    root.insert("cậu", cau);

    let mut bac = HashMap::new();
    bac.insert(
        "task_or_update",
        vec!["Dạ vâng bác.", "Dạ để cháu xem ngay ạ.", "Nhất trí bác ạ."],
    );
    bac.insert(
        "question",
        vec!["Dạ được bác.", "Dạ để cháu kiểm tra lại rồi thưa bác ạ."],
    );
    bac.insert(
        "thanks",
        vec!["Dạ không có gì đâu bác!", "Dạ là việc cháu nên làm ạ!"],
    );
    bac.insert(
        "apology",
        vec!["Dạ không sao đâu bác!", "Dạ bác đừng bận tâm ạ."],
    );
    bac.insert(
        "greeting",
        vec!["Dạ cháu chào bác ạ!", "Dạ cháu nghe đây bác."],
    );
    bac.insert("general", vec!["Dạ vâng bác.", "Dạ được bác ạ."]);
    root.insert("bác", bac);

    root
});

fn detect_intent_heuristic(previous_message: &str, clean: &str) -> &'static str {
    let trimmed = previous_message.trim();
    let is_other = Regex::new(
        r"^(https?://|git\s+|npm\s+|curl\s+|docker\s+|error:|select\s+\*|const\s+|let\s+|var\s+)\b",
    )
    .unwrap()
    .is_match(trimmed)
        || Regex::new(r"^(thông báo:|thong bao:)")
            .unwrap()
            .is_match(trimmed)
        || Regex::new(r"^(so tai khoan|ma otp|so dien thoai|cccd)\b")
            .unwrap()
            .is_match(clean)
        || Regex::new(r"^(\d+[\s,.\d]*|\.\.\.|\?\?\?|[a-z0-9_./-]{30,})$")
            .unwrap()
            .is_match(clean);
    if is_other {
        return "other";
    }

    let is_leave = Regex::new(
        r"\b(nghi lam|nghi hoc|nghi phep|nghi viec|nghi trua|nghi ngoi|nghi le|nghi tet|nghi he|nghi om|xin nghi|tam nghi|ngay nghi|vang mat|di muon|den muon|ban viec|co viec ban|nghi nhe|nghi nha|ve som|tu daily|daily nhe|tu hop|tu sync|ban xiu|ban ti|ban chut|ban dot xuat)\b",
    )
        .unwrap()
        .is_match(clean);
    let is_yes_no = Regex::new(
        r"\b(khong|ko|k|chua|chx|duoc khong|duoc ko|duoc k|phai khong|phai ko|dung khong|dung ko|on khong|ok khong|ranh khong|ban khong|xong chua)\b",
    )
        .unwrap()
        .is_match(clean)
        || Regex::new(r"\b(co .* khong|co .* chua|di .* khong|an .* chua|duoc .* khong|co the .* khong)\b")
            .unwrap()
            .is_match(clean)
        || (previous_message.contains('?')
            && !Regex::new(
                r"\b(the nao|sao roi|may gio|khi nao|bao gio|o dau|tai sao|vi sao|nhu the nao|lam sao|cai gi)\b",
            )
                .unwrap()
                .is_match(clean));
    let is_task = !is_yes_no
        && (Regex::new(
            r"\b(add|xong|roi|luon|giup|check|test|deploy|gui|cap nhat|update|fix|sua|cung xong)\b",
        )
        .unwrap()
        .is_match(clean)
            || (Regex::new(r"\blam\b").unwrap().is_match(clean) && !is_leave));
    let is_question = Regex::new(
        r"\b(the nao|sao roi|may gio|khi nao|bao gio|o dau|tai sao|vi sao|nhu the nao|lam sao|cai gi)\b",
    )
        .unwrap()
        .is_match(clean)
        || (previous_message.contains('?') && !is_yes_no);
    let is_thanks = Regex::new(r"\b(cam on|tks|tk|thank|thanks|cmon|biet on)\b")
        .unwrap()
        .is_match(clean);
    let is_apology = Regex::new(r"\b(xin loi|sorry|sry|phien)\b")
        .unwrap()
        .is_match(clean);
    let is_greeting = Regex::new(r"^(hi|hello|alo|chao|xin chao)\b")
        .unwrap()
        .is_match(clean);
    let is_praise =
        Regex::new(r"\b(tuyet voi|qua da|hay qua|dinh|good|ok roi|ngon|tuyet|xuat sac)\b")
            .unwrap()
            .is_match(clean);

    if is_thanks {
        "thanks"
    } else if is_apology {
        "apology"
    } else if is_leave {
        "leave_or_absence"
    } else if is_yes_no {
        "yes_no_question"
    } else if is_question {
        "question"
    } else if is_task {
        "task_or_update"
    } else if is_greeting {
        "greeting"
    } else if is_praise {
        "praise"
    } else {
        "general"
    }
}

fn legacy_to_action(intent: &str) -> &'static str {
    match intent {
        "general" | "action_general_ack" | "reply_general" => "action_general_ack",
        "greeting" | "action_greeting_reply" | "reply_greeting" => "action_greeting_reply",
        "thanks" | "action_thanks_reply" | "reply_thanks" => "action_thanks_reply",
        "apology" | "action_apology_forgive" | "reply_apology" => "action_apology_forgive",
        "question" | "action_question_inquiry" | "reply_question" => "action_question_inquiry",
        "task_or_update" | "action_task_accept" | "reply_task_ack" | "reply_task_accept" => {
            "action_task_accept"
        }
        "leave_or_absence" | "action_leave_ack" | "reply_leave_ack" => "action_leave_ack",
        "praise" | "action_praise_receive" | "reply_praise" => "action_praise_receive",
        "other" | "action_no_reply_needed" | "reply_none" => "action_no_reply_needed",
        "yes_no_question" | "action_confirm_yes" | "reply_confirm_yes" => "action_confirm_yes",
        "health_inquiry" | "action_health_ack" | "reply_health" => "action_health_ack",
        "meal_or_social_invite" | "action_meal_social" | "reply_meal_social" => {
            "action_meal_social"
        }
        _ => "action_general_ack",
    }
}

fn party(pronoun: Option<&str>) -> Option<&'static str> {
    let raw = pronoun?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(normalized) = normalize_pronoun(Some(raw)) {
        return Some(normalized);
    }
    match strip_accents(raw).to_lowercase().as_str() {
        "chau" => Some("cháu"),
        "to" => Some("tớ"),
        "minh" => Some("mình"),
        _ => None,
    }
}

fn default_speaker(addressee: &str) -> &'static str {
    match addressee {
        "anh" | "chị" | "sếp" | "thầy" | "cả nhà" => "em",
        "bác" | "chú" | "cô" | "ông" | "bà" => "cháu",
        "cháu" => "chú",
        "em" => "anh",
        "cậu" => "tớ",
        "bạn" => "mình",
        "mình" => "bạn",
        _ => "mình",
    }
}

static RE_CHU: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(chu)\b").unwrap());
static RE_ANH_SPEAKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(anh oi|a oi|anh)\b").unwrap());
static RE_TASK_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(add|test|code|fix|bug|deploy|pr|commit)\b").unwrap());
static RE_WEDDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(an cuoi|dam cuoi|tiec cuoi)\b").unwrap());
static RE_HEALTH_HINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(khoe khong|khoe ko|khoe k|khoe chu|khoe ha|dao nay the nao|co khoe khong|co khoe ko|on khong|ok khong)\b",
    )
    .unwrap()
});

pub fn resolve_pronoun_pair(
    text: &str,
    addressee: Option<&str>,
    speaker: Option<&str>,
) -> (String, String) {
    let mut resolved_addr = party(addressee).map(str::to_string);
    let mut resolved_spk = party(speaker).map(str::to_string);
    let trimmed = text.trim();
    if resolved_addr.is_none() {
        if trimmed.is_empty() {
            resolved_addr = Some("bạn".to_string());
        } else {
            let clean = strip_accents(&trimmed.to_lowercase());
            if RE_CHU.is_match(&clean) {
                resolved_addr = Some("cháu".to_string());
                if resolved_spk.is_none() {
                    resolved_spk = Some("chú".to_string());
                }
            } else {
                resolved_addr = Some(extract_target_pronoun(text).to_string());
            }
        }
    }
    let mut addr = resolved_addr.unwrap_or_else(|| "bạn".to_string());

    let is_junior_proposal = !trimmed.is_empty()
        && {
            static RE_JUNIOR_PROP: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(r"\b(em thay|em nghi|em de xuat|theo em|em gop y|em bao cao|em xin y kien|em check thay|em thay ben|em hoi chut|em hoi ty)\b").unwrap()
            });
            static RE_EM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(em|e)\b").unwrap());
            static RE_POLITE_A: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"\b(a|ah|ạ)\b").unwrap());
            static RE_POLITE_DA: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"\b(da|dạ)\b").unwrap());
            static RE_ELDER_OI: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"\b(thay oi|bac oi|chu oi|co oi)\b").unwrap());

            let clean = strip_accents(&trimmed.to_lowercase());
            let norm_text = trimmed.to_lowercase();
            RE_JUNIOR_PROP.is_match(&clean)
                || (RE_EM.is_match(&clean)
                    && (RE_POLITE_A.is_match(&norm_text) || RE_POLITE_DA.is_match(&norm_text))
                    && !RE_ELDER_OI.is_match(&clean))
        };

    if is_junior_proposal && speaker.is_none() {
        let target_persona = addr.as_str();
        if matches!(target_persona, "anh" | "chị" | "sếp") {
            resolved_spk = Some(target_persona.to_string());
            addr = "em".to_string();
        }
    }

    if resolved_spk.is_none() && !trimmed.is_empty() {
        let clean = strip_accents(&trimmed.to_lowercase());
        if RE_ANH_SPEAKER.is_match(&clean) && (addr == "em" || addr == "chú") {
            resolved_spk = Some("anh".to_string());
        }
    }
    if resolved_spk.is_none() {
        resolved_spk = Some(default_speaker(&addr).to_string());
    }
    let mut spk = resolved_spk.unwrap_or_else(|| "mình".to_string());
    if spk == "chú" && addr == "em" {
        spk = "anh".to_string();
    }
    (spk, addr)
}

fn capitalize_sentences(rendered: &str) -> String {
    let mut out = String::with_capacity(rendered.len());
    let mut capitalize_next = true;
    for c in rendered.chars() {
        if capitalize_next && !c.is_whitespace() && c != '.' && c != '!' && c != '?' {
            out.extend(c.to_uppercase());
            capitalize_next = false;
            continue;
        }
        out.push(c);
        if c == '.' || c == '!' || c == '?' {
            capitalize_next = true;
        }
    }
    out
}

pub fn render_dynamic_template(template: &str, speaker: &str, addressee: &str) -> String {
    let mut spk = party(Some(speaker))
        .map(str::to_string)
        .unwrap_or_else(|| speaker.trim().to_lowercase());
    let addr = party(Some(addressee))
        .map(str::to_string)
        .unwrap_or_else(|| addressee.trim().to_lowercase());
    if spk == "chú" && addr == "em" {
        spk = "anh".to_string();
    } else if spk == "em" && addr == "chú" {
        spk = "cháu".to_string();
    }
    let rendered = template
        .replace("[X]", &addr)
        .replace("[x]", &addr)
        .replace("[Y]", &spk)
        .replace("[y]", &spk);
    capitalize_sentences(&rendered)
}

fn dynamic_templates(action: &str, speaker: &str, addressee: &str) -> &'static [&'static str] {
    let is_senior = matches!(speaker, "anh" | "chị" | "sếp") && matches!(addressee, "em" | "cháu");
    match action {
        "action_health_ack" if addressee == "sếp" || addressee == "thầy" => &[
            "Dạ [Y] cảm ơn [X], [Y] vẫn khỏe ạ.",
            "Dạ [Y] khỏe, cảm ơn [X] ạ.",
            "Dạ [Y] ổn, cảm ơn [X] quan tâm ạ.",
        ],
        "action_health_ack"
            if matches!(speaker, "em" | "cháu")
                && matches!(addressee, "anh" | "chị" | "bác" | "chú" | "cô") =>
        {
            &[
                "Cảm ơn [X], [Y] khỏe.",
                "Dạ [Y] khỏe, cảm ơn [X] nhé.",
                "[Y] vẫn khỏe, [X] thế nào?",
                "Dạ [Y] khỏe ạ.",
            ]
        }
        "action_health_ack" => &[
            "Cảm ơn [X], [Y] khỏe.",
            "[Y] khỏe, cảm ơn [X] nhé.",
            "Dạ [Y] khỏe, cảm ơn [X].",
            "[Y] vẫn khỏe, [X] thế nào?",
        ],
        "action_leave_ack" if matches!(speaker, "em" | "cháu") => &[
            "Dạ vâng [X].",
            "Dạ [X] cứ nghỉ ngơi nhé ạ.",
            "OK [X] nhé.",
            "Dạ [Y] rõ rồi [X].",
            "Dạ [X] nghỉ ngơi cho khỏe nhé ạ.",
        ],
        "action_leave_ack" => &[
            "OK [X] nhé.",
            "[X] cứ nghỉ ngơi nhé.",
            "Được rồi [X] nhé.",
            "[Y] rõ rồi [X].",
            "Nghỉ ngơi cho khỏe nhé [X]!",
        ],
        "action_meal_social" if matches!(speaker, "em" | "cháu") => &[
            "[Y] ăn rồi, cảm ơn [X].",
            "Dạ chưa [X], lát [Y] đi ăn.",
            "Đi chứ [X]!",
            "Có chứ [X], mai [Y] đi.",
        ],
        "action_meal_social" => &[
            "[Y] ăn rồi, cảm ơn [X].",
            "Chưa [X], lát [Y] đi ăn.",
            "Đi chứ [X]!",
            "Có chứ [X], mai [Y] đi.",
        ],
        "action_task_accept" if is_senior => &[
            "[Y] thấy cũng ổn đấy [X].",
            "OK [X], để [Y] xem nhé.",
            "[Y] thấy hợp lý đấy [X].",
            "Nhất trí [X] nhé.",
            "Được rồi [X] nhé.",
            "Để [Y] xem nhé [X].",
        ],
        "action_task_accept" if matches!(speaker, "em" | "cháu") => &[
            "OK, [X].",
            "Dạ vâng [X].",
            "Dạ để [Y] xử lý ngay ạ.",
            "Nhất trí [X].",
            "Dạ [Y] rõ rồi [X].",
            "Dạ để [Y] kiểm tra ngay ạ.",
        ],
        "action_task_accept" => &[
            "OK, [X].",
            "Dạ vâng [X].",
            "Dạ để [Y] xử lý ngay ạ.",
            "Nhất trí [X].",
            "Dạ để [Y] kiểm tra ngay.",
            "Để [Y] xem nhé [X].",
        ],
        "action_confirm_yes" if is_senior => &[
            "Có chứ [X].",
            "Được [X] nhé.",
            "[Y] có nhé.",
            "Chưa [X] ơi.",
            "Để [Y] xem lại đã nhé.",
        ],
        "action_confirm_yes" => &[
            "Có chứ [X].",
            "Dạ có [X].",
            "Dạ được [X].",
            "Được [X] nhé.",
            "Dạ chưa [X] ơi.",
        ],
        "action_thanks_reply" => &[
            "Dạ không có gì [X]!",
            "Dạ không có chi [X].",
            "[Y] rất vui được hỗ trợ [X]!",
            "Không có gì đâu [X]!",
            "Chuyện nhỏ mà [X].",
        ],
        "action_apology_forgive" => &[
            "Dạ không sao đâu [X]!",
            "Dạ [X] đừng bận tâm ạ.",
            "Không vấn đề gì đâu [X] ạ.",
            "Không sao đâu [X] nhé!",
        ],
        "action_greeting_reply" => &[
            "Dạ [Y] chào [X]!",
            "Dạ [Y] nghe đây [X].",
            "Chào [X] nhé!",
            "[Y] nghe đây [X].",
            "Alo [X]!",
        ],
        "action_praise_receive" => &[
            "Dạ [Y] cảm ơn [X] nhiều ạ!",
            "Cảm ơn [X] nhiều ạ!",
            "Tuyệt vời [X] ơi!",
        ],
        "action_question_inquiry" if is_senior => &[
            "Được [X] nhé.",
            "Để [Y] kiểm tra lại rồi báo [X] nhé.",
            "Chưa [X] ơi.",
            "Có [X] nhé.",
            "Để [Y] xem lại đã nhé [X].",
        ],
        "action_question_inquiry" => &[
            "Dạ được [X].",
            "Dạ để [Y] kiểm tra lại rồi báo [X] nhé.",
            "Dạ chưa [X] ơi.",
            "Dạ có [X] nhé.",
        ],
        _ if is_senior => &[
            "[Y] thấy cũng ổn đấy [X].",
            "OK [X], để [Y] xem nhé.",
            "[Y] thấy hợp lý đấy [X].",
            "Nhất trí [X] nhé.",
            "Được rồi [X] nhé.",
            "Để [Y] xem nhé [X].",
        ],
        _ => &[
            "OK, [X].",
            "Dạ vâng [X].",
            "Nhất trí [X].",
            "Dạ [Y] rõ rồi [X].",
        ],
    }
}

pub fn generate_dynamic_replies(
    action: &str,
    speaker: &str,
    addressee: &str,
    top_k: usize,
) -> Vec<String> {
    let action = legacy_to_action(action);
    if action == "action_no_reply_needed" || top_k == 0 {
        return Vec::new();
    }
    let (speaker, addressee) = resolve_pronoun_pair("", Some(addressee), Some(speaker));
    let mut rendered = Vec::new();
    for template in dynamic_templates(action, &speaker, &addressee) {
        let reply = render_dynamic_template(template, &speaker, &addressee);
        if !rendered.iter().any(|existing| existing == &reply) {
            rendered.push(reply);
        }
        if rendered.len() >= top_k {
            break;
        }
    }
    rendered
}

fn health_overlay(role: &str, speaker: &str) -> &'static [&'static str] {
    match (role, speaker) {
        ("cháu", _) => &[
            "Cảm ơn cháu, chú khỏe.",
            "Chú khỏe, cảm ơn cháu nhé.",
            "Dạ chú khỏe, cảm ơn cháu.",
            "Chú vẫn khỏe, cháu thế nào?",
        ],
        ("em", _) => &[
            "Cảm ơn em, anh khỏe.",
            "Anh khỏe, cảm ơn em nhé.",
            "Dạ cảm ơn em, anh khỏe.",
            "Anh vẫn khỏe, em thế nào?",
        ],
        ("anh", _) => &[
            "Cảm ơn anh, em khỏe.",
            "Dạ em khỏe, cảm ơn anh nhé.",
            "Em vẫn khỏe, anh thế nào?",
        ],
        ("chị", _) => &[
            "Cảm ơn chị, em khỏe.",
            "Dạ em khỏe, cảm ơn chị nhé.",
            "Em vẫn khỏe, chị thế nào?",
        ],
        ("sếp", _) => &[
            "Dạ em cảm ơn sếp, em vẫn khỏe ạ.",
            "Dạ em khỏe, cảm ơn sếp ạ.",
        ],
        ("thầy", _) => &[
            "Dạ em cảm ơn thầy, em vẫn khỏe ạ.",
            "Dạ em khỏe, cảm ơn thầy ạ.",
        ],
        ("cậu", _) => &[
            "Cảm ơn cậu, tớ khỏe.",
            "Tớ khỏe, cảm ơn cậu nhé.",
            "Tớ vẫn khỏe, cậu thế nào?",
        ],
        ("ông", _) => &["Cảm ơn ông, tôi khỏe.", "Tôi khỏe, cảm ơn ông nhé."],
        ("bà", _) => &["Cảm ơn bà, tôi khỏe.", "Tôi khỏe, cảm ơn bà nhé."],
        ("bác", _) => &["Dạ cảm ơn bác, cháu khỏe ạ.", "Dạ cháu khỏe, cảm ơn bác ạ."],
        ("chú", "anh") => &[
            "Cảm ơn chú, anh khỏe.",
            "Anh khỏe, cảm ơn chú nhé.",
            "Dạ anh khỏe, cảm ơn chú.",
            "Anh vẫn khỏe, chú thế nào?",
        ],
        ("chú", _) => &["Dạ cảm ơn chú, cháu khỏe ạ.", "Dạ cháu khỏe, cảm ơn chú ạ."],
        ("cô", _) => &["Dạ cảm ơn cô, cháu khỏe ạ.", "Dạ cháu khỏe, cảm ơn cô ạ."],
        ("cả nhà", _) => &[
            "Cảm ơn cả nhà, mình vẫn khỏe nhé!",
            "Cả nhà thế nào, mình vẫn khỏe.",
        ],
        _ => &[
            "Cảm ơn bạn, mình khỏe.",
            "Mình khỏe, cảm ơn bạn nhé.",
            "Mình vẫn khỏe, bạn thế nào?",
        ],
    }
}

fn wedding_reply(role: &str, speaker: &str) -> &'static str {
    if role == "chú" && speaker == "anh" {
        return "Có chứ chú, mai anh đi.";
    }
    match role {
        "anh" => "Có chứ anh, mai em đi.",
        "chị" => "Có chứ chị, mai em đi.",
        "em" => "Có chứ em, mai anh đi.",
        "sếp" => "Dạ có sếp, mai em đi ạ.",
        "cậu" => "Có chứ cậu, mai tớ đi.",
        "cả nhà" => "Có chứ cả nhà, mai mình đi.",
        _ => "Có chứ bạn, mai mình đi.",
    }
}

fn augment_quick_replies(
    replies: &mut Vec<String>,
    action: &str,
    role: &str,
    speaker: &str,
    clean: &str,
) {
    if action == "action_task_accept"
        && RE_TASK_CODE.is_match(clean)
        && matches!(speaker, "em" | "cháu")
    {
        let insert = match role {
            "anh" => "Để em add và test luôn.",
            "chị" => "Để em add và test luôn chị nhé.",
            _ => "",
        };
        if !insert.is_empty() && !replies.iter().any(|reply| reply == insert) && replies.len() >= 2
        {
            replies.insert(2, insert.to_string());
        }
    }
    if matches!(action, "action_meal_social" | "action_confirm_yes") && RE_WEDDING.is_match(clean) {
        let wedding = wedding_reply(role, speaker);
        if !replies.iter().any(|reply| reply == wedding) {
            replies.insert(0, wedding.to_string());
        }
    } else if matches!(
        action,
        "action_health_ack"
            | "action_confirm_yes"
            | "action_question_inquiry"
            | "action_greeting_reply"
    ) && RE_HEALTH_HINT.is_match(clean)
    {
        for reply in health_overlay(role, speaker).iter().rev() {
            if !replies.iter().any(|existing| existing == reply) {
                replies.insert(0, (*reply).to_string());
            }
        }
    }
}

pub fn suggest_quick_replies(
    previous_message: &str,
    pronoun_override: Option<&str>,
    top_k: usize,
    override_intent: Option<&str>,
) -> Vec<String> {
    if is_unreplyable_or_ack_message(previous_message) {
        return Vec::new();
    }

    if matches!(
        override_intent,
        Some("other" | "action_no_reply_needed" | "action_general_ack" | "reply_none" | "general")
    ) {
        return Vec::new();
    }

    let role = if let Some(r) = normalize_pronoun(pronoun_override) {
        r
    } else if previous_message.trim().is_empty() {
        return Vec::new();
    } else {
        extract_target_pronoun(previous_message)
    };

    if previous_message.trim().is_empty() {
        return Vec::new();
    }

    let clean = strip_accents(&previous_message.to_lowercase());
    let intent =
        override_intent.unwrap_or_else(|| detect_intent_heuristic(previous_message, &clean));
    let action = legacy_to_action(intent);
    if action == "action_no_reply_needed" || action == "action_general_ack" {
        return Vec::new();
    }

    let is_junior_proposal = {
        let norm_lower = previous_message.to_lowercase();
        Regex::new(r"\b(em thay|em nghi|em de xuat|theo em|em gop y|em bao cao|em xin y kien|em check thay|em thay ben|em hoi chut|em hoi ty)\b")
            .unwrap()
            .is_match(&clean)
            || (Regex::new(r"\b(em|e)\b").unwrap().is_match(&clean)
                && (Regex::new(r"\b(a|ah|ạ)\b").unwrap().is_match(&norm_lower)
                    || Regex::new(r"\b(da|dạ)\b").unwrap().is_match(&norm_lower))
                && !Regex::new(r"\b(thay oi|bac oi|chu oi|co oi)\b").unwrap().is_match(&clean))
    };

    if (action == "action_general" || intent == "general")
        && !is_junior_proposal
        && override_intent.is_none()
        && pronoun_override.is_none()
    {
        return Vec::new();
    }

    let (speaker, addressee) = resolve_pronoun_pair(previous_message, Some(role), None);
    let mut replies = generate_dynamic_replies(
        action,
        &speaker,
        &addressee,
        top_k.saturating_mul(2).max(top_k),
    );
    augment_quick_replies(&mut replies, action, &addressee, &speaker, &clean);
    replies.truncate(top_k);
    replies
}
