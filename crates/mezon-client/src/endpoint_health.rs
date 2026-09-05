use std::time::{Duration, Instant};

const SLOW_RTT: Duration = Duration::from_millis(500);
const SLOW_STREAK_REQUIRED: u8 = 3;
const SLOW_SWITCH_COOLDOWN: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEndpoint {
    pub id: i32,
    pub host: String,
    pub port: u16,
}

impl RealtimeEndpoint {
    pub fn is_same_node(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port
    }

    pub fn label(&self) -> String {
        if self.id > 0 {
            format!("{} ({}:{})", self.id, self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Default)]
pub struct EndpointHealth {
    endpoint: Option<RealtimeEndpoint>,
    connected_since: Option<Instant>,
    slow_streak: u8,
    slow_report_suppressed_until: Option<Instant>,
    slow_reports_disabled: bool,
}

impl EndpointHealth {
    pub fn set_endpoint(&mut self, endpoint: Option<RealtimeEndpoint>) {
        if self.is_on_same_node_as(endpoint.as_ref()) {
            self.endpoint = endpoint;
            return;
        }
        self.endpoint = endpoint;
        self.forget_connection();
        self.slow_report_suppressed_until = None;
        self.slow_reports_disabled = false;
    }

    pub fn connected_endpoint(&self) -> Option<RealtimeEndpoint> {
        self.connected_since?;
        self.endpoint.clone()
    }

    pub fn record_connected(&mut self, now: Instant) {
        self.connected_since = Some(now);
        self.slow_streak = 0;
        self.slow_report_suppressed_until = None;
        self.slow_reports_disabled = false;
    }

    pub fn record_disconnected(&mut self) {
        self.forget_connection();
    }

    pub fn record_active_probe(&mut self, rtt: Duration, now: Instant) -> bool {
        let Some(connected_since) = self.connected_since else {
            return false;
        };
        if self.slow_reports_disabled
            || self
                .slow_report_suppressed_until
                .is_some_and(|until| until > now)
        {
            self.slow_streak = 0;
            return false;
        }
        let settled_on_this_node =
            now.saturating_duration_since(connected_since) >= SLOW_SWITCH_COOLDOWN;
        if settled_on_this_node && rtt >= SLOW_RTT {
            self.slow_streak = self.slow_streak.saturating_add(1);
        } else {
            self.slow_streak = 0;
        }
        if self.slow_streak < SLOW_STREAK_REQUIRED {
            return false;
        }
        self.slow_streak = 0;
        self.slow_report_suppressed_until = Some(now + SLOW_SWITCH_COOLDOWN);
        true
    }

    pub fn disable_slow_reports(&mut self) {
        self.slow_reports_disabled = true;
        self.slow_streak = 0;
    }

    fn is_on_same_node_as(&self, other: Option<&RealtimeEndpoint>) -> bool {
        match (self.endpoint.as_ref(), other) {
            (Some(current), Some(next)) => current.is_same_node(next),
            (None, None) => true,
            _ => false,
        }
    }

    fn forget_connection(&mut self) {
        self.connected_since = None;
        self.slow_streak = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(id: i32, host: &str) -> RealtimeEndpoint {
        RealtimeEndpoint {
            id,
            host: host.into(),
            port: 4433,
        }
    }

    #[test]
    fn a_slow_link_is_reported_only_after_settling_and_three_samples() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);

        assert!(!health.record_active_probe(Duration::from_millis(800), now));
        let settled = now + SLOW_SWITCH_COOLDOWN;
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(health.record_active_probe(Duration::from_millis(800), settled));
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(
            !health.record_active_probe(Duration::from_millis(800), settled + SLOW_SWITCH_COOLDOWN)
        );
    }

    #[test]
    fn one_fast_sample_breaks_the_streak() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);
        let settled = now + SLOW_SWITCH_COOLDOWN;

        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(!health.record_active_probe(Duration::from_millis(40), settled));
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        assert!(health.record_active_probe(Duration::from_millis(800), settled));
    }

    #[test]
    fn a_gateway_confirmed_slow_node_stops_reporting_until_the_next_connect() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);

        let settled = now + SLOW_SWITCH_COOLDOWN;
        for _ in 0..2 {
            assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        }
        assert!(health.record_active_probe(Duration::from_millis(800), settled));

        health.disable_slow_reports();
        let much_later = settled + SLOW_SWITCH_COOLDOWN * 10;
        for _ in 0..10 {
            assert!(!health.record_active_probe(Duration::from_millis(800), much_later));
        }

        health.record_connected(much_later);
        let resettled = much_later + SLOW_SWITCH_COOLDOWN;
        for _ in 0..2 {
            assert!(!health.record_active_probe(Duration::from_millis(800), resettled));
        }
        assert!(health.record_active_probe(Duration::from_millis(800), resettled));
    }

    #[test]
    fn a_fresh_connection_does_not_inherit_the_previous_ones_suppression() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);

        let settled = now + SLOW_SWITCH_COOLDOWN;
        for _ in 0..2 {
            assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        }
        assert!(health.record_active_probe(Duration::from_millis(800), settled));

        health.record_disconnected();
        health.record_connected(settled);

        let resettled = settled + SLOW_SWITCH_COOLDOWN;
        for _ in 0..2 {
            assert!(!health.record_active_probe(Duration::from_millis(800), resettled));
        }
        assert!(health.record_active_probe(Duration::from_millis(800), resettled));
    }

    #[test]
    fn the_gateway_naming_the_id_of_the_node_we_are_on_keeps_the_connection() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(0, "sock.example.com")));
        health.record_connected(now);

        health.set_endpoint(Some(endpoint(3, "sock.example.com")));

        assert_eq!(
            health.connected_endpoint().map(|endpoint| endpoint.id),
            Some(3),
            "the id is metadata about the node we are on, not a different node"
        );
    }

    #[test]
    fn moving_to_another_node_starts_its_history_from_scratch() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);
        health.disable_slow_reports();

        health.set_endpoint(Some(endpoint(2, "sock2.example.com")));
        assert_eq!(health.connected_endpoint(), None);
        health.record_connected(now);

        let settled = now + SLOW_SWITCH_COOLDOWN;
        for _ in 0..2 {
            assert!(!health.record_active_probe(Duration::from_millis(800), settled));
        }
        assert!(health.record_active_probe(Duration::from_millis(800), settled));
    }

    #[test]
    fn a_dropped_connection_retires_the_observation() {
        let now = Instant::now();
        let mut health = EndpointHealth::default();
        health.set_endpoint(Some(endpoint(1, "sock.example.com")));
        health.record_connected(now);
        health.record_disconnected();

        assert_eq!(health.connected_endpoint(), None);
        assert!(!health.record_active_probe(Duration::from_millis(800), now));
    }

    #[test]
    fn a_node_the_gateway_did_not_name_is_labelled_by_address() {
        assert_eq!(
            endpoint(2, "sock.example.com").label(),
            "2 (sock.example.com:4433)"
        );
        assert_eq!(
            endpoint(0, "sock.example.com").label(),
            "sock.example.com:4433"
        );
    }
}
