use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike};
use gpui::SharedString;
use mezon_store::message_time::local_datetime;
use ui::utils::{DateTimeType, format_distance};

const MONTH_KEYS: [&str; 12] = [
    "common.timeFormat.months.jan",
    "common.timeFormat.months.feb",
    "common.timeFormat.months.mar",
    "common.timeFormat.months.apr",
    "common.timeFormat.months.may",
    "common.timeFormat.months.jun",
    "common.timeFormat.months.jul",
    "common.timeFormat.months.aug",
    "common.timeFormat.months.sep",
    "common.timeFormat.months.oct",
    "common.timeFormat.months.nov",
    "common.timeFormat.months.dec",
];

const WEEKDAY_KEYS: [&str; 7] = [
    "common.timeFormat.daysOfWeek.sun",
    "common.timeFormat.daysOfWeek.mon",
    "common.timeFormat.daysOfWeek.tue",
    "common.timeFormat.daysOfWeek.wed",
    "common.timeFormat.daysOfWeek.thu",
    "common.timeFormat.daysOfWeek.fri",
    "common.timeFormat.daysOfWeek.sat",
];

pub fn format_message_time(
    time_hhmm: &SharedString,
    local_date: Option<NaiveDate>,
    locale: &str,
    now: DateTime<Local>,
) -> SharedString {
    let Some(msg_date) = local_date else {
        return time_hhmm.clone();
    };

    let today = now.date_naive();
    let yesterday = today - Duration::days(1);

    if msg_date == today {
        time_hhmm.clone()
    } else if msg_date == yesterday {
        format!(
            "{} {}",
            mezon_i18n::t(locale, "common.yesterdayAt"),
            time_hhmm
        )
        .into()
    } else {
        format!(
            "{:02}/{:02}/{}, {}",
            msg_date.day(),
            msg_date.month(),
            msg_date.year(),
            time_hhmm
        )
        .into()
    }
}

pub fn format_i18n_full_date_from_seconds(timestamp_sec: i64, locale: &str) -> String {
    let timestamp_sec = mezon_store::message_time::normalize_unix_seconds(timestamp_sec);
    if timestamp_sec <= 0 {
        return String::new();
    }
    let Some(target) = local_datetime(timestamp_sec) else {
        return String::new();
    };
    let day_name = mezon_i18n::t(
        locale,
        WEEKDAY_KEYS[target.weekday().num_days_from_sunday() as usize],
    );
    let month_name = mezon_i18n::t(locale, MONTH_KEYS[target.month0() as usize]);
    mezon_i18n::t(locale, "common.timeFormat.fullDate")
        .replace("{{dayName}}", day_name)
        .replace("{{monthName}}", month_name)
        .replace("{{day}}", &target.day().to_string())
        .replace("{{hours}}", &format!("{:02}", target.hour()))
        .replace("{{minutes}}", &format!("{:02}", target.minute()))
}

pub fn format_relative_time_from_seconds(
    timestamp_sec: i64,
    locale: &str,
    now: DateTime<Local>,
) -> String {
    let timestamp_sec = mezon_store::message_time::normalize_unix_seconds(timestamp_sec);
    if timestamp_sec <= 0 {
        return String::new();
    }
    let Some(target) = local_datetime(timestamp_sec) else {
        return String::new();
    };
    let diff = now.timestamp().saturating_sub(timestamp_sec);
    if diff <= 1 {
        return mezon_i18n::t(locale, "common.justNow").to_string();
    }
    format_distance(
        DateTimeType::Naive(target.naive_local()),
        now.naive_local(),
        false,
        true,
        false,
    )
}

pub fn format_channel_setting_relative_time_from_seconds(
    timestamp_sec: i64,
    locale: &str,
    now: DateTime<Local>,
) -> String {
    let timestamp_sec = mezon_store::message_time::normalize_unix_seconds(timestamp_sec);
    if timestamp_sec <= 0 {
        return String::new();
    }
    let seconds = now.timestamp().saturating_sub(timestamp_sec).abs();
    let minutes = ((seconds as f64) / 60.).round() as i64;
    let token = if minutes < 2 {
        if minutes == 0 {
            DistanceToken::LessMinute
        } else {
            DistanceToken::Minutes(1)
        }
    } else if minutes < 45 {
        DistanceToken::Minutes(minutes)
    } else if minutes < 90 {
        DistanceToken::AboutHours(1)
    } else if minutes < 1_440 {
        DistanceToken::AboutHours(((minutes as f64) / 60.).round() as i64)
    } else if minutes < 2_520 {
        DistanceToken::Days(1)
    } else if minutes < 43_200 {
        DistanceToken::Days(((minutes as f64) / 1_440.).round() as i64)
    } else if minutes < 64_800 {
        DistanceToken::AboutMonths(1)
    } else if minutes < 86_400 {
        DistanceToken::AboutMonths(2)
    } else if minutes < 525_600 {
        DistanceToken::Months(((minutes as f64) / 43_200.).round() as i64)
    } else {
        let months = ((minutes as f64) / 43_200.).round() as i64;
        let years = months / 12;
        match months % 12 {
            0..=2 => DistanceToken::AboutYears(years),
            3..=8 => DistanceToken::OverYears(years),
            _ => DistanceToken::AlmostYears(years + 1),
        }
    };
    format_distance_token(token, locale)
}

#[derive(Clone, Copy)]
enum DistanceToken {
    LessMinute,
    Minutes(i64),
    AboutHours(i64),
    Days(i64),
    AboutMonths(i64),
    Months(i64),
    AboutYears(i64),
    OverYears(i64),
    AlmostYears(i64),
}

fn format_distance_token(token: DistanceToken, locale: &str) -> String {
    match locale.split('-').next().unwrap_or("en") {
        "vi" => match token {
            DistanceToken::LessMinute => "dưới 1 phút trước".into(),
            DistanceToken::Minutes(n) => format!("{n} phút trước"),
            DistanceToken::AboutHours(n) => format!("khoảng {n} giờ trước"),
            DistanceToken::Days(n) => format!("{n} ngày trước"),
            DistanceToken::AboutMonths(n) => format!("khoảng {n} tháng trước"),
            DistanceToken::Months(n) => format!("{n} tháng trước"),
            DistanceToken::AboutYears(n) => format!("khoảng {n} năm trước"),
            DistanceToken::OverYears(n) => format!("hơn {n} năm trước"),
            DistanceToken::AlmostYears(n) => format!("gần {n} năm trước"),
        },
        "es" => {
            let value = match token {
                DistanceToken::LessMinute => "menos de un minuto".into(),
                DistanceToken::Minutes(1) => "1 minuto".into(),
                DistanceToken::Minutes(n) => format!("{n} minutos"),
                DistanceToken::AboutHours(1) => "alrededor de 1 hora".into(),
                DistanceToken::AboutHours(n) => format!("alrededor de {n} horas"),
                DistanceToken::Days(1) => "1 día".into(),
                DistanceToken::Days(n) => format!("{n} días"),
                DistanceToken::AboutMonths(1) => "alrededor de 1 mes".into(),
                DistanceToken::AboutMonths(n) => format!("alrededor de {n} meses"),
                DistanceToken::Months(n) => format!("{n} meses"),
                DistanceToken::AboutYears(1) => "alrededor de 1 año".into(),
                DistanceToken::AboutYears(n) => format!("alrededor de {n} años"),
                DistanceToken::OverYears(1) => "más de 1 año".into(),
                DistanceToken::OverYears(n) => format!("más de {n} años"),
                DistanceToken::AlmostYears(1) => "casi 1 año".into(),
                DistanceToken::AlmostYears(n) => format!("casi {n} años"),
            };
            format!("hace {value}")
        }
        "ru" => match token {
            DistanceToken::LessMinute => "меньше минуты назад".into(),
            DistanceToken::Minutes(n) => {
                format!("{} назад", ru_count(n, "минуту", "минуты", "минут"))
            }
            DistanceToken::AboutHours(n) => {
                format!("около {} назад", ru_count(n, "часа", "часов", "часов"))
            }
            DistanceToken::Days(n) => {
                format!("{} назад", ru_count(n, "день", "дня", "дней"))
            }
            DistanceToken::AboutMonths(n) => format!(
                "около {} назад",
                ru_count(n, "месяца", "месяцев", "месяцев")
            ),
            DistanceToken::Months(n) => {
                format!("{} назад", ru_count(n, "месяц", "месяца", "месяцев"))
            }
            DistanceToken::AboutYears(n) => {
                format!("около {} назад", ru_count(n, "года", "лет", "лет"))
            }
            DistanceToken::OverYears(n) => {
                format!("больше {} назад", ru_count(n, "года", "лет", "лет"))
            }
            DistanceToken::AlmostYears(n) => {
                format!("почти {} назад", ru_count(n, "год", "года", "лет"))
            }
        },
        _ => match token {
            DistanceToken::LessMinute => "less than a minute ago".into(),
            DistanceToken::Minutes(1) => "1 minute ago".into(),
            DistanceToken::Minutes(n) => format!("{n} minutes ago"),
            DistanceToken::AboutHours(1) => "about 1 hour ago".into(),
            DistanceToken::AboutHours(n) => format!("about {n} hours ago"),
            DistanceToken::Days(1) => "1 day ago".into(),
            DistanceToken::Days(n) => format!("{n} days ago"),
            DistanceToken::AboutMonths(1) => "about 1 month ago".into(),
            DistanceToken::AboutMonths(n) => format!("about {n} months ago"),
            DistanceToken::Months(n) => format!("{n} months ago"),
            DistanceToken::AboutYears(1) => "about 1 year ago".into(),
            DistanceToken::AboutYears(n) => format!("about {n} years ago"),
            DistanceToken::OverYears(1) => "over 1 year ago".into(),
            DistanceToken::OverYears(n) => format!("over {n} years ago"),
            DistanceToken::AlmostYears(1) => "almost 1 year ago".into(),
            DistanceToken::AlmostYears(n) => format!("almost {n} years ago"),
        },
    }
}

fn ru_count(count: i64, one: &str, few: &str, many: &str) -> String {
    let rem_100 = count % 100;
    let rem_10 = count % 10;
    let word = if rem_10 == 1 && rem_100 != 11 {
        one
    } else if (2..=4).contains(&rem_10) && !(12..=14).contains(&rem_100) {
        few
    } else {
        many
    };
    format!("{count} {word}")
}

pub fn format_date_divider(ts: i64, locale: &str, now: DateTime<Local>) -> String {
    let Some(dt) = local_datetime(ts) else {
        return String::new();
    };

    let month = mezon_i18n::t(locale, MONTH_KEYS[dt.month0() as usize]);
    let formatted = format!("{:02} {} {}", dt.day(), month, dt.year());

    let today = now.date_naive();
    if dt.date_naive() == today {
        format!("{}, {}", mezon_i18n::t(locale, "common.today"), formatted)
    } else {
        let weekday = mezon_i18n::t(
            locale,
            WEEKDAY_KEYS[dt.weekday().num_days_from_sunday() as usize],
        );
        format!("{weekday}, {formatted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn i18n_full_date_is_empty_for_missing_timestamp() {
        assert_eq!(format_i18n_full_date_from_seconds(0, "en"), "");
    }

    #[test]
    fn message_time_today_is_hhmm_only() {
        let now = Local::now();
        let hhmm: SharedString = format!("{:02}:{:02}", now.hour(), now.minute()).into();
        let label = format_message_time(&hhmm, Some(now.date_naive()), "en", now);
        assert_eq!(label, hhmm);
    }

    #[test]
    fn date_divider_today_prefixes_common_today() {
        let now = Local::now();
        let label = format_date_divider(now.timestamp(), "en", now);
        assert!(label.starts_with("Today,"));
    }

    #[test]
    fn channel_setting_distance_uses_react_date_locale_mapping() {
        let now = Local::now();
        assert_eq!(
            format_channel_setting_relative_time_from_seconds(now.timestamp() - 12 * 60, "vi", now),
            "12 phút trước"
        );
        assert_eq!(
            format_channel_setting_relative_time_from_seconds(
                now.timestamp() - 45 * 86_400,
                "vi-VN",
                now
            ),
            "khoảng 2 tháng trước"
        );
        assert_eq!(
            format_channel_setting_relative_time_from_seconds(
                now.timestamp() - 3 * 86_400,
                "de",
                now
            ),
            "3 days ago"
        );
    }
}
