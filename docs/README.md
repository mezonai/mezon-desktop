# Mezon Desktop — Developer Docs

This folder documents the codebase for developers new to Rust and/or desktop app development.
Each file covers a specific area. Read them in order if you're starting from scratch.

## Reading Order

1. [Architecture Overview](./01-architecture-overview.md) — big picture, crate structure, tech stack
2. [App Startup: main.rs](./02-app-startup.md) — how the app boots, step by step
3. [Local Data Management](./03-local-data.md) — settings file, OS keychain, what's persisted
4. [GPUI: The UI Framework](./04-gpui-framework.md) — how GPUI works (with React analogies)
5. [Migration Plan](./05-migration-plan.md) — the full 15-stage Electron → Rust roadmap
6. [Authentication Flow](./06-auth-flow.md) — HTTP client, JWT, LoginView state machine, session persistence
7. [UI Components](./07-ui-components.md) — theme tokens, primitives, compositions, RootView layout
8. [Native Platform APIs](./08-native-apis.md) — tray, badges, notifications, autostart, deep links, single-instance
9. [GPUI Internals](./09-gpui-internals.md) — Entity ownership, notify/observe/emit, TextInput pipeline, async patterns, dual runtime
10. [Stubs and Future Work](./10-stubs-and-future.md) — mezon-proto, mezon-updater, unconnected settings, Stage 2 insertion points
11. [Known Issues](./11-known-issues.md) — all CRITICAL/HIGH/MEDIUM bugs and safety gaps with fix guidance
12. [Testing Guide](./12-testing-guide.md) — how to write unit, async, and GPUI tests; coverage targets
13. [Contributing Guide](./13-contributing.md) — workflow, crate boundaries, adding components/screens/native modules
14. [Native Module Deep Dive](./14-native-deep-dive.md) — tray icon loading, badge GDI bitmap, deep link registry/xdg, CFNotificationCenter trampoline, Windows message loop
15. [UI Primitives Deep Dive](./15-ui-primitives-deep-dive.md) — Spinner GPUI animation, Icon SVG system, inline const approach
16. [Transport Deep Dive](./16-transport-deep-dive.md) — Tokio runtime wrapper, TCP/TLS abridged protocol, generated protobuf API requests

## Quick Reference

| Topic | File |
|-------|------|
| Build & run commands | [`../CLAUDE.md`](../CLAUDE.md) |
| Crate dependency map | [Architecture Overview](./01-architecture-overview.md#crate-dependency-map) |
| How settings are saved | [Local Data](./03-local-data.md#1-settings-file) |
| How auth tokens are stored | [Local Data](./03-local-data.md#2-session-tokens-os-keychain) |
| What runs before the window opens | [App Startup](./02-app-startup.md) |
| Current project status | [Migration Plan](./05-migration-plan.md#current-status) |
| OTP vs password login | [Auth Flow](./06-auth-flow.md#4-the-login-view) |
| Theme color tokens | [UI Components](./07-ui-components.md#1-theme-system-themers) |
| How AuthState drives the UI | [Auth Flow](./06-auth-flow.md#5-how-rootview-uses-authstate) |
| OS tray / notifications / deep links | [Native APIs](./08-native-apis.md) |
| Single-instance lock | [Native APIs](./08-native-apis.md#7-single-instance-lock-instancers) |
| GPUI Entity ownership model | [GPUI Internals](./09-gpui-internals.md#1-the-ownership-model) |
| How TextInput handles IME / typing | [GPUI Internals](./09-gpui-internals.md#5-the-textinput-pipeline) |
| Why there are two async runtimes | [GPUI Internals](./09-gpui-internals.md#9-the-dual-runtime) |
| What's a stub vs real code | [Stubs & Future](./10-stubs-and-future.md) |
| Where Stage 2 plugs in | [Stubs & Future](./10-stubs-and-future.md#9-stage-2-integration-map) |
| All known bugs (CRITICAL/HIGH/MEDIUM) | [Known Issues](./11-known-issues.md) |
| How to write tests for this codebase | [Testing Guide](./12-testing-guide.md) |
| How to add a component / screen / module | [Contributing](./13-contributing.md) |
| Crate boundaries — what goes where | [Contributing](./13-contributing.md#2-crate-boundaries--what-goes-where) |
| Tray icon loading / fallback | [Native Deep Dive](./14-native-deep-dive.md#1-tray-mezon-nativesrctrayrs) |
| Windows badge GDI bitmap generation | [Native Deep Dive](./14-native-deep-dive.md#2-badge-mezon-nativesrcbadgers) |
| Deep link registry / .desktop file layout | [Native Deep Dive](./14-native-deep-dive.md#3-deep-links-mezon-nativesrcdeep_linkrs) |
| macOS CFNotificationCenter trampoline pattern | [Native Deep Dive](./14-native-deep-dive.md#macos-cfnotificationcenter-trampoline-pattern) |
| Windows power event message loop | [Native Deep Dive](./14-native-deep-dive.md#windows-message-only-window--wtsregistersessionnotification) |
| Spinner animation (GPUI Animation / rotate) | [UI Primitives Deep Dive](./15-ui-primitives-deep-dive.md#1-spinner-mezon-uisrccomponentsprimitivesspin) |
| Icon SVG system and inline const approach | [UI Primitives Deep Dive](./15-ui-primitives-deep-dive.md#2-icon-mezon-uisrccomponentsprimitivesvicons) |
| TCP transport runtime / protocol | [Transport Deep Dive](./16-transport-deep-dive.md) |
