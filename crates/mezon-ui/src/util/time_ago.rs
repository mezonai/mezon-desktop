//! Coarse relative-time labels for a countdown, over the web client's `timeFormat.timeAgo`
//! strings. The buckets are not the web's own: `BanCountDown` rounds to the nearest unit (and so
//! can claim two hours with an hour and forty minutes left) and drops sub-minute values into a
//! per-second ticker. Time *remaining* has to round down, so these floor instead.

// The same three the ban store schedules its repaints on, so a bucket edge here and a wake there
// cannot drift apart.
const MINUTE: i64 = mezon_store::BAN_LABEL_MINUTE_SECS as i64;
const HOUR: i64 = mezon_store::BAN_LABEL_HOUR_SECS as i64;
const DAY: i64 = mezon_store::BAN_LABEL_DAY_SECS as i64;

fn count(locale: &str, key: &'static str, value: i64) -> String {
    mezon_i18n::t(locale, key).replace("{{count}}", &value.to_string())
}

/// How long is left: whole days, then hours, then minutes, then a catch-all for the final
/// minute, each rounded down. A non-positive value reads as expired.
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
    fn buckets_round_down_to_the_unit_on_show() {
        assert_eq!(remaining("en", 0), "Expired");
        assert_eq!(remaining("en", -5), "Expired");
        assert_eq!(remaining("en", 30), "Less than a minute");
        assert_eq!(remaining("en", 60), "1m");
        assert_eq!(remaining("en", 59 * 60), "59m");
        assert_eq!(remaining("en", 60 * 60), "1h");
        assert_eq!(remaining("en", 25 * 60 * 60), "1d");
    }

    /// The store sleeps until `seconds_until_ban_label_changes` says this string turns over. If it
    /// ever wakes on a label that still reads the same, that is a repaint of the whole chat spent
    /// on nothing — and if it sleeps past one, the notice sits there lying about the time left.
    #[test]
    fn every_scheduled_wake_lands_on_a_label_that_reads_differently() {
        for remaining_secs in [
            2 * 86_400_u64,
            90_000,
            86_400,
            7_000,
            3_600,
            3_599,
            930,
            900,
            61,
            60,
            45,
            1,
        ] {
            let step = mezon_store::seconds_until_ban_label_changes(remaining_secs) as i64;
            let left = remaining_secs as i64;
            assert!(step > 0, "{remaining_secs}s left scheduled no wake at all");
            let now = remaining("en", left);
            if step > 1 {
                assert_eq!(
                    remaining("en", left - step + 1),
                    now,
                    "{remaining_secs}s left: woke {step}s early, label had not turned over yet"
                );
            }
            assert_ne!(
                remaining("en", left - step),
                now,
                "{remaining_secs}s left: slept {step}s past the turn"
            );
        }
    }
}
