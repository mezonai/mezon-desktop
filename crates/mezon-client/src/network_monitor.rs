use tokio::sync::watch;

pub struct NetworkMonitor {
    online: watch::Receiver<bool>,
    watching_host: bool,
    #[cfg(target_os = "macos")]
    _thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(not(target_os = "macos"))]
    _keep_open: watch::Sender<bool>,
}

impl NetworkMonitor {
    /// Watch reachability of the host this client actually talks to.
    ///
    /// Watching the zero address instead — the usual "is there any internet" idiom — is useless
    /// here: loopback is always a route, so it answers "reachable" with every interface down.
    /// Measured on 07/09/2026 by turning Wi-Fi off with the app running: `is_online` stayed `true`
    /// throughout. A host is the only question worth asking, and asking it still costs no traffic:
    /// `SCNetworkReachability` answers from the routing table, it does not probe.
    pub fn for_host(host: &str) -> Self {
        #[cfg(target_os = "macos")]
        {
            let (tx, online) = watch::channel(true);
            let target = host.trim();
            if target.is_empty() {
                return Self {
                    online,
                    watching_host: false,
                    _thread: None,
                };
            }
            let Ok(host_c) = std::ffi::CString::new(target) else {
                return Self {
                    online,
                    watching_host: false,
                    _thread: None,
                };
            };
            match std::thread::Builder::new()
                .name("mezon-netmon".into())
                .spawn(move || macos::run(tx, host_c))
            {
                Ok(thread) => Self {
                    online,
                    watching_host: true,
                    _thread: Some(thread),
                },
                Err(e) => {
                    tracing::warn!("network monitor: could not spawn the reachability thread: {e}");
                    Self {
                        online,
                        watching_host: false,
                        _thread: None,
                    }
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = host;
            let (keep_open, online) = watch::channel(true);
            Self {
                online,
                watching_host: false,
                _keep_open: keep_open,
            }
        }
    }

    /// Whether this platform actually reports reachability, or only ever claims "online".
    ///
    /// Callers use this to decide whether the OS answer can stand in for an HTTP probe: where it
    /// is `true` the signal is a real one and costs nothing, where it is `false` the receiver is a
    /// constant and a caller that trusted it would never notice the network going away.
    pub fn has_os_signal(&self) -> bool {
        self.watching_host
    }

    pub fn is_online(&self) -> bool {
        *self.online.borrow()
    }

    pub fn online(&self) -> watch::Receiver<bool> {
        self.online.clone()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use system_configuration::network_reachability::{ReachabilityFlags, SCNetworkReachability};
    use tokio::sync::watch;

    /// Whether the host can be reached without asking the person to do something first.
    ///
    /// `CONNECTION_REQUIRED` alone does not mean offline. A VPN configured on demand reports the
    /// host as reachable *and* as needing a connection, because the tunnel will dial itself the
    /// moment traffic is sent — reading that as "no network" would park reconnect on a machine one
    /// packet away from working. It is only offline when nothing will bring the link up on its own,
    /// or when it needs a human (a captive portal, a password prompt), which is what
    /// `INTERVENTION_REQUIRED` marks.
    fn flags_online(flags: ReachabilityFlags) -> bool {
        if !flags.contains(ReachabilityFlags::REACHABLE) {
            return false;
        }
        if !flags.contains(ReachabilityFlags::CONNECTION_REQUIRED) {
            return true;
        }
        let dials_itself = flags.contains(ReachabilityFlags::CONNECTION_ON_DEMAND)
            || flags.contains(ReachabilityFlags::CONNECTION_ON_TRAFFIC);
        dials_itself && !flags.contains(ReachabilityFlags::INTERVENTION_REQUIRED)
    }

    pub fn run(tx: watch::Sender<bool>, host: std::ffi::CString) {
        let Some(mut reachability) = SCNetworkReachability::from_host(&host) else {
            tracing::warn!("network monitor: no reachability handle for the configured host");
            return;
        };

        if let Ok(flags) = reachability.reachability() {
            let _ = tx.send(flags_online(flags));
        }

        if reachability
            .set_callback(move |flags| {
                let _ = tx.send(flags_online(flags));
            })
            .is_err()
        {
            tracing::warn!("network monitor: failed to set reachability callback");
            return;
        }

        let scheduled = unsafe {
            reachability.schedule_with_runloop(&CFRunLoop::get_current(), kCFRunLoopCommonModes)
        };
        if scheduled.is_err() {
            tracing::warn!("network monitor: failed to schedule reachability on run loop");
            return;
        }

        CFRunLoop::run_current();
    }

    #[cfg(test)]
    mod flag_tests {
        use super::*;

        const REACH: ReachabilityFlags = ReachabilityFlags::REACHABLE;
        const NEEDS: ReachabilityFlags = ReachabilityFlags::CONNECTION_REQUIRED;
        const ON_DEMAND: ReachabilityFlags = ReachabilityFlags::CONNECTION_ON_DEMAND;
        const ON_TRAFFIC: ReachabilityFlags = ReachabilityFlags::CONNECTION_ON_TRAFFIC;
        const NEEDS_HUMAN: ReachabilityFlags = ReachabilityFlags::INTERVENTION_REQUIRED;

        #[test]
        fn a_plain_reachable_host_is_online() {
            assert!(flags_online(REACH));
        }

        #[test]
        fn nothing_reachable_is_offline() {
            assert!(!flags_online(ReachabilityFlags::empty()));
            assert!(!flags_online(NEEDS));
            assert!(!flags_online(NEEDS | ON_DEMAND));
        }

        /// The case this predicate got wrong: a VPN that dials on demand reports both reachable and
        /// connection-required, and a machine one packet away from working must not read as offline.
        #[test]
        fn a_vpn_that_dials_itself_is_online() {
            assert!(flags_online(REACH | NEEDS | ON_DEMAND));
            assert!(flags_online(REACH | NEEDS | ON_TRAFFIC));
        }

        /// But not when someone has to type something first — a captive portal or a password.
        #[test]
        fn a_link_that_needs_a_human_is_offline() {
            assert!(!flags_online(REACH | NEEDS | ON_DEMAND | NEEDS_HUMAN));
            assert!(!flags_online(REACH | NEEDS));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads live OS state, so it is `#[ignore]`d — run by hand with
    /// `cargo test -p mezon-client os_reachability -- --ignored --nocapture`.
    ///
    /// A remote host must read as reachable on a machine that is online. Watching the zero address
    /// used to pass this trivially while being blind to an interface going down, which is the bug
    /// this signal exists to avoid; a hostname is what makes the answer mean something.
    #[test]
    #[ignore = "reads live OS network state"]
    fn os_reachability_reports_a_live_network() {
        let monitor = NetworkMonitor::for_host("api.mezon.ai");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        println!(
            "has_os_signal={} is_online={}",
            monitor.has_os_signal(),
            monitor.is_online()
        );
        assert!(monitor.has_os_signal(), "macOS must report a real signal");
        assert!(
            monitor.is_online(),
            "OS reported offline on a machine that is online — reconnect would park"
        );
    }

    /// An empty or unusable host leaves no signal to trust, and the caller must be told so it can
    /// keep using the HTTP probe rather than believe a constant.
    #[test]
    fn an_unusable_host_reports_no_signal() {
        assert!(!NetworkMonitor::for_host("").has_os_signal());
        assert!(!NetworkMonitor::for_host("   ").has_os_signal());
        assert!(!NetworkMonitor::for_host("bad\0host").has_os_signal());
    }
}
