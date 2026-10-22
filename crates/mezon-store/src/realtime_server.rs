use gpui::App;
use mezon_client::RealtimeEndpoint;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{AppConfig, Settings};

const PROD_GATEWAY_HOST: &str = "gw.mezon.ai";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RealtimeServer {
    #[default]
    Auto,
    Vn1,
    Vn2,
    Us,
}

impl RealtimeServer {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Vn1, Self::Vn2, Self::Us];

    pub fn host(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Vn1 => Some("sock.mezon.ai"),
            Self::Vn2 => Some("sock3.mezon.ai"),
            Self::Us => Some("sock2.mezon.ai"),
        }
    }

    pub fn region_name(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Vn1 => Some("VN1"),
            Self::Vn2 => Some("VN2"),
            Self::Us => Some("US"),
        }
    }

    pub fn region_of_host(host: &str) -> Option<&'static str> {
        Self::ALL
            .into_iter()
            .find(|server| {
                server
                    .host()
                    .is_some_and(|known| known.eq_ignore_ascii_case(host))
            })
            .and_then(Self::region_name)
    }

    pub fn is_offered_by(gateway_host: &str) -> bool {
        gateway_host.trim().eq_ignore_ascii_case(PROD_GATEWAY_HOST)
    }

    pub fn is_offered(cx: &App) -> bool {
        AppConfig::try_global(cx).is_some_and(|config| Self::is_offered_by(config.client_host()))
    }

    pub fn current(cx: &App) -> Self {
        if !Self::is_offered(cx) {
            return Self::Auto;
        }
        Settings::try_global(cx)
            .map(|settings| settings.read(cx).realtime_server)
            .unwrap_or_default()
    }

    pub fn steer(self, endpoint: RealtimeEndpoint) -> RealtimeEndpoint {
        match (self.host(), self.node_id()) {
            (Some(host), Some(id)) => RealtimeEndpoint {
                id,
                host: host.to_string(),
                port: endpoint.port,
            },
            _ => endpoint,
        }
    }

    fn node_id(self) -> Option<i32> {
        match self {
            Self::Auto => None,
            Self::Vn1 => Some(1),
            Self::Vn2 => Some(3),
            Self::Us => Some(2),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Vn1 => "vn1",
            Self::Vn2 => "vn2",
            Self::Us => "us",
        }
    }

    fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|server| server.key() == key)
            .unwrap_or_default()
    }
}

impl Serialize for RealtimeServer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl<'de> Deserialize<'de> for RealtimeServer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = Option::<String>::deserialize(deserializer)?;
        Ok(key.as_deref().map(Self::from_key).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(host: &str, port: u16) -> RealtimeEndpoint {
        RealtimeEndpoint {
            id: 0,
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn auto_keeps_the_node_the_gateway_gave() {
        let gateway_node = endpoint("sock3.mezon.ai", 443);
        assert_eq!(
            RealtimeServer::Auto.steer(gateway_node.clone()),
            gateway_node
        );
    }

    #[test]
    fn a_pinned_server_replaces_the_host_and_keeps_the_port() {
        let steered = RealtimeServer::Us.steer(endpoint("sock.mezon.ai", 443));
        assert_eq!(
            steered,
            RealtimeEndpoint {
                id: 2,
                host: "sock2.mezon.ai".to_string(),
                port: 443,
            }
        );
        assert_eq!(RealtimeServer::Vn2.steer(endpoint("x", 7349)).id, 3);
        assert_eq!(RealtimeServer::Vn1.steer(endpoint("x", 7349)).port, 7349);
    }

    #[test]
    fn region_names_follow_the_mobile_labels() {
        assert_eq!(RealtimeServer::region_of_host("sock.mezon.ai"), Some("VN1"));
        assert_eq!(
            RealtimeServer::region_of_host("SOCK3.mezon.ai"),
            Some("VN2")
        );
        assert_eq!(RealtimeServer::region_of_host("sock2.mezon.ai"), Some("US"));
        assert_eq!(RealtimeServer::region_of_host("dev-mezon.nccsoft.vn"), None);
    }

    #[test]
    fn only_the_production_gateway_offers_a_choice() {
        assert!(RealtimeServer::is_offered_by("gw.mezon.ai"));
        assert!(!RealtimeServer::is_offered_by("dev-mezon.nccsoft.vn"));
        assert!(!RealtimeServer::is_offered_by(""));
    }

    #[test]
    fn the_choice_round_trips_and_unknown_values_fall_back_to_auto() {
        for server in RealtimeServer::ALL {
            let json = serde_json::to_string(&server).expect("serialize");
            let back: RealtimeServer = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, server);
        }
        let unknown: RealtimeServer = serde_json::from_str("\"eu\"").expect("deserialize");
        assert_eq!(unknown, RealtimeServer::Auto);
        let null: RealtimeServer = serde_json::from_str("null").expect("deserialize");
        assert_eq!(null, RealtimeServer::Auto);
    }

    #[test]
    fn settings_without_the_field_default_to_auto() {
        let settings: Settings = serde_json::from_str("{}").expect("deserialize");
        assert_eq!(settings.realtime_server, RealtimeServer::Auto);
        let pinned: Settings =
            serde_json::from_str(r#"{"realtime_server":"vn2"}"#).expect("deserialize");
        assert_eq!(pinned.realtime_server, RealtimeServer::Vn2);
    }
}
