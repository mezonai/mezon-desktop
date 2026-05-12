# Transport Deep Dive

This document replaces the old transport notes (`TRANSPORT_*`, `QUICK_REFERENCE`, `LOGGING_GUIDE`) and describes the current Rust transport stack.

## Status

The core TypeScript transport reference has been ported. Rust code no longer depends on:

- `crates/mezon-client/src/abridged_tcp_adapter.ts`
- `crates/mezon-client/src/transport.ts`

Those files can be removed after any desired history/archive cleanup.

## Layering

```text
UI
↓
AppApi                       crates/mezon-client/src/app_api.rs
↓
TransportClient              crates/mezon-client/src/transport_runtime.rs
↓
MezonTransport               crates/mezon-client/src/transport.rs
↓
TransportAdapter trait       crates/mezon-client/src/transport_adapter.rs
↓
AbridgedTcpAdapter           crates/mezon-client/src/abridged_tcp_adapter.rs
↓
TCP/TLS socket
```

UI should call `AppApi` only. `AppApi` wraps the shared `TransportClient` so views do not parse `tcp_url`, create transports, reconnect, or install message callbacks. Do not call `MezonTransport` or `AbridgedTcpAdapter` directly from GPUI code.

Reason: GPUI tasks run on GPUI/smol executors, while TCP/TLS transport uses Tokio. `TransportClient` owns the dedicated Tokio runtime boundary; `AppApi` owns the UI-facing API surface.

## Files

| File | Role |
|---|---|
| `app_api.rs` | UI-facing API wrapper around shared `TransportClient`. Views should depend on this. |
| `transport_runtime.rs` | UI-safe wrapper, spawns transport operations on dedicated Tokio runtime. |
| `transport.rs` | API client, CID routing, pending request map, timeout handling, protobuf body/response handling. |
| `transport_adapter.rs` | Adapter trait and callback types. |
| `abridged_tcp_adapter.rs` | TCP/TLS wire protocol, handshake, ping, framing, read/write loop, RAW response reassembly. |
| `crates/mezon-proto/build.rs` | Generates protobuf types with `prost-build`. |
| `crates/mezon-proto/src/api.proto` | API request/response protobuf schema. |
| `crates/mezon-proto/src/realtime.proto` | Realtime envelope/event protobuf schema. |

## Runtime Boundary

`AppApi` is the public API surface for UI code. It delegates to the shared `TransportClient`, which exists because UI code cannot call Tokio transport internals directly.

Wrong from UI:

```rust
let host = session.tcp_host.as_deref().unwrap();
let port = 7349;
let transport = TransportClient::new(String::new());
transport.connect(host, port, &session.token, on_message, on_close).await?;
let account = transport.get_account().await?;
```

Correct from UI:

```rust
let api = self.api.clone();
cx.spawn(async move |_, cx| {
    let account = api.get_account().await?;
    let clans = api.list_clan_descs().await?;
    let channels = match clans.first() {
        Some(clan) => api.list_channel_descs(&clan.clan_id).await?,
        None => Vec::new(),
    };
    Ok::<_, anyhow::Error>((account, channels))
})
.detach();
```

Connection setup belongs to app bootstrap, not individual views:

1. Login updates `AuthState::Authenticated(session)`.
2. `spawn_transport_task` reads `session.tcp_host`, uses hardcoded dev port `7349`, and connects shared `TransportClient` once.
3. UI views call `AppApi` methods over that shared connection.

Calling Tokio APIs directly from GPUI tasks can panic:

```text
there is no reactor running
```

## Wire Protocol

The TCP/TLS stream uses a small prefix byte to identify packet type. `Envelope` is the protobuf payload format; these prefix bytes are the wire framing around it.

| Prefix | Type | Direction | Purpose |
|---|---|---|---|
| `0x00` | Ping/Pong | both | Health check with CID. |
| `0xff` | RAW API response | server -> client | API response with CID, response code, chunk flag, payload length, payload. |
| `0x01..0x7e` | Abridged small frame | mostly client -> server, server -> client possible | Protobuf `Envelope`, length = prefix * 4. |
| `0x7f` | Abridged extended frame | both | Protobuf `Envelope`, extended 24-bit length. |
| `0xef` | Handshake | client -> server | Initial token handshake after TLS. |

The practical classification is 3 runtime packet families plus the initial handshake:

1. Ping/Pong packets (`0x00`)
   Purpose: connection liveness only. They are intentionally tiny and bypass protobuf so the client can cheaply verify the socket and resolve a pending ping by CID.

2. RAW API response packets (`0xff`)
   Purpose: request/response completion. They carry `cid`, `response_code`, chunk/final flag, payload length, and protobuf response bytes. The adapter can resolve the exact pending API request without decoding a realtime `Envelope` first, and can reassemble chunked responses by CID.

3. Abridged protobuf frames (`0x01..0x7e`, `0x7f`)
   Purpose: general protobuf message transport. Client API requests and possible server-push realtime events use `realtime::Envelope` inside these frames. The abridged header only says how many payload bytes follow; protobuf decides whether the payload is `ApiRequestEvent`, channel event, message event, etc.

Handshake (`0xef`) is separate because it authenticates the socket before normal packet exchange starts.

Parser order in `AbridgedTcpAdapter::handle_data`:

```rust
if data[0] == 0x00 {
    // ping/pong
}

if data[0] == 0xff {
    // raw API response
}

if data[0] < 127 {
    // abridged small frame
} else if data[0] == 0x7f {
    // abridged extended frame
} else {
    // unexpected
}
```

`0xef` is client write-only in the current flow, so it is emitted during `connect()` and is not handled by `handle_data`.

### TLS

The TCP adapter uses `tokio-rustls`. Current dev behavior accepts the server certificate, matching TS `rejectUnauthorized: false` behavior. This should be gated or replaced before production.

### Handshake

After TLS handshake:

```text
0xef + token_len_div_4 + padded_token
```

Token bytes are padded to 4-byte alignment.

### Ping/Pong

Prefix:

```text
0x00
```

```text
00 + cid_be_u16
```

Example for CID `1`:

```text
00 00 01
```

The adapter routes pong by CID:

```rust
let cid = u16::from_be_bytes([data[1], data[2]]);
handlers.trigger_message(cid, 0, vec![]);
```

This resolves the pending `ping_roundtrip()` request.

### Abridged Framing

Prefix:

```text
0x01..0x7e  small frame
0x7f        extended frame
```

Small payloads:

```text
len_div_4 + payload
```

Large payloads:

```text
7f + len_div_4_le_24 + payload
```

Payloads are padded to 4-byte alignment before framing.

Example client request for `GetAccount`:

```text
05 08 02 e2 05 0e 08 01 12 0a 47 65 74 41 63 63 6f 75 6e 74 00
```

Breakdown:

```text
05                       abridged header: payload = 5 * 4 = 20 bytes
08 02                    Envelope.cid = 2
e2 05 0e                 Envelope.api_request_event field
08 01                    api_index = 1
12 0a                    api_name length = 10
47 65 74 41 63...        "GetAccount"
00                       padding
```

### RAW API Responses

Prefix:

```text
0xff
```

```text
ff + cid_be_u16 + code_be_u32 + payload_len_be_u32 + payload
```

The upper 16 bits of `code` are response code. The lower 16 bits are a chunk flag. `0xff` marks final chunk. The adapter reassembles chunks by CID before resolving the request.

Example response header from `GetAccount`:

```text
ff 00 02 00 00 00 ff 00 00 00 75 ...
```

Breakdown:

```text
ff              raw response prefix
00 02           cid = 2
00 00 00 ff     code = 0x000000ff
00 00 00 75     payload_len = 117
...             117-byte protobuf response payload
```

Then:

```text
response_code = code >> 16 = 0
fin_flag = code & 0xffff = 0xff
```

`0xff` as `fin_flag` means final chunk.

## Protobuf

Generated modules:

```rust
mezon_proto::api
mezon_proto::realtime
```

`realtime.proto` imports `Api/api.proto`. The build script copies `src/api.proto` into `$OUT_DIR/Api/api.proto` and compiles from there so `protoc` sees the same import path.

API requests are wrapped in:

```protobuf
message Envelope {
  int32 cid = 1;
  oneof message {
    ApiRequestEvent api_request_event = 92;
  }
}

message ApiRequestEvent {
  int32 api_index = 1;
  string api_name = 2;
  bytes body = 3;
}
```

The field tag for `api_request_event = 92` appears on wire as:

```text
e2 05
```

## API Indexes

`transport.rs` keeps TS-compatible `ApiNameEnum` indexes for currently exposed methods:

| API | Index |
|---|---:|
| `ListChannelDescs` | 0 |
| `GetAccount` | 1 |
| `ListClanDescs` | 2 |
| `ListClanUsers` | 3 |
| `ListRoles` | 4 |
| `GetNotificationClan` | 9 |
| `ListMutedChannel` | 10 |
| `ListFriends` | 14 |
| `ListChannelMessages` | 30 |
| `AddFriends` | 57 |
| `CreateChannelDesc` | 66 |
| `DeleteChannelDesc` | 72 |
| `DeleteAccount` | 75 |
| `DeleteFriends` | 76 |
| `UpdateAccount` | 106 |
| `SendChannelMessage` | 180 |

When adding a new API method, copy its exact index from the source enum and add a generated request/response decode path.

## Implemented API Methods

Implemented with generated protobuf request/response handling:

- `get_account()` -> `api::Account`
- `list_channel_descs()` -> `api::ChannelDescList`
- `list_clan_descs()` -> `api::ClanDescList`
- `list_channel_messages()` -> `api::ChannelMessageList`
- `send_channel_message()` -> `api::ChannelMessage`
- `list_friends()` -> `api::FriendList`
- `get_notification_clan()` -> `api::NotificationClan` request
- `list_muted_channels(clan_id)` -> `api::MutedChannelList`
- `create_channel()` -> `api::CreateChannelDescRequest` + `api::ChannelDescription`
- `delete_channel()` -> `api::DeleteChannelDescRequest`
- `add_friend()` -> `api::AddFriendsRequest`
- `delete_friend()` -> `api::DeleteFriendsRequest`
- `update_account()` -> `api::UpdateAccountRequest`
- `delete_account()` -> empty body like TS

UI-facing methods exposed through `AppApi`:

- `get_account()`
- `list_clan_descs()`
- `list_channel_descs(clan_id)`

## UI Usage Pattern

Authenticated views receive `Arc<AppApi>` from `RootView`. They should call API methods directly and update their own entity state from the async result.

Example from `AccountTestView`:

```rust
let result = match api.get_account().await {
    Ok(account) => match api.list_clan_descs().await {
        Ok(clans) => {
            let clan = clans.into_iter().next();
            match clan.as_ref() {
                Some(clan) => match api.list_channel_descs(&clan.clan_id).await {
                    Ok(channels) => Ok((account, Some(clan.clone()), channels)),
                    Err(e) => Err(e),
                },
                None => Ok((account, None, Vec::new())),
            }
        }
        Err(e) => Err(e),
    },
    Err(e) => Err(e),
};
```

Do not do this in views:

1. Parse `tcp_url`.
2. Create a new `TransportClient`.
3. Connect.
4. Install callbacks.
5. Sleep for handshake readiness.
6. Call one API.
7. Close transport.

Expected success pattern:

```text
Shared TCP transport connected
get_account over shared TCP succeeded
TransportClient::list_clan_descs() called
TransportClient::list_channel_descs() called
📥 RAW API response detected
```

## Debugging

Useful commands:

```bash
RUST_LOG=mezon=debug,info just run
RUST_LOG=mezon_client::transport=debug just run
RUST_LOG=mezon_client::abridged_tcp_adapter=trace just run
RUST_LOG=mezon=trace,info just run
```

Common issues:

| Symptom | Likely Cause |
|---|---|
| `there is no reactor running` | UI called Tokio transport internals directly. Use `TransportClient`. |
| View repeats host/port/connect boilerplate | UI bypassed `AppApi`. Inject and call `Arc<AppApi>` instead. |
| Ping timeout | Handshake not processed/flushed before ping, invalid token, or socket not reading. |
| `ListChannelDescs` returns code `3` | Request likely missing `clan_id`; call `list_clan_descs()` first, then `list_channel_descs(&clan_id)`. |
| `can not unmarshal` | API index/body protobuf mismatch. Check `api_request_event = 92` and generated body type. |
| No `READ ... bytes` after write | Server did not respond; inspect token, endpoint, request bytes. |
| `READ` occurs but no pending request resolves | CID mismatch or parser routed as server-push. |

## Verification

```bash
cargo check -p mezon-proto -p mezon-client -p mezon-ui --message-format short
cargo test -p mezon-proto
```

## Remaining Work

- Add unit tests for abridged frame parsing and RAW response chunk reassembly.
- Add mock adapter tests for CID routing and timeout behavior.
- Gate or replace dev-only no-cert verifier before production.
- Port additional API methods only when product code needs them.
