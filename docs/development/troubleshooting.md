# Troubleshooting

Use this guide for common local development failures.

## Clippy Fails

Run:

```sh
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Fix the lint locally. Use `#[allow]` only with a narrow scope and a reason.

## Tests Fail Under Nextest

Run a narrower test:

```sh
cargo nextest run -p mezon-store <test-name>
```

If the test is async and flaky, check for:

- sleeps instead of synchronization
- leaked tasks
- shared global state
- port or filesystem conflicts
- missing cancellation

## App Freezes

Likely causes:

- blocking IO in a GPUI render path
- CPU-heavy work on the UI thread
- lock held across `.await`
- channel send waiting forever
- platform API call made synchronously from UI

Move work to an owned async service or `spawn_blocking` when blocking is unavoidable.

## Native Feature Does Not Work

Check:

- OS permissions
- portal availability on Linux
- app identity and signing requirements
- platform-specific logs
- `mezon-native` error variants

Do not add UI-specific platform hacks. Fix or model the native boundary.

## Cargo Deny Fails

Run:

```sh
cargo deny check
cargo tree -i <crate>
```

Decide whether to:

- remove the dependency
- use an existing workspace dependency
- upgrade or pin a safe version
- add a documented policy exception

## Dependency Resolution Is Slow Or Surprising

Inspect the graph:

```sh
cargo tree -p mezon-app
cargo tree -d
```

Prefer workspace dependency declarations for shared crates.
