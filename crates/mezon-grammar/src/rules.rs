//! Rule-based dictionaries, spelling normalization, and Vietnamese phonotactics.
//! Provides exact, zero-latency corrections for teencode, confusable pairs, and diacritics.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrectionEdit {
    pub original: String,
    pub replacement: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CorrectionResult {
    pub original: String,
    pub corrected: String,
    pub has_errors: bool,
    pub edits: Vec<CorrectionEdit>,
}

/// Protected foreign and technology words that should never be mangled.
pub static FOREIGN_AND_TECH_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let words = [
        "ok",
        "oke",
        "okie",
        "oki",
        "okay",
        "hi",
        "hello",
        "hey",
        "bye",
        "goodbye",
        "thanks",
        "thank",
        "pls",
        "please",
        "sorry",
        "sry",
        "welcome",
        "help",
        "mezon",
        "grammar",
        "model",
        "mode",
        "suggest",
        "add",
        "app",
        "chat",
        "bot",
        "client",
        "server",
        "api",
        "ui",
        "ux",
        "web",
        "online",
        "offline",
        "channel",
        "code",
        "git",
        "commit",
        "push",
        "pull",
        "merge",
        "branch",
        "pr",
        "repo",
        "clone",
        "test",
        "testing",
        "runner",
        "check",
        "task",
        "bug",
        "fix",
        "fixed",
        "issue",
        "debug",
        "deploy",
        "build",
        "release",
        "update",
        "upgrade",
        "setup",
        "install",
        "config",
        "link",
        "file",
        "folder",
        "data",
        "database",
        "query",
        "table",
        "schema",
        "email",
        "mail",
        "inbox",
        "comment",
        "reply",
        "post",
        "feed",
        "call",
        "voice",
        "video",
        "screen",
        "share",
        "stream",
        "live",
        "record",
        "user",
        "admin",
        "member",
        "team",
        "group",
        "role",
        "permission",
        "account",
        "pass",
        "password",
        "login",
        "logout",
        "signin",
        "signup",
        "auth",
        "token",
        "status",
        "active",
        "pending",
        "done",
        "cancel",
        "success",
        "fail",
        "failed",
        "error",
        "log",
        "logs",
        "metric",
        "metrics",
        "benchmark",
        "latency",
        "memory",
        "cache",
        "cpu",
        "gpu",
        "ram",
        "kb",
        "mb",
        "gb",
        "tb",
        "byte",
        "bytes",
        "pixel",
        "view",
        "click",
        "tap",
        "input",
        "output",
        "button",
        "icon",
        "modal",
        "tab",
        "text",
        "image",
        "audio",
        "media",
        "svg",
        "png",
        "jpg",
        "jpeg",
        "gif",
        "json",
        "html",
        "css",
        "js",
        "ts",
        "py",
        "sh",
        "doc",
        "docs",
        "document",
        "pdf",
        "readme",
        "project",
        "work",
        "meeting",
        "meet",
        "agenda",
        "plan",
        "deadline",
        "kpi",
        "feedback",
        "review",
        "rate",
        "score",
        "vote",
        "poll",
        "survey",
        "game",
        "play",
        "player",
        "room",
        "lobby",
        "ping",
        "lag",
        "fps",
        "top",
        "pro",
        "vip",
        "max",
        "min",
        "auto",
        "manual",
        "custom",
        "default",
        "free",
        "premium",
        "price",
        "sale",
        "deal",
        "discount",
        "order",
        "bill",
        "pay",
        "desktop",
        "example",
        "sample",
        "electron",
        "browser",
        "backend",
        "frontend",
        "fullstack",
        "mobile",
        "ios",
        "android",
        "sdk",
        "cli",
        "gui",
        "unikey",
        "evkey",
        "vietkey",
        "openkey",
        "telex",
        "vni",
        "chip",
        "system",
    ];
    words.into_iter().collect()
});

pub static UNAMBIGUOUS_ENGLISH_MARKERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let markers = [
        "is", "are", "was", "were", "this", "that", "these", "those", "with", "from", "have",
        "has", "had", "will", "would", "could", "should", "you", "your", "they", "their", "what",
        "when", "where", "which", "been", "being", "does", "did", "doing", "about", "between",
        "through",
    ];
    markers.into_iter().collect()
});

pub static ENGLISH_COLLOCATIONS: LazyLock<HashSet<(&'static str, &'static str)>> =
    LazyLock::new(|| {
        let pairs = [
            ("the", "model"),
            ("on", "client"),
            ("good", "day"),
            ("have", "a"),
            ("in", "the"),
            ("of", "the"),
            ("to", "the"),
            ("for", "the"),
            ("with", "the"),
            ("on", "the"),
            ("at", "the"),
            ("from", "the"),
            ("thank", "you"),
            ("thanks", "for"),
            ("let", "me"),
            ("see", "you"),
        ];
        pairs.into_iter().collect()
    });

static RE_VN_DIACRITICS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[àáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđ]")
        .unwrap()
});

static RE_MULTISYLLABIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[aeiouyàáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵ]+[^aeiouyàáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵ\s]+[aeiouyàáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵ]+").unwrap()
});

fn has_double_consonant(w: &str) -> bool {
    let bytes = w.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i].is_ascii_alphabetic() && bytes[i] == bytes[i + 1] {
            return true;
        }
    }
    false
}

static RE_INVALID_VN_ENDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[bdfgklrsvxz]$").unwrap());

static RE_CONSONANT_CODA_CLUSTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(st|sk|sh|ct|pt|ft|lt|rt|ld|nd|rd|mp|nk|rk|ck|nt|ce|de|te|ge)$").unwrap()
});

static RE_CONSONANT_ONSET_CLUSTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(gr|cl|cr|pr|pl|fr|fl|br|bl|dr|st|sp|sk|sc|sm|sn|sw|str|spr|scr)").unwrap()
});

/// Vietnamese teencode and shorthand normalization mapping.
pub static TEENCODE_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    let pairs = [
        ("mik", "mình"),
        ("mềnh", "mình"),
        ("ng", "người"),
        ("ngừi", "người"),
        ("k", "không"),
        ("ko", "không"),
        ("kh", "không"),
        ("khum", "không"),
        ("hok", "không"),
        ("chx", "chưa"),
        ("wa", "quá"),
        ("lem", "lắm"),
        ("lun", "luôn"),
        ("cx", "cũng"),
        ("cug", "cũng"),
        ("nhìu", "nhiều"),
        ("nhiu", "nhiều"),
        ("dc", "được"),
        ("đc", "được"),
        ("lm", "làm"),
        ("bik", "biết"),
        ("ns", "nói"),
        ("thik", "thích"),
        ("thx", "cảm ơn"),
        ("iu", "yêu"),
        ("zui", "vui"),
        ("vuii", "vui"),
        ("z", "vậy"),
        ("dị", "vậy"),
        ("j", "gì"),
        ("ji", "gì"),
        ("saoo", "sao"),
        ("ntn", "như thế nào"),
        ("nma", "nhưng mà"),
        ("nvay", "như vậy"),
        ("vs", "với"),
        ("đag", "đang"),
        ("dag", "đang"),
        ("roii", "rồi"),
        ("hẻ", "hả"),
        ("ak", "à"),
        ("ah", "à"),
        ("uh", "ừ"),
        ("uk", "ừ"),
        ("uhm", "ừm"),
        ("um", "ừm"),
        ("hnay", "hôm nay"),
        ("hqa", "hôm qua"),
        ("bh", "bây giờ"),
        ("bjo", "bây giờ"),
        ("t2", "thứ hai"),
        ("t3", "thứ ba"),
        ("t4", "thứ tư"),
        ("t5", "thứ năm"),
        ("t6", "thứ sáu"),
        ("t7", "thứ bảy"),
        ("cn", "chủ nhật"),
        ("tks", "cảm ơn"),
        ("cmon", "cảm ơn"),
        ("camon", "cảm ơn"),
        ("stt", "status"),
        ("bthg", "bình thường"),
        ("bth", "bình thường"),
        ("snghi", "suy nghĩ"),
        ("ktra", "kiểm tra"),
        ("kcj", "không có gì"),
        ("kcg", "không có gì"),
        ("kb", "không biết"),
        ("kbit", "không biết"),
        ("kbiet", "không biết"),
        ("ngta", "người ta"),
        ("ngkhac", "người khác"),
        ("mn", "mọi người"),
        ("mng", "mọi người"),
        ("cban", "các bạn"),
        ("trc", "trước"),
        ("trog", "trong"),
        ("gđ", "gia đình"),
        ("ib", "inbox"),
        ("rep", "reply"),
        ("dth", "dễ thương"),
        ("dthuong", "dễ thương"),
        ("ck", "chồng"),
        ("vk", "vợ"),
        ("bn", "bạn"),
    ];
    for (k, v) in pairs {
        m.insert(k, v);
    }
    m
});

/// Confusable compound phrases, spelling fixes, and multi-word collocations.
pub static CONFUSABLE_PHRASES: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    let mut list = vec![
        ("ok ánh", "ok anh"),
        ("ok anh", "ok anh"),
        ("ok em", "ok em"),
        ("ok chi", "ok chị"),
        ("ok sep", "ok sếp"),
        ("ok sếp", "ok sếp"),
        ("ok ban", "ok bạn"),
        ("ok bạn", "ok bạn"),
        ("ok bac", "ok bác"),
        ("ok chu", "ok chú"),
        ("ok thay", "ok thầy"),
        ("ko can dung unikey nua", "không cần dùng unikey nữa"),
        ("khong can dung unikey nua", "không cần dùng unikey nữa"),
        ("can dung unikey nua", "cần dùng unikey nữa"),
        ("ko can dung unikey", "không cần dùng unikey"),
        ("khong can dung unikey", "không cần dùng unikey"),
        ("minh sua roi nhe", "mình sửa rồi nhé"),
        ("minh sua roi nhé", "mình sửa rồi nhé"),
        ("minh sưa roi nhe", "mình sửa rồi nhé"),
        ("minh sưa roi nhé", "mình sửa rồi nhé"),
        ("them dau tieng viet", "thêm dấu tiếng Việt"),
        ("can dung unikey", "cần dùng unikey"),
        ("dung unikey nua", "dùng unikey nữa"),
        ("ko can dung nua", "không cần dùng nữa"),
        ("khong can dung nua", "không cần dùng nữa"),
        ("thay chay co khop", "thấy chạy có khớp"),
        ("khop ben desktop", "khớp bên desktop"),
        ("ben desktop khong", "bên desktop không"),
        ("xem ben client", "xem bên client"),
        ("sua roi nhe", "sửa rồi nhé"),
        ("sua roi nhé", "sửa rồi nhé"),
        ("sưa roi nhe", "sửa rồi nhé"),
        ("sưa roi nhé", "sửa rồi nhé"),
        ("sưa rồi nhé", "sửa rồi nhé"),
        ("minh sua roi", "mình sửa rồi"),
        ("minh sưa roi", "mình sửa rồi"),
        ("di ngay nhe", "đi ngay nhé"),
        ("di ngay nhé", "đi ngay nhé"),
        ("đi ngay nhe", "đi ngay nhé"),
        ("di ngay", "đi ngay"),
        ("đi ngay", "đi ngay"),
        ("ngay nhe", "ngay nhé"),
        ("ngay nhé", "ngay nhé"),
        ("lam ngay", "làm ngay"),
        ("làm ngay", "làm ngay"),
        ("ve ngay", "về ngay"),
        ("về ngay", "về ngay"),
        ("ngay lap tuc", "ngay lập tức"),
        ("ngay bay gio", "ngay bây giờ"),
        ("dau tieng viet", "dấu tiếng Việt"),
        ("cau vua roi", "câu vừa rồi"),
        ("cầu vừa rồi", "câu vừa rồi"),
        ("chua can dung", "chưa cần dùng"),
        ("khong can dung", "không cần dùng"),
        ("ko can dung", "không cần dùng"),
        ("can dung nua", "cần dùng nữa"),
        ("chay co khop", "chạy có khớp"),
        ("desktop khong", "desktop không"),
        ("dung unikey", "dùng unikey"),
        ("tieng viet", "tiếng Việt"),
        ("minh sua", "mình sửa"),
        ("minh sưa", "mình sửa"),
        ("sua roi", "sửa rồi"),
        ("sưa roi", "sửa rồi"),
        ("sưa rồi", "sửa rồi"),
        ("thay check", "thầy check"),
        ("thày check", "thầy check"),
        ("thay chay", "thấy chạy"),
        ("vua roi", "vừa rồi"),
        ("co khop", "có khớp"),
        ("khop ben", "khớp bên"),
        ("can dung", "cần dùng"),
        ("dung de", "dùng để"),
        ("dung cho", "dùng cho"),
        ("dung duoc", "dùng được"),
        ("dung thu", "dùng thử"),
        ("dung tam", "dùng tạm"),
        ("dung nua", "dùng nữa"),
        ("dang dung", "đang dùng"),
        ("chua dung", "chưa dùng"),
        ("se dung", "sẽ dùng"),
        ("muon dung", "muốn dùng"),
        ("cach dung", "cách dùng"),
        ("de anh", "để anh"),
        ("mong cai", "mong cái"),
        ("sáng lạng", "xán lạn"),
        ("xáng lạng", "xán lạn"),
        ("sang lang", "xán lạn"),
        ("xang lan", "xán lạn"),
        ("xắp sếp", "sắp xếp"),
        ("sắp sếp", "sắp xếp"),
        ("xắp xếp", "sắp xếp"),
        ("xap sep", "sắp xếp"),
        ("xuất ăn", "suất ăn"),
        ("xuat an", "suất ăn"),
        ("sơ xuất", "sơ suất"),
        ("xơ xuất", "sơ suất"),
        ("chuẩn sát", "chuẩn xác"),
        ("chuan sat", "chuẩn xác"),
        ("sát thực", "xác thực"),
        ("cọ sát", "cọ xát"),
        ("đường xá", "đường sá"),
        ("phố sá", "phố xá"),
        ("suất xắc", "xuất sắc"),
        ("xuat sac", "xuất sắc"),
        ("xử lý", "xử lý"),
        ("sử lý", "xử lý"),
        ("chia xẻ", "chia sẻ"),
        ("xúc tích", "súc tích"),
        ("cha cứu", "tra cứu"),
        ("tra cuu", "tra cứu"),
        ("chò chuyện", "trò chuyện"),
        ("tro chuyen", "trò chuyện"),
        ("chăn chở", "trăn trở"),
        ("chau chuốt", "trau chuốt"),
        ("chau chuot", "trau chuốt"),
        ("trau truốt", "trau chuốt"),
        ("trân thành", "chân thành"),
        ("chan thanh", "chân thành"),
        ("trân trọng", "trân trọng"),
        ("dấu dếm", "giấu giếm"),
        ("dau dem", "giấu giếm"),
        ("giành dụm", "dành dụm"),
        ("dành giụm", "dành dụm"),
        ("rảnh dỗi", "rảnh rỗi"),
        ("ranh roi", "rảnh rỗi"),
        ("dông bão", "giông bão"),
        ("sữa chữa", "sửa chữa"),
        ("sua chua", "sửa chữa"),
        ("bàn cải", "bàn cãi"),
        ("ban cai", "bàn cãi"),
        ("chín mùi", "chín muồi"),
        ("bàng quang", "bàng quan"),
        ("đọc giả", "độc giả"),
        ("cám ơn", "cảm ơn"),
        ("du an moi", "dự án mới"),
        ("du an", "dự án"),
        ("hoi ve", "hỏi về"),
        ("he thong", "hệ thống"),
        ("chung ta", "chúng ta"),
        ("rat la rat", "rất"),
        ("rất là rất", "rất"),
        ("qua la qua", "quá"),
        ("quá là quá", "quá"),
        ("add vao", "add vào"),
        ("mode nay", "Mode này"),
        ("check task", "check task"),
        ("com trua", "cơm trưa"),
        ("an trua", "ăn trưa"),
        ("ca nha", "cả nhà"),
        ("voi sep", "với sếp"),
        ("di an", "đi ăn"),
        ("an com", "ăn cơm"),
        ("di lam", "đi làm"),
        ("rat vui", "rất vui"),
        ("rat ngon", "rất ngon"),
        ("rat tot", "rất tốt"),
        ("rat nhieu", "rất nhiều"),
        ("cac ban", "các bạn"),
        ("di hoc", "đi học"),
        ("di ve", "đi về"),
        ("ve nha", "về nhà"),
        ("di ngu", "đi ngủ"),
        ("ngu ngon", "ngủ ngon"),
        ("uong nuoc", "uống nước"),
        ("an sang", "ăn sáng"),
        ("an toi", "ăn tối"),
        ("chao ban", "chào bạn"),
        ("an bun", "ăn bún"),
        ("bun cha", "bún chả"),
        ("da thit", "da thịt"),
        ("ben hong", "bên hông"),
        ("bit tat", "bít tất"),
        ("tập chung", "tập trung"),
        ("nổ lực", "nỗ lực"),
        ("chia sẽ", "chia sẻ"),
        ("hiện thị", "hiển thị"),
        ("hổ trợ", "hỗ trợ"),
        ("lỗ hỏng", "lỗ hổng"),
        ("kiếm tra", "kiểm tra"),
        ("bài toàn", "bài toán"),
        ("sản phầm", "sản phẩm"),
        ("phát triền", "phát triển"),
        ("hiểu quả", "hiệu quả"),
        ("yêu tố", "yếu tố"),
        ("khoá luận", "khóa luận"),
        ("thức đẩy", "thúc đẩy"),
        ("chủ để", "chủ đề"),
        ("tuỳ theo", "tùy theo"),
        ("hài hoà", "hài hòa"),
        ("tác gỉa", "tác giả"),
        ("tổ chúc", "tổ chức"),
        ("khó khan", "khó khăn"),
        ("gia đoạn", "giai đoạn"),
        ("vận dung", "vận dụng"),
        ("dự trên", "dựa trên"),
        ("đều chỉnh", "điều chỉnh"),
        ("sức khoẻ", "sức khỏe"),
        ("thoả mãn", "thỏa mãn"),
        ("thõa mãn", "thỏa mãn"),
        ("thoả thuận", "thỏa thuận"),
        ("uỷ ban", "ủy ban"),
        ("toà án", "tòa án"),
        ("thuỷ sản", "thủy sản"),
        ("tích luỹ", "tích lũy"),
        ("qúa trình", "quá trình"),
        ("kết qủa", "kết quả"),
        ("dữ liêụ", "dữ liệu"),
        ("họat động", "hoạt động"),
        ("cở sở", "cơ sở"),
        ("xã hôị", "xã hội"),
        ("nôị dung", "nội dung"),
        ("nội dụng", "nội dung"),
        ("công nghê", "công nghệ"),
        ("phương tiên", "phương tiện"),
        ("đối tương", "đối tượng"),
        ("cán bô", "cán bộ"),
        ("quan hê", "quan hệ"),
        ("sử dung", "sử dụng"),
        ("hoạt đông", "hoạt động"),
        ("thực hiên", "thực hiện"),
        ("hê thống", "hệ thống"),
        ("hệ thông", "hệ thống"),
        ("xây dụng", "xây dựng"),
        ("tiêu dung", "tiêu dùng"),
        ("canh tranh", "cạnh tranh"),
        ("rõ rang", "rõ ràng"),
        ("sẵn sang", "sẵn sàng"),
        ("khách hang", "khách hàng"),
        ("thông kê", "thống kê"),
        ("con nguời", "con người"),
        ("chi phi", "chi phí"),
        ("sang tạo", "sáng tạo"),
        ("quy đinh", "quy định"),
        ("xác đinh", "xác định"),
        ("thực trang", "thực trạng"),
        ("gia tang", "gia tăng"),
        ("phạm vị", "phạm vi"),
        ("du lich", "du lịch"),
        ("đạo tạo", "đào tạo"),
        ("phản ảnh", "phản ánh"),
        ("vẫn đề", "vấn đề"),
        ("nghĩ làm", "nghỉ làm"),
        ("nghi lam", "nghỉ làm"),
        ("nghĩ học", "nghỉ học"),
        ("nghi hoc", "nghỉ học"),
        ("nghĩ phép", "nghỉ phép"),
        ("nghi phep", "nghỉ phép"),
        ("nghĩ việc", "nghỉ việc"),
        ("nghi viec", "nghỉ việc"),
        ("nghĩ trưa", "nghỉ trưa"),
        ("nghi trua", "nghỉ trưa"),
        ("nghĩ ngơi", "nghỉ ngơi"),
        ("nghi ngoi", "nghỉ ngơi"),
        ("nghĩ lễ", "nghỉ lễ"),
        ("nghi le", "nghỉ lễ"),
        ("nghĩ tết", "nghỉ tết"),
        ("nghi tet", "nghỉ tết"),
        ("nghĩ hè", "nghỉ hè"),
        ("nghi he", "nghỉ hè"),
        ("nghĩ ốm", "nghỉ ốm"),
        ("nghi om", "nghỉ ốm"),
        ("xin nghĩ", "xin nghỉ"),
        ("xin nghi", "xin nghỉ"),
        ("được nghĩ", "được nghỉ"),
        ("duoc nghi", "được nghỉ"),
        ("tạm nghĩ", "tạm nghỉ"),
        ("tam nghi", "tạm nghỉ"),
        ("ngày nghĩ", "ngày nghỉ"),
        ("ngay nghi", "ngày nghỉ"),
        ("giờ nghĩ", "giờ nghỉ"),
        ("gio nghi", "giờ nghỉ"),
        ("nhà nghĩ", "nhà nghỉ"),
        ("nha nghi", "nhà nghỉ"),
        ("suy nghỉ", "suy nghĩ"),
        ("nghỉ rằng", "nghĩ rằng"),
        ("nghỉ tới", "nghĩ tới"),
        ("nghỉ về", "nghĩ về"),
        ("nghỉ đến", "nghĩ đến"),
        ("nghỉ lại", "nghĩ lại"),
        ("nghỉ xem", "nghĩ xem"),
        ("nghỉ ra", "nghĩ ra"),
        ("anh nghi lam", "anh nghỉ làm"),
        ("em nghi lam", "em nghỉ làm"),
        ("mai nghi lam", "mai nghỉ làm"),
        ("nghi lam nhe", "nghỉ làm nhé"),
        ("nghi lam nha", "nghỉ làm nha"),
        ("mai anh nghi", "mai anh nghỉ"),
        ("sang mai nghi", "sáng mai nghỉ"),
        ("hom nay nghi", "hôm nay nghỉ"),
        ("cung xong", "cũng xong"),
        ("vay thui", "vậy thôi"),
        ("vay thoi", "vậy thôi"),
        ("vay thui nhe", "vậy thôi nhé"),
        ("vay thoi nhe", "vậy thôi nhé"),
        ("anh oi", "anh ơi"),
        ("em oi", "em ơi"),
        ("ban oi", "bạn ơi"),
        ("moi nguoi oi", "mọi người ơi"),
        ("ca nha oi", "cả nhà ơi"),
        ("giup em", "giúp em"),
        ("giup anh", "giúp anh"),
        ("cong viec", "công việc"),
        ("tot lanh", "tốt lành"),
        ("thanh cong", "thành công"),
        ("mot ngay", "một ngày"),
        ("chuc ban", "chúc bạn"),
        ("toi muon", "tôi muốn"),
        ("co the", "có thể"),
        ("chung ta", "chúng ta"),
        ("chung toi", "chúng tôi"),
        ("khong the", "không thể"),
        ("khong phai", "không phải"),
        ("bay gio", "bây giờ"),
        ("thoi gian", "thời gian"),
        ("tat ca", "tất cả"),
        ("bao gio", "bao giờ"),
        ("lam viec", "làm việc"),
        ("kiem tra", "kiểm tra"),
        ("van de", "vấn đề"),
        ("su dung", "sử dụng"),
        ("hoat dong", "hoạt động"),
        ("thong tin", "thông tin"),
        ("dien thoai", "điện thoại"),
        ("chuan bi", "chuẩn bị"),
        ("sinh vien", "sinh viên"),
        ("benh vien", "bệnh viện"),
        ("lien he", "liên hệ"),
        ("hop dong", "hợp đồng"),
        ("tai khoan", "tài khoản"),
        ("khach hang", "khách hàng"),
        ("tien do", "tiến độ"),
        ("bao cao", "báo cáo"),
        ("thong bao", "thông báo"),
        ("ung dung", "ứng dụng"),
        ("giao dien", "giao diện"),
        ("tai lieu", "tài liệu"),
        ("dong nghiep", "đồng nghiệp"),
        ("tin nhan", "tin nhắn"),
        ("tro chuyen", "trò chuyện"),
        ("cam thay", "cảm thấy"),
        ("tot hon", "tốt hơn"),
        ("xay ra", "xảy ra"),
        ("bat dau", "bắt đầu"),
        ("duoc roi", "được rồi"),
        ("san pham", "sản phẩm"),
        ("chat luong", "chất lượng"),
        ("phat trien", "phát triển"),
        ("chinh xac", "chính xác"),
        ("hoan thanh", "hoàn thành"),
        ("thuc hien", "thực hiện"),
        ("huong dan", "hướng dẫn"),
        ("yeu cau", "yêu cầu"),
    ];
    // Sort descending by target phrase length to greedily match longest collocations first
    list.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    list
});

pub struct CompiledConfusable {
    pub re: Regex,
    pub replacement: &'static str,
    pub multi_word: bool,
}

pub static COMPILED_CONFUSABLES: LazyLock<Vec<CompiledConfusable>> = LazyLock::new(|| {
    CONFUSABLE_PHRASES
        .iter()
        .map(|(wrong, right)| CompiledConfusable {
            re: Regex::new(&format!(r"(?i)\b{}\b", regex::escape(wrong))).unwrap(),
            replacement: right,
            multi_word: wrong.contains(' '),
        })
        .collect()
});

static RE_SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([,.\?!:;\]\)\}])").unwrap());
static RE_SPACE_AFTER_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([,.\?!:;])([^\s0-9,.\?!:;\'\x22\]\)\}])").unwrap());
static RE_MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").unwrap());
static RE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
static RE_FENCED_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```[\s\S]*?```").unwrap());
static RE_INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]+`").unwrap());
static RE_EMOJI_SHORTCODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":[A-Za-z0-9_+\-]+:").unwrap());
static RE_DOTTED_OR_AT_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\S*[.@]\S+").unwrap());
const LITERAL_MASK_START: char = '\u{E000}';
const LITERAL_MASK_END: char = '\u{E001}';
static RE_VN_DIACRITIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[àáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđÀÁẢÃẠĂẰẮẲẴẶÂẦẤẨẪẬÈÉẺẼẸÊỀẾỂỄỆÌÍỈĨỊÒÓỎÕỌÔỒỐỔỖỘƠỜỚỞỠỢÙÚỦŨỤƯỪỨỬỮỰỲÝỶỸỴĐ]").unwrap()
});

pub fn contains_vietnamese_diacritics(text: &str) -> bool {
    RE_VN_DIACRITIC.is_match(text)
}

pub(crate) fn protected_literal_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for re in [
        &*RE_FENCED_CODE,
        &*RE_URL,
        &*RE_INLINE_CODE,
        &*RE_EMOJI_SHORTCODE,
        &*RE_DOTTED_OR_AT_TOKEN,
    ] {
        for matched in re.find_iter(text) {
            spans.push((matched.start(), matched.end()));
        }
    }
    spans.sort_by_key(|span| span.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn span_overlaps_any(start: usize, end: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(from, to)| start < to && end > from)
}

fn literal_mask_placeholder(index: usize) -> String {
    format!("{LITERAL_MASK_START}{index}.{LITERAL_MASK_END}")
}

pub(crate) fn mask_protected_literals(text: &str) -> (String, Vec<String>) {
    let spans = protected_literal_spans(text);
    if spans.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut out = String::with_capacity(text.len());
    let mut originals = Vec::with_capacity(spans.len());
    let mut cursor = 0usize;
    for &(start, end) in &spans {
        if start < cursor || end > text.len() || start >= end {
            continue;
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        out.push_str(&text[cursor..start]);
        out.push_str(&literal_mask_placeholder(originals.len()));
        originals.push(text[start..end].to_string());
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    (out, originals)
}

pub(crate) fn unmask_protected_literals(text: &str, originals: &[String]) -> String {
    let mut current = text.to_string();
    for (index, original) in originals.iter().enumerate() {
        let placeholder = literal_mask_placeholder(index);
        current = current.replacen(&placeholder, original, 1);
    }
    current
}

fn rewrite_outside_spans(
    text: &str,
    spans: &[(usize, usize)],
    rewrite: impl Fn(&str) -> String,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for &(start, end) in spans {
        if start > cursor {
            out.push_str(&rewrite(&text[cursor..start]));
        }
        let start = start.max(cursor);
        if end > start {
            out.push_str(&text[start..end]);
            cursor = end;
        }
    }
    if cursor < text.len() {
        out.push_str(&rewrite(&text[cursor..]));
    }
    out
}

/// Case-matching helper to preserve casing style (UPPER, Capitalized, lower).
pub fn match_case(original: &str, replacement: &str) -> String {
    if original.is_empty() || replacement.is_empty() {
        return replacement.to_string();
    }
    let is_all_upper = original
        .chars()
        .all(|c| !c.is_alphabetic() || c.is_uppercase());
    if is_all_upper && original.chars().any(|c| c.is_alphabetic()) {
        return replacement.to_uppercase();
    }
    let first_char = original.chars().next().unwrap();
    if first_char.is_uppercase() {
        let mut chars = replacement.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => replacement.to_string(),
        }
    } else {
        replacement.to_string()
    }
}

pub fn is_foreign_or_tech_word(word: &str) -> bool {
    let w = word.trim().to_lowercase();
    if w.is_empty() {
        return false;
    }
    // 1. Known Vietnamese teencode abbreviations (t2, t3, mik, bik, thik, zui, etc.)
    if TEENCODE_MAP.contains_key(w.as_str()) {
        return false;
    }
    // 2. Alphanumeric / units (e.g. 447kb, v2, 100%)
    if w.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // 3. Contains Vietnamese diacritics
    if RE_VN_DIACRITICS.is_match(&w) {
        return false;
    }
    // 4. Known dictionary
    if FOREIGN_AND_TECH_WORDS.contains(w.as_str()) {
        return true;
    }
    // 4b. Common confusables or endings
    if w.ends_with("bg") {
        return false;
    }
    // 5. Non-Vietnamese alphabet letters
    if w.chars().any(|c| matches!(c, 'f' | 'j' | 'w' | 'z'))
        && !matches!(w.as_str(), "zui" | "wa" | "j" | "z")
    {
        return true;
    }
    // 5b. Double consonants (except oo, ee, uu)
    if has_double_consonant(&w)
        && !w.starts_with("oo")
        && !w.starts_with("ee")
        && !w.starts_with("uu")
    {
        return true;
    }
    // 6. Syllable endings
    if RE_INVALID_VN_ENDING.is_match(&w) && !w.ends_with("ng") {
        return true;
    }
    // 7. Coda cluster
    if RE_CONSONANT_CODA_CLUSTER.is_match(&w) {
        return true;
    }
    // 8. Onset cluster
    if RE_CONSONANT_ONSET_CLUSTER.is_match(&w) {
        return true;
    }
    // 9. Multisyllabic
    if RE_MULTISYLLABIC.is_match(&w) {
        return true;
    }
    false
}

pub fn is_predominantly_english_sentence(text: &str) -> bool {
    let lower = text.to_lowercase();
    if RE_VN_DIACRITICS.is_match(&lower) {
        return false;
    }

    let re_words = Regex::new(r"\b[a-zA-Z0-9_]+\b").unwrap();
    let words: Vec<&str> = re_words.find_iter(text).map(|m| m.as_str()).collect();
    if words.is_empty() {
        return false;
    }

    let tokens_lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut unambig_eng = 0;
    for w in &tokens_lower {
        if UNAMBIGUOUS_ENGLISH_MARKERS.contains(w.as_str()) {
            unambig_eng += 1;
        }
    }

    let mut bigram_match = false;
    for i in 0..tokens_lower.len().saturating_sub(1) {
        if ENGLISH_COLLOCATIONS.contains(&(tokens_lower[i].as_str(), tokens_lower[i + 1].as_str()))
        {
            bigram_match = true;
            break;
        }
    }

    let mut foreign_count = 0;
    for &w in &words {
        let w_lower = w.to_lowercase();
        if is_foreign_or_tech_word(w) || UNAMBIGUOUS_ENGLISH_MARKERS.contains(w_lower.as_str()) {
            foreign_count += 1;
        }
    }
    let ratio = foreign_count as f32 / words.len() as f32;

    if unambig_eng >= 2 {
        return true;
    }
    if unambig_eng >= 1 && (bigram_match || (foreign_count >= 2 && ratio >= 0.5)) {
        return true;
    }
    if bigram_match && words.len() <= 5 {
        return true;
    }
    if foreign_count >= 2 && ratio >= 0.65 {
        return true;
    }

    false
}

/// Normalizes spacing around punctuation marks.
pub fn normalize_punctuation_spacing(text: &str) -> (String, Vec<CorrectionEdit>) {
    let mut edits = Vec::new();
    let protected = protected_literal_spans(text);
    let step1 = rewrite_outside_spans(text, &protected, |part| {
        RE_SPACE_BEFORE_PUNCT.replace_all(part, "$1").into_owned()
    });
    let step2 = rewrite_outside_spans(&step1, &protected_literal_spans(&step1), |part| {
        RE_SPACE_AFTER_PUNCT.replace_all(part, "$1 $2").into_owned()
    });
    let cleaned = RE_MULTI_SPACE.replace_all(&step2, " ").trim().to_string();

    if cleaned != text.trim() {
        edits.push(CorrectionEdit {
            original: text.to_string(),
            replacement: cleaned.clone(),
            reason: "Chuẩn hóa khoảng trắng dấu câu".to_string(),
        });
    }
    (cleaned, edits)
}

/// Normalizes sentence capitalization after terminal punctuation.
pub fn normalize_sentence_capitalization(text: &str) -> (String, Vec<CorrectionEdit>) {
    let mut edits = Vec::new();
    let re = Regex::new(r"(^|[.?!]\s+)(\p{L})").unwrap();

    let mut capitalized = re
        .replace_all(text, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let letter = &caps[2];
            format!("{}{}", prefix, letter.to_uppercase())
        })
        .into_owned();
    for &(start, end) in &protected_literal_spans(text) {
        if start < end
            && end <= text.len()
            && end <= capitalized.len()
            && text.is_char_boundary(start)
            && text.is_char_boundary(end)
            && capitalized.is_char_boundary(start)
            && capitalized.is_char_boundary(end)
        {
            capitalized.replace_range(start..end, &text[start..end]);
        }
    }

    if capitalized != text {
        edits.push(CorrectionEdit {
            original: text.to_string(),
            replacement: capitalized.clone(),
            reason: "Viết hoa chữ cái đầu câu".to_string(),
        });
    }
    (capitalized, edits)
}

/// Tone placement standardizer (quy tắc đặt dấu mới: hòa, khỏe, thủy).
pub fn normalize_tone_placement(text: &str) -> (String, Vec<CorrectionEdit>) {
    let mut edits = Vec::new();
    let re_word = Regex::new(r"\b\p{L}+\b").unwrap();

    let result = re_word
        .replace_all(text, |caps: &regex::Captures| {
            let word = &caps[0];
            let lower = word.to_lowercase();
            let modern_map = [
                ("hoà", "hòa"),
                ("hoá", "hóa"),
                ("hoả", "hỏa"),
                ("hoã", "hõa"),
                ("hoạ", "họa"),
                ("toà", "tòa"),
                ("toá", "tóa"),
                ("toả", "tỏa"),
                ("toạ", "tọa"),
                ("thuỷ", "thủy"),
                ("thuý", "thúy"),
                ("thuỳ", "thùy"),
                ("thuỵ", "thụy"),
                ("khoẻ", "khỏe"),
                ("khoé", "khóe"),
            ];
            for (old_tone, new_tone) in modern_map {
                if lower == old_tone {
                    let rep = match_case(word, new_tone);
                    edits.push(CorrectionEdit {
                        original: word.to_string(),
                        replacement: rep.clone(),
                        reason: "Chuẩn hóa vị trí dấu thanh theo quy tắc mới".to_string(),
                    });
                    return rep;
                }
            }
            word.to_string()
        })
        .to_string();

    (result, edits)
}

/// Strips Vietnamese accents for normalization.
pub fn strip_accents(s: &str) -> String {
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

pub static COMMON_CANDIDATES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    let pairs = [
        ("noi", "nói"),
        ("tieng", "tiếng"),
        ("dau", "dấu"),
        ("co", "có"),
        ("hieu", "hiểu"),
        ("khong", "không"),
        ("toi", "tôi"),
        ("ban", "bạn"),
        ("dang", "đang"),
        ("lam", "làm"),
        ("viec", "việc"),
        ("di", "đi"),
        ("den", "đến"),
        ("ve", "về"),
        ("va", "và"),
        ("rat", "rất"),
        ("vui", "vui"),
        ("duoc", "được"),
        ("muon", "muốn"),
        ("thich", "thích"),
        ("phai", "phải"),
        ("nen", "nên"),
        ("chua", "chưa"),
        ("roi", "rồi"),
        ("luon", "luôn"),
        ("giup", "giúp"),
        ("nhe", "nhé"),
        ("se", "sẽ"),
        ("can", "cần"),
        ("dung", "đúng"),
        ("lan", "lần"),
        ("kiem", "kiểm"),
        ("nho", "nhớ"),
        ("truoc", "trước"),
        ("chuan", "chuẩn"),
        ("tra", "trả"),
        ("biet", "biết"),
        ("nghi", "nghĩ"),
        ("uong", "uống"),
        ("ngu", "ngủ"),
        ("chuc", "chúc"),
        ("mot", "một"),
        ("tot", "tốt"),
        ("lanh", "lành"),
        ("thanh", "thành"),
        ("cong", "công"),
        ("trua", "trưa"),
        ("sep", "sếp"),
        ("cung", "cũng"),
        ("minh", "mình"),
        ("cac", "các"),
        ("an", "ăn"),
        ("com", "cơm"),
        ("chan", "chán"),
        ("met", "mệt"),
        ("xau", "xấu"),
        ("vay", "vậy"),
        ("vi", "vì"),
        ("hoac", "hoặc"),
        ("ngoai", "ngoài"),
        ("nua", "nữa"),
        ("da", "đã"),
        ("ha", "hả"),
        ("oi", "ơi"),
        ("it", "ít"),
        ("cai", "cái"),
        ("vao", "vào"),
    ];
    for (k, v) in pairs {
        m.insert(k, v);
    }
    m
});

pub static COLLOCATIONS_MAP: LazyLock<
    HashMap<(&'static str, &'static str), (&'static str, &'static str)>,
> = LazyLock::new(|| {
    let mut m = HashMap::new();
    let pairs = [
        (("di", "lam"), ("đi", "làm")),
        (("rat", "vui"), ("rất", "vui")),
        (("cac", "ban"), ("các", "bạn")),
        (("cung", "xong"), ("cũng", "xong")),
        (("xong", "roi"), ("xong", "rồi")),
        (("vay", "thui"), ("vậy", "thôi")),
        (("vay", "thoi"), ("vậy", "thôi")),
        (("ve", "nha"), ("về", "nhà")),
        (("di", "ngu"), ("đi", "ngủ")),
        (("ngu", "ngon"), ("ngủ", "ngon")),
        (("uong", "nuoc"), ("uống", "nước")),
        (("di", "an"), ("đi", "ăn")),
        (("an", "com"), ("ăn", "cơm")),
        (("an", "bun"), ("ăn", "bún")),
        (("bun", "cha"), ("bún", "chả")),
        (("da", "thit"), ("da", "thịt")),
        (("ben", "hong"), ("bên", "hông")),
        (("bit", "tat"), ("bít", "tất")),
        (("noi", "tieng"), ("nói", "tiếng")),
        (("tieng", "viet"), ("tiếng", "Việt")),
        (("khong", "dau"), ("không", "dấu")),
        (("ko", "dau"), ("không", "dấu")),
        (("co", "dau"), ("có", "dấu")),
        (("co", "hieu"), ("có", "hiểu")),
        (("hieu", "khong"), ("hiểu", "không")),
        (("hieu", "ko"), ("hiểu", "không")),
        (("anh", "em"), ("anh", "em")),
        (("o", "dau"), ("ở", "đâu")),
        (("di", "dau"), ("đi", "đâu")),
        (("bat", "dau"), ("bắt", "đầu")),
        (("ban", "dau"), ("ban", "đầu")),
        (("dau", "tien"), ("đầu", "tiên")),
        (("dau", "cau"), ("dấu", "câu")),
        (("xin", "chao"), ("xin", "chào")),
        (("com", "trua"), ("cơm", "trưa")),
        (("an", "trua"), ("ăn", "trưa")),
        (("ca", "nha"), ("cả", "nhà")),
        (("voi", "sep"), ("với", "sếp")),
        (("cam", "on"), ("cảm", "ơn")),
        (("xin", "loi"), ("xin", "lỗi")),
        (("chuc", "mung"), ("chúc", "mừng")),
        (("tot", "lanh"), ("tốt", "lành")),
        (("lam", "viec"), ("làm", "việc")),
        (("tro", "chuyen"), ("trò", "chuyện")),
        (("tin", "nhan"), ("tin", "nhắn")),
        (("he", "thong"), ("hệ", "thống")),
        (("co", "the"), ("có", "thể")),
        (("chung", "ta"), ("chúng", "ta")),
        (("chung", "toi"), ("chúng", "tôi")),
        (("bay", "gio"), ("bây", "giờ")),
        (("thoi", "gian"), ("thời", "gian")),
        (("kiem", "tra"), ("kiểm", "tra")),
        (("van", "de"), ("vấn", "đề")),
        (("hop", "dong"), ("hợp", "đồng")),
        (("tai", "khoan"), ("tài", "khoản")),
        (("khach", "hang"), ("khách", "hàng")),
        (("tien", "do"), ("tiến", "độ")),
        (("bao", "cao"), ("báo", "cáo")),
        (("thong", "bao"), ("thông", "báo")),
        (("ung", "dung"), ("ứng", "dụng")),
        (("giao", "dien"), ("giao", "diện")),
        (("tai", "lieu"), ("tài", "liệu")),
        (("dong", "nghiep"), ("đồng", "nghiệp")),
        (("chao", "luon"), ("cháo", "lươn")),
        (("con", "luon"), ("con", "lươn")),
        (("thit", "luon"), ("thịt", "lươn")),
        (("luon", "leo"), ("lươn", "lẹo")),
        (("luon", "dong"), ("lươn", "đồng")),
        (("bay", "luon"), ("bay", "lượn")),
        (("luon", "lo"), ("lượn", "lờ")),
        (("luon", "quanh"), ("lượn", "quanh")),
        (("di", "luon"), ("đi", "luôn")),
        (("lam", "luon"), ("làm", "luôn")),
        (("ngay", "luon"), ("ngay", "luôn")),
        (("xong", "luon"), ("xong", "luôn")),
        (("di", "ngay"), ("đi", "ngay")),
        (("lam", "ngay"), ("làm", "ngay")),
        (("ngay", "nhe"), ("ngay", "nhé")),
        (("xong", "ngay"), ("xong", "ngay")),
        (("den", "ngay"), ("đến", "ngay")),
        (("ve", "ngay"), ("về", "ngay")),
        (("luon", "luon"), ("luôn", "luôn")),
        (("luon", "tien"), ("luôn", "tiện")),
        (("luon", "nhe"), ("luôn", "nhé")),
        (("suggest", "luon"), ("suggest", "luôn")),
        (("nay", "to"), ("này", "to")),
        (("to", "qua"), ("to", "quá")),
        (("rat", "ngon"), ("rất", "ngon")),
        (("anh", "ay"), ("anh", "ấy")),
        (("rat", "luon"), ("rất", "lươn")),
        // thui -> authentic 'thui'
        (("be", "thui"), ("bê", "thui")),
        (("mon", "be"), ("món", "bê")),
        (("thit", "thui"), ("thịt", "thui")),
        (("cho", "thui"), ("chó", "thui")),
        (("thui", "rom"), ("thui", "rơm")),
        (("thui", "chot"), ("thui", "chột")),
        (("thui", "thui"), ("thui", "thủi")),
        (("mot", "minh"), ("một", "mình")),
        (("rat", "thom"), ("rất", "thơm")),
        (("dung", "lam"), ("đừng", "làm")),
        (("tai", "nang"), ("tài", "năng")),
        (("nang", "tre"), ("năng", "trẻ")),
        // thui -> chat slang for 'thôi'
        (("vay", "thui"), ("vậy", "thôi")),
        (("the", "thui"), ("thế", "thôi")),
        (("nghi", "lam"), ("nghỉ", "làm")),
        (("nghi", "hoc"), ("nghỉ", "học")),
        (("nghi", "phep"), ("nghỉ", "phép")),
        (("nghi", "viec"), ("nghỉ", "việc")),
        (("nghi", "trua"), ("nghỉ", "trưa")),
        (("nghi", "ngoi"), ("nghỉ", "ngơi")),
        (("nghi", "le"), ("nghỉ", "lễ")),
        (("nghi", "tet"), ("nghỉ", "tết")),
        (("nghi", "he"), ("nghỉ", "hè")),
        (("nghi", "om"), ("nghỉ", "ốm")),
        (("nghi", "sinh"), ("nghỉ", "sinh")),
        (("nghi", "duong"), ("nghỉ", "dưỡng")),
        (("nghi", "mat"), ("nghỉ", "mát")),
        (("xin", "nghi"), ("xin", "nghỉ")),
        (("duoc", "nghi"), ("được", "nghỉ")),
        (("tam", "nghi"), ("tạm", "nghỉ")),
        (("ngay", "nghi"), ("ngày", "nghỉ")),
        (("gio", "nghi"), ("giờ", "nghỉ")),
        (("nha", "nghi"), ("nhà", "nghỉ")),
        (("nghi", "thui"), ("nghỉ", "thôi")),
        (("thui", "di"), ("thôi", "đi")),
        (("thui", "nhe"), ("thôi", "nhé")),
        (("thui", "nha"), ("thôi", "nha")),
        (("thui", "ma"), ("thôi", "mà")),
        (("di", "ma"), ("đi", "mà")),
        (("thui", "roi"), ("thôi", "rồi")),
        (("duoc", "thui"), ("được", "thôi")),
        (("dung", "thui"), ("đúng", "thôi")),
        (("xong", "thui"), ("xong", "thôi")),
        (("ve", "thui"), ("về", "thôi")),
        (("di", "thui"), ("đi", "thôi")),
        (("an", "thui"), ("ăn", "thôi")),
        (("lam", "thui"), ("làm", "thôi")),
        (("ngu", "thui"), ("ngủ", "thôi")),
        (("uong", "thui"), ("uống", "thôi")),
        // toi -> tối (evening / night / optimal / max)
        (("buoi", "toi"), ("buổi", "tối")),
        (("troi", "toi"), ("trời", "tối")),
        (("bong", "toi"), ("bóng", "tối")),
        (("chieu", "toi"), ("chiều", "tối")),
        (("dem", "toi"), ("đêm", "tối")),
        (("sang", "toi"), ("sáng", "tối")),
        (("toi", "nay"), ("tối", "nay")),
        (("toi", "qua"), ("tối", "qua")),
        (("toi", "mai"), ("tối", "mai")),
        (("toi", "hom"), ("tối", "hôm")),
        (("an", "toi"), ("ăn", "tối")),
        (("com", "toi"), ("cơm", "tối")),
        (("ve", "toi"), ("về", "tối")),
        (("luc", "toi"), ("lúc", "tối")),
        (("toi", "thui"), ("tối", "thui")),
        (("toi", "tam"), ("tối", "tăm")),
        (("toi", "uu"), ("tối", "ưu")),
        (("toi", "da"), ("tối", "đa")),
        (("toi", "thieu"), ("tối", "thiểu")),
        (("toi", "gian"), ("tối", "giản")),
        (("toi", "hau"), ("tối", "hậu")),
        (("toi", "co"), ("tối", "cổ")),
        (("den", "toi"), ("đen", "tối")),
        // toi -> tới (arrive / reach / to)
        (("cho", "toi"), ("cho", "tới")),
        (("di", "toi"), ("đi", "tới")),
        (("den", "toi"), ("đến", "tới")),
        (("tien", "toi"), ("tiến", "tới")),
        (("huong", "toi"), ("hướng", "tới")),
        (("nhin", "toi"), ("nhìn", "tới")),
        (("chay", "toi"), ("chạy", "tới")),
        (("ghe", "toi"), ("ghé", "tới")),
        (("tuan", "toi"), ("tuần", "tới")),
        (("thang", "toi"), ("tháng", "tới")),
        (("nam", "toi"), ("năm", "tới")),
        (("lan", "toi"), ("lần", "tới")),
        (("dan", "toi"), ("dẫn", "tới")),
        (("mang", "toi"), ("mang", "tới")),
        (("dua", "toi"), ("đưa", "tới")),
        (("chuyen", "toi"), ("chuyển", "tới")),
        (("gui", "toi"), ("gửi", "tới")),
        (("quay", "toi"), ("quay", "tới")),
        (("bay", "toi"), ("bay", "tới")),
        (("nghi", "toi"), ("nghĩ", "tới")),
        (("nhac", "toi"), ("nhắc", "tới")),
        (("tim", "toi"), ("tìm", "tòi")),
        // toi -> tội (crime / guilt / pity)
        (("pham", "toi"), ("phạm", "tội")),
        (("co", "toi"), ("có", "tội")),
        (("toi", "nghiep"), ("tội", "nghiệp")),
        (("toi", "pham"), ("tội", "phạm")),
        (("mac", "toi"), ("mắc", "tội")),
        (("dac", "toi"), ("đắc", "tội")),
        (("vo", "toi"), ("vô", "tội")),
        (("chuoc", "toi"), ("chuộc", "tội")),
        (("tong", "toi"), ("tống", "tội")),
        (("tha", "toi"), ("tha", "tội")),
        (("ket", "toi"), ("kết", "tội")),
        (("ganh", "toi"), ("gánh", "tội")),
        (("nhan", "toi"), ("nhận", "tội")),
        (("biet", "toi"), ("biết", "tội")),
        (("ta", "toi"), ("tạ", "tội")),
        (("chiu", "toi"), ("chịu", "tội")),
        (("dinh", "toi"), ("định", "tội")),
        (("luan", "toi"), ("luận", "tội")),
        (("toi", "loi"), ("tội", "lỗi")),
        (("toi", "do"), ("tội", "đồ")),
        (("toi", "va"), ("tội", "vạ")),
        (("mang", "toi"), ("mang", "tội")),
        (("phat", "toi"), ("phạt", "tội")),
        (("toi", "ac"), ("tội", "ác")),
        (("toi", "tinh"), ("tội", "tình")),
        (("toi", "trang"), ("tội", "trạng")),
        // toi -> authentic 'toi' (dead / ruined)
        (("chet", "toi"), ("chết", "toi")),
        (("toi", "mang"), ("toi", "mạng")),
        (("toi", "doi"), ("toi", "đời")),
        (("toi", "cong"), ("toi", "công")),
        (("toi", "com"), ("toi", "cơm")),
        (("dich", "toi"), ("dịch", "toi")),
        (("ga", "toi"), ("gà", "toi")),
        (("mat", "toi"), ("mất", "toi")),
        // toi -> tôi (pronoun)
        (("toi", "muon"), ("tôi", "muốn")),
        (("toi", "nghi"), ("tôi", "nghĩ")),
        (("toi", "biet"), ("tôi", "biết")),
        (("toi", "thay"), ("tôi", "thấy")),
        (("toi", "hieu"), ("tôi", "hiểu")),
        (("toi", "lam"), ("tôi", "làm")),
        (("toi", "di"), ("tôi", "đi")),
        (("toi", "an"), ("tôi", "ăn")),
        (("toi", "ve"), ("tôi", "về")),
        (("toi", "o"), ("tôi", "ở")),
        (("toi", "xin"), ("tôi", "xin")),
        (("toi", "co"), ("tôi", "có")),
        (("toi", "can"), ("tôi", "cần")),
        (("toi", "da"), ("tôi", "đã")),
        (("toi", "se"), ("tôi", "sẽ")),
        (("toi", "dang"), ("tôi", "đang")),
        (("toi", "cung"), ("tôi", "cũng")),
        (("toi", "thich"), ("tôi", "thích")),
        (("toi", "yeu"), ("tôi", "yêu")),
        (("toi", "khong"), ("tôi", "không")),
        (("toi", "chua"), ("tôi", "chưa")),
        (("chung", "toi"), ("chúng", "tôi")),
        (("cua", "toi"), ("của", "tôi")),
        (("voi", "toi"), ("với", "tôi")),
        (("minh", "sua"), ("mình", "sửa")),
        (("sua", "roi"), ("sửa", "rồi")),
        (("sua", "nhe"), ("sửa", "nhé")),
        (("sua", "xong"), ("sửa", "xong")),
        (("sua", "lai"), ("sửa", "lại")),
        (("sua", "code"), ("sửa", "code")),
        (("sua", "bug"), ("sửa", "bug")),
        (("sua", "loi"), ("sửa", "lỗi")),
        (("em", "sua"), ("em", "sửa")),
        (("anh", "sua"), ("anh", "sửa")),
        (("thay", "check"), ("thầy", "check")),
        (("thay", "chay"), ("thấy", "chạy")),
        (("cau", "vua"), ("câu", "vừa")),
        (("vua", "roi"), ("vừa", "rồi")),
        (("xem", "ben"), ("xem", "bên")),
        (("ben", "client"), ("bên", "client")),
        (("ben", "desktop"), ("bên", "desktop")),
        (("chay", "co"), ("chạy", "có")),
        (("co", "khop"), ("có", "khớp")),
        (("khop", "ben"), ("khớp", "bên")),
        (("can", "dung"), ("cần", "dùng")),
        (("dung", "unikey"), ("dùng", "unikey")),
        (("dung", "de"), ("dùng", "để")),
        (("dung", "cho"), ("dùng", "cho")),
        (("dung", "duoc"), ("dùng", "được")),
        (("dung", "thu"), ("dùng", "thử")),
        (("dung", "tam"), ("dùng", "tạm")),
        (("dung", "nua"), ("dùng", "nữa")),
        (("dang", "dung"), ("đang", "dùng")),
        (("chua", "dung"), ("chưa", "dùng")),
        (("se", "dung"), ("sẽ", "dùng")),
        (("muon", "dung"), ("muốn", "dùng")),
        (("cach", "dung"), ("cách", "dùng")),
        (("de", "dung"), ("dễ", "dùng")),
        (("kho", "dung"), ("khó", "dùng")),
        (("tap", "dung"), ("tập", "dùng")),
        (("thich", "dung"), ("thích", "dùng")),
        (("quen", "dung"), ("quen", "dùng")),
        (("it", "dung"), ("ít", "dùng")),
        (("hay", "dung"), ("hay", "dùng")),
        (("de", "anh"), ("để", "anh")),
        (("mong", "cai"), ("mong", "cái")),
        (("ok", "anh"), ("ok", "anh")),
        (("ok", "em"), ("ok", "em")),
        (("ok", "chi"), ("ok", "chị")),
        (("ok", "sep"), ("ok", "sếp")),
        (("ok", "ban"), ("ok", "bạn")),
        (("ok", "bac"), ("ok", "bác")),
        (("ok", "chu"), ("ok", "chú")),
        (("ok", "thay"), ("ok", "thầy")),
        (("ok", "nhe"), ("ok", "nhé")),
    ];
    for (k, v) in pairs {
        m.insert(k, v);
    }
    m
});

pub fn restore_diacritics(text: &str) -> (String, Vec<CorrectionEdit>) {
    if is_predominantly_english_sentence(text) {
        return (text.to_string(), Vec::new());
    }

    let re_token = Regex::new(r"(\p{L}+|[^\p{L}]+)").unwrap();
    let protected = protected_literal_spans(text);
    let mut tokens: Vec<String> = Vec::new();
    let mut word_indices = Vec::new();
    let re_word = Regex::new(r"^\p{L}+$").unwrap();
    for matched in re_token.find_iter(text) {
        let token = matched.as_str().to_string();
        if re_word.is_match(&token)
            && !is_foreign_or_tech_word(&token)
            && !span_overlaps_any(matched.start(), matched.end(), &protected)
        {
            word_indices.push(tokens.len());
        }
        tokens.push(token);
    }

    if word_indices.is_empty() {
        return (text.to_string(), Vec::new());
    }

    let orig_tokens = tokens.clone();
    let mut matched_indices = std::collections::HashSet::new();

    // 1. Collocations pass
    if word_indices.len() >= 2 {
        for k in 0..word_indices.len() - 1 {
            let i1 = word_indices[k];
            let i2 = word_indices[k + 1];
            let s1 = strip_accents(&orig_tokens[i1]).to_lowercase();
            let s2 = strip_accents(&orig_tokens[i2]).to_lowercase();
            if let Some(&(w1, w2)) = COLLOCATIONS_MAP.get(&(s1.as_str(), s2.as_str())) {
                tokens[i1] = match_case(&tokens[i1], w1);
                tokens[i2] = match_case(&tokens[i2], w2);
                matched_indices.insert(i1);
                matched_indices.insert(i2);
            }
        }
    }

    // 2. Candidate unaccented word pass
    for &idx in &word_indices {
        if matched_indices.contains(&idx) {
            continue;
        }
        let s = strip_accents(&orig_tokens[idx]).to_lowercase();
        if tokens[idx].to_lowercase() == s {
            if let Some(&cand) = COMMON_CANDIDATES.get(s.as_str()) {
                tokens[idx] = match_case(&tokens[idx], cand);
            }
        }
    }

    let result = tokens.concat();
    let mut edits = Vec::new();
    let mut k = 0;
    while k < word_indices.len() {
        let idx = word_indices[k];
        if k < word_indices.len() - 1 {
            let next_idx = word_indices[k + 1];
            let s1 = strip_accents(&orig_tokens[idx]).to_lowercase();
            let s2 = strip_accents(&orig_tokens[next_idx]).to_lowercase();
            if COLLOCATIONS_MAP.contains_key(&(s1.as_str(), s2.as_str()))
                && (orig_tokens[idx] != tokens[idx] || orig_tokens[next_idx] != tokens[next_idx])
            {
                let orig_colloc = format!("{} {}", orig_tokens[idx], orig_tokens[next_idx]);
                let new_colloc = format!("{} {}", tokens[idx], tokens[next_idx]);
                edits.push(CorrectionEdit {
                    original: orig_colloc.clone(),
                    replacement: new_colloc.clone(),
                    reason: format!(
                        "Khôi phục dấu theo cụm từ ('{}' -> '{}')",
                        orig_colloc, new_colloc
                    ),
                });
                k += 2;
                continue;
            }
        }
        if orig_tokens[idx] != tokens[idx] {
            edits.push(CorrectionEdit {
                original: orig_tokens[idx].clone(),
                replacement: tokens[idx].clone(),
                reason: format!(
                    "Thêm dấu tiếng Việt ('{}' -> '{}')",
                    orig_tokens[idx], tokens[idx]
                ),
            });
        }
        k += 1;
    }

    (result, edits)
}
