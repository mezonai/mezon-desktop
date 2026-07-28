# MMN Parity — React ⇄ Rust

Cách giữ **1:1** cho MỌI feature liên quan MMN (blockchain token) một cách dễ & tự-kiểm.
React (`../mezon`) là **single source of truth**.

## Quy trình (đọc trước khi port feature MMN mới)

1. **Inventory trước, không dò-theo-triệu-chứng.** Chạy `scripts/mmn-parity-audit.sh` → ra toàn bộ call-site MMN của React. Mỗi dòng phải có 1 hàng trong bảng dưới. Dòng nào chưa có = feature còn thiếu.
2. **Copy giá trị đúng chữ từ React.** Chuỗi/số/tên field (card text, note mặc định, amount, tên key `extra_info`) copy nguyên văn kèm chú thích `// React: <file>:<line>`. Không diễn giải lại.
3. **Golden test khoá wire-format** (xem cuối file) — để parity không âm thầm trôi. Sửa code làm lệch → test đỏ.
4. **Layer đúng chỗ:** logic/tx trong `mezon-store` (WalletStore), UI trong `mezon-ui`. Store không gọi thẳng `Shell` → qua `WalletToastBridge`. `mmn-client` không leak vào `mezon-ui`.
5. **Cập nhật bảng này** khi thêm/sửa 1 feature (giống `MASTER_CHECKLIST.md`).

Trạng thái: ✅ 1:1 · 🟡 một phần (thiếu UI/nhánh phụ) · ⬜ chưa làm

---

## Nền tảng

| Feature | React | Rust | TT |
|---|---|---|---|
| Config 4 URL + chainId | `MezonContext.createXClient` · env `NX_CHAT_APP_{MMN,INDEXER,ZK,DONG}_API_URL` | `config.rs` fields · `INDEXER_CHAIN_ID="1337"` | ✅ |
| id_token (JWT cho zk) | `session.id_token` | `Session.id_token` (auth.rs) | ✅ |
| 4 SDK client | `mmn-client-js` | crate `mmn-client` | ✅ |
| Ký Ed25519 + address | nacl · bs58(sha256(userId)) | ed25519-dalek · byte-for-byte | ✅ (test vector) |

## Client methods (mmn-client-js → crate mmn-client)

| React | Rust | TT |
|---|---|---|
| `generateEphemeralKeyPair` | `generate_ephemeral_key_pair` | ✅ |
| `getAddressFromUserId` | `address_from_user_id` | ✅ |
| `getAccountByUserId` | `get_account_by_user_id` | ✅ |
| `getCurrentNonce` | `get_current_nonce` | ✅ |
| `scaleAmountToDecimals` | `scale_amount_to_decimals` | ✅ |
| `sendTransaction` / `sendTransactionByAddress` | `send_transaction` / `send_transaction_by_address` | ✅ |
| `zkClient.getZkProofs` | `ZkClient.get_zk_proofs` | ✅ |
| `indexerClient.getTransactionByWallet` | `IndexerClient.get_transaction_by_wallet` | ✅ |
| `indexerClient.getTransactionByHash` | `IndexerClient.get_transaction_by_hash` | 🟡 (client có, UI detail chưa) |
| `dongClient.claimAmountRedEnvelopeQR` | `DongClient.claim_amount_red_envelope_qr` | 🟡 (client có, UI chưa) |

## Luồng (state + action)

| Feature | React | Rust | TT |
|---|---|---|---|
| Enable ví (zk) | `wallet.slice fetchZkProofs` (auth.slice gọi) | `WalletStore::enable_wallet` (observe AuthState) | ✅ |
| fetchWalletDetail | `getAccountByUserId` | `WalletStore::refresh_wallet` / enable | ✅ |
| sendTransaction core | `wallet.slice sendTransaction` | `WalletStore::send_transaction` | ✅ |
| updateWalletByAction | balance +/- | `apply_balance_delta` | ✅ |
| reset/logout | `resetState`/`setLogout` | `WalletStore::reset` (NotAuthenticated) | ✅ |
| **Transfer token** | `giveCoffee.slice sendToken` + FooterProfile | `WalletStore::send_token` + `send_token_modal.rs` | ✅ |
| — thẻ SendToken sau transfer | `sendNotificationMessage` (code 11) | `create_dm_and_send_token_card` | ✅ |
| **Give coffee** | `updateGiveCoffee` + reaction + card + 300ms | `messages.rs give_coffee_reaction` | ✅ |
| **Receive token** | `ontokensent`: balance + bankSound | `on_token_sent` + `play_bank_sound` | ✅ |
| — `handleSocketToken` counter | tokenUpdate/tokenSocket | — | ⬜ |
| — `setSendTokenEvent` | success event state | (toast đã có) | ⬜ |
| — `extra_attribute` unlock source | update emoji/sticker src | — | ⬜ |
| **Transaction history** | `transactionHistory.slice` + TransactionHistory | `transaction_history_modal.rs` (list + filter) | ✅ (cơ bản) |
| — Transaction detail | `TransactionDetail` (getTransactionByHash) | — | ⬜ |
| **buyItemForSale (UnlockItem)** | `emojiRecent.slice buyItemForSale` — mua emoji/sticker bằng token | — | ⬜ |
| **Red envelope QR claim** | `wallet.slice claimAmountRedEnvelopeQR` (DongClient) | client method có, UI/wiring chưa | ⬜ |
| **ModalWalletNotAvailable** | modal khi ví chưa bật | — | ⬜ |
| SendToken card render | `TokenTransactionMsg` | `token_transaction_card.rs` | ✅ |
| Context-menu gate theo `code===SendToken` | ẩn 1 số item | (cần rà `message_context_menu.rs`) | 🟡 |

## Golden values — copy đúng chữ (khoá bằng test)

| Giá trị | React | Rust |
|---|---|---|
| Give-coffee amount | `AMOUNT_TOKEN.TEN_THOUSAND_TOKENS = 10000` | `GIVE_COFFEE_AMOUNT = 10_000` |
| Decimals | `6` | `DECIMALS = 6` |
| chainId indexer | `'1337'` | `INDEXER_CHAIN_ID` |
| Message code SendToken | `TypeMessage.SendToken = 11` | `MESSAGE_CODE_SEND_TOKEN = 11` |
| DM mode | `STREAM_MODE_DM = 4` | `DirectKind::Dm.stream_mode()` |
| Card transfer | `` `Funds Transferred: ${formatMoney(x)}₫ | ${note}` `` | `"Funds Transferred: {}₫ | {note}"` |
| Card coffee | `` `${t('tokensSent')} ${formatMoney(10000)}₫ | ${t('giveCoffeeAction')}` `` | `"{tokensSent} 10,000₫ | {giveCoffeeAction}"` |
| Note transfer default | `t('transferFunds')` (ns `common`) | `common.transferFunds` |
| textData give-coffee | `'givecoffee'` | `"givecoffee"` |
| extraInfo transfer | `{type:transfer_token, UserReceiverId, UserSenderId, UserSenderUsername, ExtraAttribute}` | `ExtraInfo` 5 field |
| extraInfo give-coffee | `{type:give_coffee, ChannelId, ClanId, MessageRefId, UserReceiverId, UserSenderId, UserSenderUsername}` | `ExtraInfo` 7 field |

## Còn lại để đạt 100% (theo ưu tiên)
1. ⬜ **buyItemForSale (UnlockItem)** — mua emoji/sticker bằng token + cập nhật src khi nhận (nhánh `extra_attribute`).
2. ⬜ **Red envelope QR** — wiring `DongClient` + UI claim.
3. ⬜ **Transaction detail** modal (getTransactionByHash).
4. ⬜ **ModalWalletNotAvailable**.
5. ⬜ Counter `tokenUpdate`/`setSendTokenEvent` (badge token pending).
6. 🟡 Rà context-menu gate theo `code == SendToken`.
