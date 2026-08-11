use anyhow::Result;
use url::Url;

pub struct ChannelAppLaunchParams<'a> {
    pub web_app_data: &'a str,
    pub clan_id: &'a str,
    pub clan_name: Option<&'a str>,
}

pub fn build_channel_app_url(app_url: &str, params: ChannelAppLaunchParams<'_>) -> Result<String> {
    match Url::parse(app_url) {
        Ok(mut url) => {
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("data", params.web_app_data);
                pairs.append_pair("clanId", params.clan_id);
                if let Some(clan_name) = params.clan_name.filter(|name| !name.is_empty()) {
                    pairs.append_pair("clanName", clan_name);
                }
            }
            Ok(url.to_string())
        }
        Err(_) => {
            let sep = if app_url.contains('?') { '&' } else { '?' };
            let mut query = format!(
                "data={}&clanId={}",
                encode_url_param(params.web_app_data),
                encode_url_param(params.clan_id),
            );
            if let Some(clan_name) = params.clan_name.filter(|name| !name.is_empty()) {
                query.push_str("&clanName=");
                query.push_str(&encode_url_param(clan_name));
            }
            Ok(format!("{app_url}{sep}{query}"))
        }
    }
}

pub fn encode_url_param(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_query_params_to_valid_url() {
        let url = build_channel_app_url(
            "https://app.example.com/path",
            ChannelAppLaunchParams {
                web_app_data: "hash123",
                clan_id: "42",
                clan_name: Some("My Clan"),
            },
        )
        .unwrap();
        assert!(url.contains("data=hash123"));
        assert!(url.contains("clanId=42"));
        assert!(url.contains("clanName=My+Clan") || url.contains("clanName=My%20Clan"));
    }

    #[test]
    fn falls_back_for_invalid_base_url() {
        let url = build_channel_app_url(
            "not-a-url",
            ChannelAppLaunchParams {
                web_app_data: "hash",
                clan_id: "1",
                clan_name: None,
            },
        )
        .unwrap();
        assert_eq!(url, "not-a-url?data=hash&clanId=1");
    }
}
