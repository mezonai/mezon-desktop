# Release

Release work should be repeatable, auditable, and conservative. This document defines the engineering checklist; packaging details can be expanded as installers mature.

## Pre-Release Checks

```sh
just lint
just test
just safety
cargo build --release
```

Also verify:

- app starts cleanly on supported platforms
- logging does not expose secrets
- update checks use production endpoints
- migration tests pass
- native integrations fail gracefully when unavailable
- version metadata is correct

## Versioning

Workspace version is defined in the root `Cargo.toml` under `[workspace.package]`.

When changing version:

- update release notes
- update any updater metadata
- verify generated package metadata
- avoid unrelated dependency churn

## Update Metadata

`mezon-updater` should treat update metadata as untrusted input. Validate:

- version
- platform
- download URL
- integrity or signature fields
- minimum supported app version when applicable

## Build Artifacts

Release artifacts must be produced from a clean, reviewed commit. Do not create release builds from a dirty working tree unless the build is explicitly marked local or experimental.

## Rollback

Every release should have a rollback decision:

- disable update offer
- publish fixed metadata
- ship patch release
- document user mitigation

The updater must not assume every failure can be fixed by retrying.
