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
        "ukr" => load!("../../../assets/i18n/ukr.json"),
        "pl" => load!("../../../assets/i18n/pl.json"),
        "nl" => load!("../../../assets/i18n/nl.json"),
        "fr" => load!("../../../assets/i18n/fr.json"),
        "blr" => load!("../../../assets/i18n/blr.json"),
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

pub fn api_error(locale: &str, code: u32) -> &'static str {
    const KEYS: [&str; 17] = [
        "errors.error_0",
        "errors.error_1",
        "errors.error_2",
        "errors.error_3",
        "errors.error_4",
        "errors.error_5",
        "errors.error_6",
        "errors.error_7",
        "errors.error_8",
        "errors.error_9",
        "errors.error_10",
        "errors.error_11",
        "errors.error_12",
        "errors.error_13",
        "errors.error_14",
        "errors.error_15",
        "errors.error_16",
    ];
    let key = KEYS.get(code as usize).copied().unwrap_or(KEYS[2]);
    t(locale, key)
}

#[cfg(test)]
mod tests {
    use super::{data, t};

    const LOCALES: &[&str] = &[
        "en", "vi", "ru", "ukr", "es", "tt", "de", "it", "pt", "jpn", "pl", "kr", "swe", "blr",
        "fr", "nl",
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
    fn api_error_maps_status_codes_and_clamps_unknown_ones() {
        assert_eq!(super::api_error("en", 7), "Permission denied");
        assert_eq!(
            super::api_error("en", 16),
            "Session expired. Please log in again."
        );
        assert_eq!(super::api_error("en", 99), "Unknown error");
        assert_ne!(super::api_error("vi", 7), super::api_error("en", 7));
        for locale in LOCALES {
            for code in 0..=16u32 {
                let message = super::api_error(locale, code);
                assert!(
                    !message.starts_with("errors.error_"),
                    "{locale} code {code} fell through to the raw key"
                );
            }
        }
    }

    #[test]
    fn every_locale_carries_every_english_key() {
        let english = super::data("en");
        for locale in LOCALES {
            let bundle = super::data(locale);
            let missing: Vec<_> = english
                .keys()
                .filter(|k| !bundle.contains_key(*k))
                .collect();
            assert!(
                missing.is_empty(),
                "{locale} is missing {} keys, first: {:?}",
                missing.len(),
                &missing[..missing.len().min(5)]
            );
        }
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
                // Ask the bundle, not `t`: `t` answers in English for any locale that is
                // missing the key, so it can never report one as absent.
                assert!(
                    data(locale).contains_key(key),
                    "locale {locale} is missing {key}"
                );
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
    fn age_restricted_birthday_form_is_localized() {
        assert_eq!(t("en", "ageRestricted.dateOfBirth"), "Date of birth");
        assert_eq!(t("en", "ageRestricted.selectDay"), "Day");
        assert_eq!(t("vi", "ageRestricted.selectYear"), "Năm");
        assert_eq!(t("vi", "ageRestricted.month.january"), "Tháng 1");
        for locale in LOCALES {
            assert_ne!(
                t(locale, "ageRestricted.month.december"),
                "ageRestricted.month.december",
                "locale {locale} is missing the december label"
            );
        }
    }

    #[test]
    fn full_react_corpus_present() {
        assert_eq!(t("en", "clan.title"), "Customize Your Clan");
        assert_eq!(t("en", "channelCreator.monthsShort.0"), "JAN");
    }
}
