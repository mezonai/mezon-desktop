# Security

Desktop clients handle credentials, local files, OS integration, and untrusted network data. Treat security as a default engineering concern.

## Secrets

Secrets must not be:

- logged
- stored in plain-text settings
- rendered into crash reports
- included in notification bodies
- passed through debug formatting

Use keychain or platform credential storage for tokens.

## Auth

Represent auth states explicitly:

```rust
pub enum AuthState {
    Unknown,
    Unauthenticated,
    Authenticating,
    Authenticated(Session),
    Expired,
}
```

Do not infer authentication from `Option<String>` token presence alone.

## Input Validation

Validate at trust boundaries:

- REST responses
- protobuf messages
- deep links
- local settings files
- update metadata
- environment variables

```rust
let url = DeepLink::parse(raw_url).map_err(NativeError::InvalidDeepLink)?;
```

## Update Safety

Update checks must validate source, version, and metadata. Any executable update path must include signature or integrity verification before installation.

## File System

Avoid following untrusted paths without normalization. Keep app data under the platform app-data directory. Do not write secrets into logs or temp files.

## Dependency Security

Run:

```sh
just safety
cargo deny check
```

Any new dependency should be justified in review, especially if it handles networking, parsing, crypto, compression, or native APIs.

## Error Messages

User-facing errors should not reveal secrets, filesystem internals, tokens, or raw server payloads.
