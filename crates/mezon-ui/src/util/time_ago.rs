//! Coarse relative-time labels, matching the web client's `convertTimestampToTimeRemaining`.

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

fn count(locale: &str, key: &'static str, value: i64) -> String {
    mezon_i18n::t(locale, key).replace("{{count}}", &value.to_string())
}

/// How long is left, in the same buckets the web client uses: whole days, then hours, then
/// minutes, then a catch-all for the final minute. A non-positive value reads as expired.
pub fn remaining(locale: &str, seconds: i64) -> String {
    if seconds <= 0 {
        return mezon_i18n::t(locale, "common.timeFormat.timeAgo.expired").to_string();
    }
    if seconds >= DAY {
        return count(locale, "common.timeFormat.timeAgo.days", seconds / DAY);
    }
    if seconds >= HOUR {
        return count(locale, "common.timeFormat.timeAgo.hours", seconds / HOUR);
    }
    if seconds >= MINUTE {
        return count(
            locale,
            "common.timeFormat.timeAgo.minutes",
            seconds / MINUTE,
        );
    }
    mezon_i18n::t(locale, "common.timeFormat.timeAgo.lessThanMinute").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_match_the_web_clients_thresholds() {
        assert_eq!(remaining("en", 0), "Expired");
        assert_eq!(remaining("en", -5), "Expired");
        assert_eq!(remaining("en", 30), "Less than a minute");
        assert_eq!(remaining("en", 60), "1m");
        assert_eq!(remaining("en", 59 * 60), "59m");
        assert_eq!(remaining("en", 60 * 60), "1h");
        assert_eq!(remaining("en", 25 * 60 * 60), "1d");
    }
}
