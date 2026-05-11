# Testing Guide

This codebase currently has zero tests. This document explains how to write tests for it — covering pure-function unit tests, GPUI entity tests, async tests, and how to run the test suite.

The project uses `cargo-nextest` as the test runner (faster, better output than `cargo test`). Coverage is measured with `cargo-llvm-cov`.

---

## 1. Quick Start

```bash
just test              # run all workspace tests via nextest
just test my_fn_name   # run tests matching a name pattern
just cov               # generate + open HTML coverage report
just cov-summary       # print coverage summary to terminal
```

Under the hood:
```bash
cargo nextest run              # equivalent to `just test`
cargo llvm-cov --html          # equivalent to `just cov`
```

---

## 2. Where Tests Live

Follow the standard Rust convention: unit tests go in `#[cfg(test)]` modules in the same file as the code; integration tests go in a `tests/` directory at the crate root.

```
crates/mezon-client/
├── src/
│   ├── auth.rs         ← unit tests at bottom in #[cfg(test)] mod tests { }
│   ├── session.rs      ← unit tests here
│   └── keychain.rs     ← unit tests here
└── tests/
    └── auth_integration.rs   ← integration tests (if needed)
```

---

## 3. Unit Tests for Pure Functions

These are the highest-priority tests to add first. They have no GPUI dependency and are straightforward:

### `Session::is_expired` (`mezon-client/src/session.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn not_expired_when_expires_at_is_future() {
        let session = Session {
            expires_at: now_secs() + 3600, // 1 hour from now
            ..Default::default()
        };
        assert!(!session.is_expired());
    }

    #[test]
    fn expired_when_expires_at_is_past() {
        let session = Session {
            expires_at: now_secs() - 1, // 1 second ago
            ..Default::default()
        };
        assert!(session.is_expired());
    }

    #[test]
    fn not_expired_when_expires_at_is_zero() {
        // Zero means "unknown" — treated as never expiring
        let session = Session {
            expires_at: 0,
            ..Default::default()
        };
        assert!(!session.is_expired());
    }
}
```

### `decode_jwt_claims` (`mezon-client/src/auth.rs`)

This private function needs to be made `pub(crate)` or the test module must be inside `auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_jwt(uid: &str, usn: &str, exp: u64) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({ "uid": uid, "usn": usn, "exp": exp });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("header.{encoded}.signature")
    }

    #[test]
    fn extracts_claims_from_valid_jwt() {
        let token = make_jwt("user-123", "alice", 9999999999);
        let (uid, usn, exp) = decode_jwt_claims(&token);
        assert_eq!(uid, "user-123");
        assert_eq!(usn, "alice");
        assert_eq!(exp, 9999999999);
    }

    #[test]
    fn returns_defaults_for_malformed_token() {
        let (uid, usn, exp) = decode_jwt_claims("not.a.jwt");
        assert_eq!(uid, "");
        assert_eq!(usn, "");
        assert_eq!(exp, 0);
    }

    #[test]
    fn returns_defaults_for_empty_string() {
        let (uid, usn, exp) = decode_jwt_claims("");
        assert_eq!(uid, "");
        assert_eq!(usn, "");
        assert_eq!(exp, 0);
    }
}
```

### `Settings` serialization (`mezon-store/src/lib.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_serialize_and_round_trip() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.auto_start, original.auto_start);
        assert_eq!(restored.theme, original.theme);
        assert_eq!(restored.zoom_factor, original.zoom_factor);
    }

    #[test]
    fn settings_with_custom_values_round_trip() {
        let original = Settings {
            auto_start: true,
            zoom_factor: 1.25,
            theme: "light".to_string(),
            window_bounds: Some([100, 200, 1440, 900]),
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&original).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert!(restored.auto_start);
        assert_eq!(restored.zoom_factor, 1.25);
        assert_eq!(restored.window_bounds, Some([100, 200, 1440, 900]));
    }

    #[test]
    fn missing_fields_use_defaults_via_serde_default() {
        // Simulates a settings file that is missing new fields added after a release
        let json = r#"{"auto_start": true}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.auto_start);
        assert_eq!(settings.zoom_factor, 1.0);       // default
        assert_eq!(settings.theme, "dark");           // default
    }
}
```

### `LoginError::Display` (`mezon-store/src/lib.rs`)

```rust
#[test]
fn login_error_display_messages_are_user_friendly() {
    assert_eq!(
        LoginError::InvalidCredentials.to_string(),
        "Invalid credentials. Please try again."
    );
    assert_eq!(
        LoginError::OtpExpired.to_string(),
        "OTP has expired. Please request a new one."
    );
    assert!(LoginError::NetworkError("timeout".into()).to_string().contains("Network error"));
    assert!(LoginError::ServerError("500".into()).to_string().contains("Server error"));
}
```

---

## 4. Async Tests with Tokio

`mezon-client` uses tokio for HTTP. To test async functions, add `tokio` as a dev dependency and use `#[tokio::test]`:

```toml
# crates/mezon-client/Cargo.toml
[dev-dependencies]
tokio = { workspace = true }
```

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_load_returns_defaults_when_file_missing() {
        // Use a temp dir so we don't pollute ~/.config/mezon
        let result = Settings::load().await;
        // If ~/.config/mezon/settings.json doesn't exist this should succeed with defaults
        assert!(result.is_ok());
    }
}
```

For HTTP client tests that hit a real server, use integration tests behind a feature flag or environment variable gate:

```rust
#[cfg(test)]
mod integration {
    use super::*;

    #[tokio::test]
    #[ignore = "requires live dev server — run with --include-ignored"]
    async fn request_otp_returns_req_id() {
        let client = MezonClient::default();
        let result = client.request_otp("test@example.com").await;
        // Assert shape, not specific values
        assert!(result.is_ok() || result.unwrap_err().to_string().contains("HTTP"));
    }
}
```

Run ignored tests explicitly:
```bash
cargo nextest run --include-ignored
```

---

## 5. GPUI Tests

GPUI ships with a headless test context (`gpui::TestApp`) that lets you create entities and test state transitions without opening a real window. Tests can be written in any crate that depends on `gpui`.

```rust
#[cfg(test)]
mod tests {
    use gpui::{App, TestApp};
    use mezon_store::AuthState;

    #[test]
    fn auth_state_defaults_to_not_authenticated() {
        TestApp::new().run(|cx| {
            let state = cx.new(|_cx| AuthState::default());
            assert!(matches!(state.read(cx), AuthState::NotAuthenticated));
        });
    }

    #[test]
    fn auth_state_transitions_to_authenticated() {
        TestApp::new().run(|cx| {
            let state = cx.new(|_cx| AuthState::default());

            let fake_session = mezon_client::Session {
                token: "tok".into(),
                user_id: "u1".into(),
                ..Default::default()
            };
            state.update(cx, |s, cx| {
                *s = AuthState::Authenticated(fake_session);
                cx.notify();
            });

            assert!(matches!(state.read(cx), AuthState::Authenticated(_)));
        });
    }
}
```

### Testing `cx.notify()` re-render triggers

`gpui::VisualTestContext` (from `gpui::test`) lets you assert that a view re-renders after state changes. This is more involved and generally not needed for pure business logic tests.

---

## 6. Testing `mezon-native` Modules

Platform-specific modules are hard to unit test because they touch the OS. The recommended approach is:

1. Test the **pure logic** separately from the OS call
2. Feature-gate integration tests that actually call the OS API

Example for `autostart.rs`:

```rust
#[cfg(test)]
mod tests {
    // The sync_auto_start function cannot be meaningfully unit tested
    // without side-effecting the real system.
    // Test that it does not panic on a valid path:
    #[test]
    #[cfg(not(ci))]  // skip on CI where we can't register login items
    fn sync_auto_start_false_does_not_panic() {
        // Should succeed silently even if not currently registered
        super::sync_auto_start(false);
    }
}
```

For `SingleInstance`, the logic can be tested end-to-end in a subprocess:

```rust
// tests/single_instance.rs
#[test]
fn second_instance_returns_none() {
    use mezon_native::instance::SingleInstance;
    // First instance
    let lock = SingleInstance::try_acquire().unwrap();
    assert!(lock.is_some());

    // "Second instance" — this will fail to bind the same socket
    // Note: this only works if run as a separate process or after cleaning up the socket
}
```

---

## 7. Coverage Goals

Target: **80% line coverage** across non-vendor crates.

```bash
just cov-summary   # shows per-file coverage percentages
just cov           # opens HTML report in browser
```

Priority order for getting to 80%:
1. `mezon-store` — `Settings` serde + `LoginError` display + `AuthState` variants (~100% achievable with pure tests)
2. `mezon-client::session` — `is_expired` (~100% with 3 tests)
3. `mezon-client::auth` — `decode_jwt_claims`, `parse_session` (~80% without live server)
4. `mezon-ui` components — `Label`, `Button`, `Avatar` builder chains (test that they produce valid elements)
5. `mezon-native::autostart` — the pure builder logic

---

## 8. Running Tests in CI

The `justfile` already has the right commands. A CI step would run:

```bash
just lint        # cargo clippy + fmt check — must pass
just test        # cargo nextest run — must pass
just cov-summary # report coverage — fail if below threshold
```

To fail the build if coverage drops below 80%:
```bash
cargo llvm-cov --fail-under-lines 80
```

---

## 9. Test File Template

Copy this into any `src/foo.rs` file to start testing it:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_test_name_describes_what_should_happen() {
        // Arrange
        let input = ...;

        // Act
        let result = function_under_test(input);

        // Assert
        assert_eq!(result, expected);
    }
}
```

For async tests add `#[tokio::test]` and `async` to the signature.
For GPUI tests wrap the body in `TestApp::new().run(|cx| { ... })`.
