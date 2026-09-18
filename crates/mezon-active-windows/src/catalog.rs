use crate::info::normalize_process_name;

pub const ACTIVITY_TYPE_WORK: i32 = 1;
pub const ACTIVITY_TYPE_CODING: i32 = ACTIVITY_TYPE_WORK;
pub const ACTIVITY_TYPE_LIVE: i32 = 2;
pub const ACTIVITY_TYPE_PLAY: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Coding,
    Live,
    Play,
}

impl ActivityKind {
    pub fn as_type(self) -> i32 {
        match self {
            Self::Coding => ACTIVITY_TYPE_CODING,
            Self::Live => ACTIVITY_TYPE_LIVE,
            Self::Play => ACTIVITY_TYPE_PLAY,
        }
    }
}

struct CatalogEntry {
    aliases: &'static [&'static str],
    kind: ActivityKind,
}

const CODING_ALIASES: &[&str] = &[
    "Code",
    "Visual Studio Code",
    "Cursor",
    "Zed",
    "Xcode",
    "Sublime Text",
    "Atom",
    "Notepad",
    "CoffeeCup HTML Editor",
    "TextMate",
    "Bluefish",
    "Vim",
    "NetBeans",
    "Codeshare.io",
    "GNU Emacs",
    "Spacemacs",
    "BBEdit",
    "WebStorm",
    "UltraEdit",
    "Espresso",
    "Nova",
    "Unity",
    "Figma",
];

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        aliases: CODING_ALIASES,
        kind: ActivityKind::Coding,
    },
    CatalogEntry {
        aliases: &["Spotify"],
        kind: ActivityKind::Live,
    },
    CatalogEntry {
        aliases: &["LeagueClientUx", "League Of Legends"],
        kind: ActivityKind::Play,
    },
];

const LINUX_COMM_ALIASES: &[(&str, &str)] = &[
    ("sublime_text", "Sublime Text"),
    ("zed", "Zed"),
    ("codium", "Code"),
    ("vscodium", "Code"),
    ("webstorm", "WebStorm"),
    ("gvim", "Vim"),
    ("nvim", "Vim"),
    ("emacs", "GNU Emacs"),
    ("spacemacs", "Spacemacs"),
    ("netbeans", "NetBeans"),
    ("textmate", "TextMate"),
    ("bbedit", "BBEdit"),
    ("nova", "Nova"),
    ("espresso", "Espresso"),
    ("bluefish", "Bluefish"),
    ("unity", "Unity"),
    ("figma", "Figma"),
    ("xcode", "Xcode"),
];

const LINUX_PATH_HINTS: &[(&str, &str)] = &[
    ("/cursor/cursor", "Cursor"),
    ("/cursor.appimage", "Cursor"),
    ("/zed.appimage", "Zed"),
    ("/zed.app/", "Zed"),
    ("/lib/zed/", "Zed"),
    ("/share/zed/", "Zed"),
    ("/spotify/", "Spotify"),
    ("/code/code", "Code"),
    ("/visual studio code/", "Visual Studio Code"),
    ("/vscodium/", "Code"),
    ("/codium/", "Code"),
    ("/webstorm/", "WebStorm"),
    ("/sublime_text/", "Sublime Text"),
    ("/sublime-text/", "Sublime Text"),
    ("/jetbrains/webstorm", "WebStorm"),
    ("/unity/", "Unity"),
    ("/figma/", "Figma"),
    ("/xcode/", "Xcode"),
    ("/nova/", "Nova"),
    ("/netbeans/", "NetBeans"),
    ("/emacs/", "GNU Emacs"),
    ("/spacemacs/", "Spacemacs"),
];

pub fn is_coding_app(app_name: &str) -> bool {
    match_process_name(app_name).is_some_and(|(_, kind)| kind == ActivityKind::Coding)
}

pub fn classify_process_name(raw: &str) -> Option<ActivityKind> {
    match_process_name(raw).map(|(_, kind)| kind)
}

pub fn match_process_name(raw: &str) -> Option<(String, ActivityKind)> {
    let normalized = normalize_process_name(raw);
    if normalized.is_empty() {
        return None;
    }
    for entry in CATALOG {
        for alias in entry.aliases {
            if alias.eq_ignore_ascii_case(&normalized) {
                return Some((alias.to_string(), entry.kind));
            }
        }
    }
    None
}

pub fn match_linux_process(
    comm: &str,
    cmdline: &str,
    exe: Option<&str>,
) -> Option<(String, ActivityKind)> {
    if comm.eq_ignore_ascii_case("cursorsandbox") {
        return None;
    }
    if let Some(matched) = match_process_name(comm) {
        return Some(matched);
    }
    let comm_normalized = comm.replace('_', " ");
    if comm_normalized != comm
        && let Some(matched) = match_process_name(&comm_normalized)
    {
        return Some(matched);
    }
    for (comm_alias, catalog_alias) in LINUX_COMM_ALIASES {
        if comm.eq_ignore_ascii_case(comm_alias) {
            return match_process_name(catalog_alias);
        }
    }
    for line in [cmdline, exe.unwrap_or_default()] {
        if line.is_empty() {
            continue;
        }
        if let Some(matched) = match_linux_cmdline(line) {
            return Some(matched);
        }
    }
    None
}

fn match_linux_cmdline(line: &str) -> Option<(String, ActivityKind)> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("cursorsandbox") {
        return None;
    }
    for (needle, alias) in LINUX_PATH_HINTS {
        if lower.contains(needle) {
            return match_process_name(alias);
        }
    }
    None
}

pub fn pick_highest_priority_match(
    matches: impl IntoIterator<Item = (String, ActivityKind)>,
) -> Option<(String, ActivityKind)> {
    let mut best: Option<(String, ActivityKind, u8)> = None;
    for (name, kind) in matches {
        let priority = play_first_kind_priority(kind);
        let replace = best
            .as_ref()
            .map(|(_, _, current)| priority > *current)
            .unwrap_or(true);
        if replace {
            best = Some((name, kind, priority));
        }
    }
    best.map(|(name, kind, _)| (name, kind))
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn running_process_kind_priority(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Coding => 3,
        ActivityKind::Live => 2,
        ActivityKind::Play => 1,
    }
}

fn play_first_kind_priority(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Play => 3,
        ActivityKind::Coding => 2,
        ActivityKind::Live => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coding_aliases_include_electron_editors_and_zed() {
        let expected = [
            "Code",
            "Visual Studio Code",
            "Cursor",
            "Zed",
            "Xcode",
            "Sublime Text",
            "Atom",
            "Notepad",
            "CoffeeCup HTML Editor",
            "TextMate",
            "Bluefish",
            "Vim",
            "NetBeans",
            "Codeshare.io",
            "GNU Emacs",
            "Spacemacs",
            "BBEdit",
            "WebStorm",
            "UltraEdit",
            "Espresso",
            "Nova",
            "Unity",
            "Figma",
        ];
        assert_eq!(CODING_ALIASES, expected);
    }

    #[test]
    fn classifies_code_editors_as_coding() {
        assert_eq!(
            classify_process_name("Code.exe"),
            Some(ActivityKind::Coding)
        );
        assert_eq!(
            classify_process_name("Visual Studio Code"),
            Some(ActivityKind::Coding)
        );
        assert_eq!(classify_process_name("Cursor"), Some(ActivityKind::Coding));
        assert_eq!(classify_process_name("cursor"), Some(ActivityKind::Coding));
        assert_eq!(classify_process_name("Zed"), Some(ActivityKind::Coding));
        assert_eq!(classify_process_name("zed"), Some(ActivityKind::Coding));
    }

    #[test]
    fn classifies_spotify_and_lol() {
        assert_eq!(classify_process_name("Spotify"), Some(ActivityKind::Live));
        assert_eq!(
            classify_process_name("LeagueClientUx"),
            Some(ActivityKind::Play)
        );
    }

    #[test]
    fn rejects_unlisted_apps() {
        assert_eq!(classify_process_name("Google Chrome"), None);
        assert_eq!(classify_process_name(""), None);
    }

    #[test]
    fn match_linux_process_uses_cmdline_path_hints() {
        assert_eq!(
            match_linux_process("electron", "/opt/Cursor/cursor", None),
            Some(("Cursor".to_string(), ActivityKind::Coding))
        );
    }

    #[test]
    fn match_linux_process_maps_editor_comm_names() {
        assert_eq!(
            match_linux_process("sublime_text", "", None),
            Some(("Sublime Text".to_string(), ActivityKind::Coding))
        );
        assert_eq!(
            match_linux_process("codium", "", None),
            Some(("Code".to_string(), ActivityKind::Coding))
        );
        assert_eq!(
            match_linux_process("webstorm", "", None),
            Some(("WebStorm".to_string(), ActivityKind::Coding))
        );
        assert_eq!(
            match_linux_process("zed", "", None),
            Some(("Zed".to_string(), ActivityKind::Coding))
        );
        assert_eq!(
            match_linux_process("electron", "/usr/lib/zed/zed-editor", None),
            Some(("Zed".to_string(), ActivityKind::Coding))
        );
    }

    #[test]
    fn match_linux_cmdline_does_not_match_trailing_arguments() {
        assert!(match_linux_process("rg", "rg needle /home/me/Code", None).is_none());
    }

    #[test]
    fn running_process_priority_prefers_coding_over_play() {
        assert!(
            running_process_kind_priority(ActivityKind::Coding)
                > running_process_kind_priority(ActivityKind::Play)
        );
    }

    #[test]
    fn is_coding_app_recognizes_catalog_editors() {
        assert!(is_coding_app("Cursor"));
        assert!(is_coding_app("Zed"));
        assert!(is_coding_app("WebStorm"));
        assert!(!is_coding_app("Spotify"));
    }

    #[test]
    fn pick_highest_priority_match_prefers_play_over_coding_and_live() {
        let picked = pick_highest_priority_match([
            ("Spotify".to_string(), ActivityKind::Live),
            ("Code".to_string(), ActivityKind::Coding),
            ("LeagueClientUx".to_string(), ActivityKind::Play),
        ]);
        assert_eq!(
            picked,
            Some(("LeagueClientUx".to_string(), ActivityKind::Play))
        );
    }
}
