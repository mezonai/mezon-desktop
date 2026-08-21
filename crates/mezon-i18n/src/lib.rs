use std::collections::HashMap;
use std::sync::OnceLock;

fn data(locale: &str) -> &'static HashMap<String, String> {
    macro_rules! load {
        ($file:literal) => {{
            static CELL: OnceLock<HashMap<String, String>> = OnceLock::new();
            CELL.get_or_init(|| {
                serde_json::from_str(include_str!($file)).expect(concat!("Invalid ", $file))
            })
        }};
    }
    match locale {
        "vi" => load!("../../../assets/i18n/vi.json"),
        "ru" => load!("../../../assets/i18n/ru.json"),
        "es" => load!("../../../assets/i18n/es.json"),
        "tt" => load!("../../../assets/i18n/tt.json"),
        "de" => load!("../../../assets/i18n/de.json"),
        "it" => load!("../../../assets/i18n/it.json"),
        "pt" => load!("../../../assets/i18n/pt.json"),
        "jpn" => load!("../../../assets/i18n/jpn.json"),
        "kr" => load!("../../../assets/i18n/kr.json"),
        "swe" => load!("../../../assets/i18n/swe.json"),
        _ => load!("../../../assets/i18n/en.json"),
    }
}

/// Look up a translation key for the given locale.
///
/// Falls back to English if the key is missing in the requested locale,
/// and returns `key` itself if it is not found in English either.
pub fn t(locale: &str, key: &'static str) -> &'static str {
    if let Some(value) = data(locale).get(key) {
        return value.as_str();
    }
    if locale != "en"
        && let Some(value) = data("en").get(key)
    {
        return value.as_str();
    }
    key
}

#[cfg(test)]
mod tests {
    use super::t;

    const LOCALES: &[&str] = &[
        "en", "vi", "ru", "es", "tt", "de", "it", "pt", "jpn", "kr", "swe",
    ];

    #[test]
    fn resolves_per_locale() {
        assert_eq!(t("en", "common.settings"), "Settings");
        assert_eq!(t("vi", "common.settings"), "Cài đặt");
        assert_eq!(t("ru", "setting.language.title"), "Язык");
        assert_eq!(t("jpn", "setting.language.title"), "言語");
    }

    #[test]
    fn unknown_locale_falls_back_to_english() {
        assert_eq!(t("xx", "common.settings"), "Settings");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("en", "no.such.key"), "no.such.key");
        assert_eq!(t("vi", "no.such.key"), "no.such.key");
    }

    #[test]
    fn every_locale_bundle_loads() {
        for locale in LOCALES {
            assert_ne!(
                t(locale, "setting.language.title"),
                "setting.language.title",
                "locale {locale} failed to resolve setting.language.title"
            );
        }
    }

    #[test]
    fn language_names_are_autonyms() {
        assert_eq!(t("vi", "setting.language.korean"), "한국어");
        assert_eq!(t("de", "setting.language.swedish"), "Svenska");
        assert_eq!(t("kr", "setting.language.german"), "Deutsch");
    }

    #[test]
    fn archived_threads_section_label() {
        assert_eq!(t("en", "channelTopbar.archivedThreads"), "Archived Threads");
    }

    #[test]
    fn close_dm_confirm_resolves_in_every_locale() {
        for locale in LOCALES {
            for key in [
                "dmMessage.closeDmConfirm.title",
                "dmMessage.closeDmConfirm.content",
                "dmMessage.closeDmConfirm.confirmText",
                "dmMessage.closeDmConfirm.error",
            ] {
                assert_ne!(t(locale, key), key, "locale {locale} is missing {key}");
            }
        }
        assert_eq!(
            t("en", "dmMessage.closeDmConfirm.title"),
            "Close Direct Message"
        );
        assert_eq!(
            t("vi", "dmMessage.closeDmConfirm.title"),
            "Đóng cuộc trò chuyện"
        );
    }

    #[test]
    fn full_react_corpus_present() {
        assert_eq!(t("en", "clan.title"), "Customize Your Clan");
        assert_eq!(t("en", "channelCreator.monthsShort.0"), "JAN");
    }
}
