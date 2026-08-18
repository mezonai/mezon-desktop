use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_audio::{AudioPlayer, decode_audio};
use mezon_client::RealtimeEvent;
use mezon_client::transport_runtime::http_client_arc;
use mmn_client::{
    AddTxResponse, ClaimRedEnvelopeQrRequest, ClaimRedEnvelopeQrResponse, DECIMALS, DongClient,
    EphemeralKeyPair, ExtraInfo, GetZkProofRequest, IndexerClient, MmnClient,
    SendTransactionRequest, Transaction, ZkClient, ZkClientType, ZkProof, address_from_user_id,
    generate_ephemeral_key_pair, is_secure_endpoint, scale_amount_to_decimals,
};

use mezon_client::Session;

use crate::AuthState;
use crate::Settings;
use crate::cache::Freshness;
use crate::config::{AppConfig, INDEXER_CHAIN_ID};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::wallet_persist::{self, PersistedWalletState};

const GIVE_COFFEE_AMOUNT: i64 = 10_000;

fn wallet_message(cx: &App, key: &'static str) -> String {
    let locale = Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default();
    mezon_i18n::t(&locale, key).to_string()
}

fn id_token_valid_for(jwt: &str) -> Option<i64> {
    mezon_client::jwt_expires_at(jwt).map(|exp| exp as i64 - mezon_client::server_now_secs() as i64)
}

fn now_secs() -> i64 {
    mezon_client::server_now_secs() as i64
}

fn should_refresh_balance(refreshing: bool, force: bool, same_user: bool, fresh: bool) -> bool {
    if refreshing {
        return false;
    }
    force || !same_user || !fresh
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenDirection {
    Received,
    Sent,
    Unrelated,
}

fn token_direction(my_id: &str, sender_id: &str, receiver_id: &str) -> TokenDirection {
    if my_id.is_empty() {
        return TokenDirection::Unrelated;
    }
    if receiver_id == my_id {
        TokenDirection::Received
    } else if sender_id == my_id {
        TokenDirection::Sent
    } else {
        TokenDirection::Unrelated
    }
}

fn balance_after_delta(current: &str, scaled: &str, add: bool) -> Option<String> {
    let current: i128 = current.trim().parse().ok()?;
    let delta: i128 = scaled.trim().parse().ok()?;
    let next = if add {
        current.saturating_add(delta)
    } else {
        current.saturating_sub(delta)
    };
    Some(next.max(0).to_string())
}

static BANK_SOUND: &[u8] = include_bytes!("../assets/audio/bankSound.mp3");

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletDetail {
    pub address: String,
    pub balance: String,
}

#[derive(Debug, Clone, Default)]
pub struct WalletTransaction {
    pub sent: bool,
    pub value: String,
    pub counterparty: String,
    pub note: String,
    pub hash: String,
    pub timestamp: i64,
    pub sender_user_id: Option<String>,
    pub receiver_user_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WalletTransactionPage {
    pub transactions: Vec<WalletTransaction>,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct TransactionCursor {
    pub timestamp: String,
    pub hash: String,
}

impl TransactionCursor {
    pub fn after(transaction: &WalletTransaction) -> Option<Self> {
        let timestamp = chrono::DateTime::from_timestamp(transaction.timestamp, 0)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Some(Self {
            timestamp,
            hash: transaction.hash.clone(),
        })
    }
}

fn map_transaction(transaction: Transaction, address: &str) -> WalletTransaction {
    let sent = transaction.from_address == address;
    let counterparty = if sent {
        transaction.to_address
    } else {
        transaction.from_address
    };
    let extra_info = serde_json::from_str::<ExtraInfo>(&transaction.extra_info).ok();
    WalletTransaction {
        sent,
        value: transaction.value,
        counterparty,
        note: transaction.text_data,
        hash: transaction.hash,
        timestamp: transaction.transaction_timestamp,
        sender_user_id: extra_info.as_ref().and_then(|e| e.user_sender_id.clone()),
        receiver_user_id: extra_info.and_then(|e| e.user_receiver_id),
    }
}

#[derive(Debug, Clone)]
pub enum WalletEvent {
    Enabled,
    BalanceChanged,
    TransactionSent { tx_hash: String },
    TokenReceived { amount: i64, sender_id: String },
    CoffeeSent,
    FlowerSent,
    FlowerUncertain,
    SendFailed { message: String },
    EnableFailed { message: String },
}

pub struct SendTokenRequest {
    pub sender: String,
    pub recipient: String,
    pub amount: i64,
    pub note: Option<String>,
    pub extra_info: Option<ExtraInfo>,
    pub by_address: bool,
}

struct WalletClients {
    mmn: MmnClient,
    indexer: Option<IndexerClient>,
    zk: ZkClient,
    dong: DongClient,
}

pub struct WalletStore {
    auth_state: Entity<AuthState>,
    clients: Option<Arc<WalletClients>>,
    wallet: Option<WalletDetail>,
    zk_proofs: Option<ZkProof>,
    ephemeral: Option<EphemeralKeyPair>,
    proof_minted_at: Option<i64>,
    failed_id_token_exp: Option<u64>,
    is_enabled: bool,
    pending_give_coffee: bool,
    pending_give_flower: bool,
    enabled_user: Option<String>,
    enabling_user: Option<String>,
    auth_user: Option<String>,
    balance_user: Option<String>,
    balance_refreshing: bool,
    balance_freshness: Freshness,
    reset_generation: u64,
    enable_generation: u64,
    enable_task: Option<Task<()>>,
    bank_player: Option<AudioPlayer>,
    bank_sound_loading: bool,
    _auth_sub: Subscription,
}

struct GlobalWalletStore(Entity<WalletStore>);
impl Global for GlobalWalletStore {}

impl EventEmitter<WalletEvent> for WalletStore {}

impl WalletStore {
    pub fn init(auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            let auth_sub = cx.observe(&auth_state, Self::on_auth_changed);
            let clients = Self::build_clients(cx);
            Self::register_realtime(cx);
            let mut this = Self {
                auth_state: auth_state.clone(),
                clients,
                wallet: None,
                zk_proofs: None,
                ephemeral: None,
                proof_minted_at: None,
                failed_id_token_exp: None,
                is_enabled: false,
                pending_give_coffee: false,
                pending_give_flower: false,
                enabled_user: None,
                enabling_user: None,
                auth_user: None,
                balance_user: None,
                balance_refreshing: false,
                balance_freshness: Freshness::new(),
                reset_generation: 0,
                enable_generation: 0,
                enable_task: None,
                bank_player: None,
                bank_sound_loading: false,
                _auth_sub: auth_sub,
            };
            this.sync_from_auth(cx);
            this
        });
        cx.set_global(GlobalWalletStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalWalletStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalWalletStore>().map(|g| g.0.clone())
    }

    pub fn is_enabled(&self) -> bool {
        self.is_enabled
    }

    pub fn is_available(&self) -> bool {
        self.is_enabled && self.zk_proofs.is_some() && self.ephemeral.is_some()
    }

    pub fn wallet(&self) -> Option<&WalletDetail> {
        self.wallet.as_ref()
    }

    pub fn balance(&self) -> Option<&str> {
        self.wallet.as_ref().map(|w| w.balance.as_str())
    }

    pub fn address(&self) -> Option<&str> {
        self.wallet.as_ref().map(|w| w.address.as_str())
    }

    pub fn pending_give_coffee(&self) -> bool {
        self.pending_give_coffee
    }

    pub fn set_pending_give_coffee(&mut self, pending: bool) {
        self.pending_give_coffee = pending;
    }

    pub fn pending_give_flower(&self) -> bool {
        self.pending_give_flower
    }

    pub fn set_pending_give_flower(&mut self, pending: bool) {
        self.pending_give_flower = pending;
    }

    pub fn give_coffee_amount() -> i64 {
        GIVE_COFFEE_AMOUNT
    }

    fn build_clients(cx: &App) -> Option<Arc<WalletClients>> {
        let config = AppConfig::try_global(cx)?;
        if config.mmn_api_url.is_empty() || config.zk_api_url.is_empty() {
            tracing::error!(
                "wallet disabled: mmn_api_url/zk_api_url unset (mmn={}, zk={})",
                !config.mmn_api_url.is_empty(),
                !config.zk_api_url.is_empty()
            );
            return None;
        }
        let endpoints = [
            ("mmn_api_url", config.mmn_api_url.as_str()),
            ("zk_api_url", config.zk_api_url.as_str()),
            ("indexer_api_url", config.indexer_api_url.as_str()),
            ("dong_service_api_url", config.dong_service_api_url.as_str()),
        ];
        for (name, url) in endpoints {
            if !url.is_empty() && !is_secure_endpoint(url) {
                tracing::error!("wallet disabled: {name} is not an https endpoint");
                return None;
            }
        }
        if config.indexer_api_url.is_empty() {
            tracing::warn!("wallet: indexer_api_url is unset, transaction history is disabled");
        }
        if config.dong_service_api_url.is_empty() {
            tracing::warn!(
                "wallet: dong_service_api_url is unset, QR red envelope claim is disabled"
            );
        }
        let http = http_client_arc();
        Some(Arc::new(WalletClients {
            mmn: MmnClient::new(http.clone(), config.mmn_api_url.clone()),
            indexer: (!config.indexer_api_url.is_empty()).then(|| {
                IndexerClient::new(
                    http.clone(),
                    config.indexer_api_url.clone(),
                    INDEXER_CHAIN_ID,
                )
            }),
            zk: ZkClient::new(http.clone(), config.zk_api_url.clone()),
            dong: DongClient::new(http, config.dong_service_api_url.clone()),
        }))
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::TokenSent, &entity, |this, event, cx| {
                this.on_token_sent(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| {
                if let Some(user_id) = this.enabled_user.clone() {
                    this.refresh_wallet(user_id, cx);
                }
            });
        });
    }

    fn on_auth_changed(this: &mut Self, _auth: Entity<AuthState>, cx: &mut Context<Self>) {
        this.sync_from_auth(cx);
    }

    fn sync_from_auth(&mut self, cx: &mut Context<Self>) {
        let user_id = match self.auth_state.read(cx) {
            AuthState::NotAuthenticated => {
                self.reset(cx);
                return;
            }
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.clone()
            }
            _ => return,
        };
        self.auth_user = Some(user_id.clone());
        self.try_restore_persisted(&user_id, cx);
        if !self.is_available() {
            self.enable_wallet_for_current_user(false, cx);
        }
        self.ensure_wallet_balance(user_id, cx);
    }

    fn try_restore_persisted(&mut self, user_id: &str, cx: &mut Context<Self>) {
        if user_id.is_empty() {
            return;
        }
        if self.is_enabled && self.enabled_user.as_deref() == Some(user_id) {
            return;
        }
        let Some(stored) = wallet_persist::load_wallet() else {
            return;
        };
        if stored.user_id != user_id {
            return;
        }
        if !stored.is_enabled {
            return;
        }
        let (Some(zk_proofs), Some(ephemeral)) = (stored.zk_proofs, stored.ephemeral) else {
            return;
        };
        self.wallet = stored.wallet;
        self.zk_proofs = Some(zk_proofs);
        self.ephemeral = Some(ephemeral);
        self.proof_minted_at = stored.proof_minted_at;
        self.is_enabled = true;
        self.enabled_user = Some(user_id.to_string());
        cx.notify();
    }

    pub fn fetch_zk_proofs_after_login(&mut self, session: &Session, cx: &mut Context<Self>) {
        if session.id_token.is_empty() || session.user_id.is_empty() {
            return;
        }
        self.reset(cx);
        self.enable_wallet(session.id_token.clone(), session.user_id.clone(), true, cx);
    }

    pub fn enable_wallet_for_current_user(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((jwt, user_id)) = self
            .auth_state
            .read(cx)
            .session_credentials()
            .filter(|(jwt, uid)| !jwt.is_empty() && !uid.is_empty())
        else {
            if force {
                cx.emit(WalletEvent::EnableFailed {
                    message: "Wallet is not available".to_string(),
                });
            }
            return;
        };
        self.enable_wallet(jwt, user_id, force, cx);
    }

    fn enable_wallet(&mut self, jwt: String, user_id: String, force: bool, cx: &mut Context<Self>) {
        if jwt.is_empty() || user_id.is_empty() {
            return;
        }
        if !force && self.is_enabled && self.enabled_user.as_deref() == Some(user_id.as_str()) {
            return;
        }
        if !force && self.enabling_user.as_deref() == Some(user_id.as_str()) {
            return;
        }
        let id_token_exp = mezon_client::jwt_expires_at(&jwt);
        if !force && id_token_exp.is_some() && id_token_exp == self.failed_id_token_exp {
            return;
        }
        let id_token_valid_for = id_token_valid_for(&jwt);
        if id_token_valid_for.is_some_and(|left| left <= 0) {
            self.failed_id_token_exp = id_token_exp;
            tracing::warn!(
                id_token_valid_for = ?id_token_valid_for,
                "wallet: id_token expired, skipping the zk proof fetch — only a fresh login mints a new one"
            );
            if force {
                cx.emit(WalletEvent::EnableFailed {
                    message: "Session expired — sign in again to use the wallet".to_string(),
                });
            }
            return;
        }
        let Some(clients) = self.clients.clone() else {
            tracing::error!("wallet: enable skipped, mmn/zk clients are not configured");
            if force {
                cx.emit(WalletEvent::EnableFailed {
                    message: wallet_message(cx, "message.wallet.notConfigured"),
                });
            }
            return;
        };
        let switching_user = self
            .enabled_user
            .as_deref()
            .or(self.enabling_user.as_deref())
            .is_some_and(|current| current != user_id.as_str());
        if switching_user {
            self.reset(cx);
        }
        let generation = self.reset_generation;
        self.enable_generation = self.enable_generation.wrapping_add(1);
        let enable_generation = self.enable_generation;
        let report_failure = force;
        self.enabling_user = Some(user_id.clone());
        self.enable_task = Some(cx.spawn(async move |this, cx| {
            let ephemeral = generate_ephemeral_key_pair().ok();
            let (zk_proofs, account, failure) = match &ephemeral {
                Some(ephemeral) => {
                    let request = GetZkProofRequest {
                        user_id: user_id.clone(),
                        ephemeral_public_key: ephemeral.public_key.clone(),
                        jwt,
                        address: address_from_user_id(&user_id),
                        client_type: ZkClientType::Mezon,
                    };
                    match clients.zk.get_zk_proofs(request).await {
                        Ok(proofs) => {
                            tracing::info!(
                                id_token_valid_for = ?id_token_valid_for,
                                "wallet: zk proof minted"
                            );
                            let account = clients.mmn.get_account_by_user_id(&user_id).await.ok();
                            (Some(proofs), account, None)
                        }
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                id_token_valid_for = ?id_token_valid_for,
                                "wallet: zk proof fetch failed"
                            );
                            (None, None, Some(error.to_string()))
                        }
                    }
                }
                None => (
                    None,
                    None,
                    Some("Could not generate the wallet key pair".to_string()),
                ),
            };
            this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    if report_failure {
                        cx.emit(WalletEvent::EnableFailed {
                            message: wallet_message(cx, "message.wallet.enableCancelled"),
                        });
                    }
                    return;
                }
                if this.enable_generation != enable_generation {
                    return;
                }
                if this.enabling_user.as_deref() == Some(user_id.as_str()) {
                    this.enabling_user = None;
                }
                let (Some(ephemeral), Some(zk_proofs)) = (ephemeral, zk_proofs) else {
                    this.failed_id_token_exp = id_token_exp;
                    if let (true, Some(message)) = (report_failure, failure) {
                        cx.emit(WalletEvent::EnableFailed { message });
                    }
                    return;
                };
                this.failed_id_token_exp = None;
                this.ephemeral = Some(ephemeral);
                this.zk_proofs = Some(zk_proofs);
                this.proof_minted_at = Some(now_secs());
                this.is_enabled = true;
                this.enabled_user = Some(user_id.clone());
                if let Some(account) = account {
                    this.wallet = Some(WalletDetail {
                        address: account.address,
                        balance: account.balance,
                    });
                }
                this.persist_wallet_state();
                cx.emit(WalletEvent::Enabled);
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn refresh_wallet(&mut self, user_id: String, cx: &mut Context<Self>) {
        self.refresh_balance(user_id, true, cx);
    }

    pub fn ensure_wallet_balance(&mut self, user_id: String, cx: &mut Context<Self>) {
        self.refresh_balance(user_id, false, cx);
    }

    fn refresh_balance(&mut self, user_id: String, force: bool, cx: &mut Context<Self>) {
        if user_id.is_empty() {
            return;
        }
        let same_user = self.balance_user.as_deref() == Some(user_id.as_str());
        let fresh = self.balance_freshness.is_fresh(crate::CACHE_TTL);
        if !should_refresh_balance(self.balance_refreshing, force, same_user, fresh) {
            return;
        }
        let Some(clients) = self.clients.clone() else {
            return;
        };
        let generation = self.reset_generation;
        self.balance_refreshing = true;
        cx.spawn(async move |this, cx| {
            let account = clients.mmn.get_account_by_user_id(&user_id).await;
            this.update(cx, |this, cx| {
                this.balance_refreshing = false;
                if this.reset_generation != generation {
                    return;
                }
                match account {
                    Ok(account) => {
                        this.balance_user = Some(user_id);
                        this.balance_freshness.mark_fetched();
                        this.wallet = Some(WalletDetail {
                            address: account.address,
                            balance: account.balance,
                        });
                        cx.emit(WalletEvent::BalanceChanged);
                        cx.notify();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "wallet: balance refresh failed");
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn send_transaction(
        &mut self,
        request: SendTokenRequest,
        cx: &mut Context<Self>,
    ) -> Task<Result<AddTxResponse, String>> {
        let Some(clients) = self.clients.clone() else {
            return Task::ready(Err("Wallet is not configured".to_string()));
        };
        let (Some(zk_proofs), Some(ephemeral)) = (self.zk_proofs.clone(), self.ephemeral.clone())
        else {
            return Task::ready(Err("Wallet is not available".to_string()));
        };
        if request.sender.is_empty() {
            return Task::ready(Err("Wallet is not available".to_string()));
        }
        if request.recipient.is_empty() {
            return Task::ready(Err("You must select a user".to_string()));
        }
        if request.amount <= 0 {
            return Task::ready(Err("Amount must be greater than zero".to_string()));
        }
        let scaled = match scale_amount_to_decimals(&request.amount.to_string(), DECIMALS) {
            Ok(value) => value,
            Err(error) => return Task::ready(Err(error.to_string())),
        };
        if let Some(balance) = self.balance()
            && !mmn_client::validate_amount(balance, &scaled)
        {
            return Task::ready(Err("Amount exceeds wallet balance".to_string()));
        }

        let SendTokenRequest {
            sender,
            recipient,
            note,
            extra_info,
            by_address,
            ..
        } = request;

        let proof_age = self.proof_minted_at.map(|minted| now_secs() - minted);
        let id_token_left = self
            .auth_state
            .read(cx)
            .session_credentials()
            .and_then(|(jwt, _)| id_token_valid_for(&jwt));
        tracing::info!(
            proof_age = ?proof_age,
            id_token_valid_for = ?id_token_left,
            "wallet: sending a transaction"
        );

        cx.spawn(async move |this, cx| {
            let nonce = clients.mmn.get_current_nonce(&sender, "pending").await;
            let nonce = match nonce {
                Ok(response) if response.error.is_empty() => response.nonce,
                Ok(response) => return Err(response.error),
                Err(error) => return Err(error.to_string()),
            };

            let tx_request = SendTransactionRequest {
                sender: sender.clone(),
                recipient: recipient.clone(),
                amount: scaled,
                nonce: nonce + 1,
                timestamp: None,
                text_data: note,
                private_key: ephemeral.private_key.clone(),
                extra_info,
                zk_proof: zk_proofs.proof.clone(),
                zk_pub: zk_proofs.public_input.clone(),
                public_key: ephemeral.public_key.clone(),
            };

            let result = if by_address {
                clients.mmn.send_transaction_by_address(tx_request).await
            } else {
                clients.mmn.send_transaction(tx_request).await
            };

            let response = match result {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        proof_age = ?proof_age,
                        "wallet: transaction request failed"
                    );
                    return Err(error.to_string());
                }
            };
            if !response.ok {
                let message = if response.error.is_empty() {
                    "Transaction failed".to_string()
                } else {
                    response.error.clone()
                };
                tracing::warn!(
                    message,
                    proof_age = ?proof_age,
                    "wallet: transaction rejected by MMN"
                );
                return Err(message);
            }
            tracing::info!(proof_age = ?proof_age, "wallet: transaction accepted");

            let tx_hash = response.tx_hash.clone();
            this.update(cx, |_this, cx| {
                cx.emit(WalletEvent::TransactionSent { tx_hash });
                cx.notify();
            })
            .ok();
            Ok(response)
        })
    }

    pub fn send_token(
        &mut self,
        sender: String,
        sender_username: String,
        recipient: String,
        amount: i64,
        note: Option<String>,
        extra_attribute: Option<String>,
        by_address: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<AddTxResponse, String>> {
        let extra_info = ExtraInfo {
            transfer_type: mmn_client::TRANSFER_TYPE_TRANSFER_TOKEN.to_string(),
            user_receiver_id: Some(recipient.clone()),
            user_sender_id: Some(sender.clone()),
            user_sender_username: Some(sender_username),
            extra_attribute: Some(extra_attribute.unwrap_or_default()),
            ..Default::default()
        };
        self.send_transaction(
            SendTokenRequest {
                sender,
                recipient,
                amount,
                note,
                extra_info: Some(extra_info),
                by_address,
            },
            cx,
        )
    }

    pub fn load_wallet_transactions(
        &self,
        address: String,
        filter: i32,
        cursor: Option<TransactionCursor>,
        cx: &mut Context<Self>,
    ) -> Task<Result<WalletTransactionPage, String>> {
        let Some(clients) = self.clients.clone() else {
            return Task::ready(Err("Wallet is not configured".to_string()));
        };
        cx.spawn(async move |_this, _cx| {
            let Some(indexer) = clients.indexer.as_ref() else {
                return Err("Transaction history is not configured".to_string());
            };
            let (timestamp_lt, last_hash) = match &cursor {
                Some(cursor) => (Some(cursor.timestamp.as_str()), Some(cursor.hash.as_str())),
                None => (None, None),
            };
            let response = indexer
                .get_transactions_by_wallet_before_timestamp(
                    &address,
                    filter,
                    None,
                    timestamp_lt,
                    last_hash,
                )
                .await
                .map_err(|error| error.to_string())?;
            let has_more = response.meta.has_more.unwrap_or(false);
            let transactions = response
                .data
                .unwrap_or_default()
                .into_iter()
                .map(|tx| map_transaction(tx, &address))
                .collect();
            Ok(WalletTransactionPage {
                transactions,
                has_more,
            })
        })
    }

    pub fn wallet_transaction_detail(
        &self,
        hash: String,
        address: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<WalletTransaction, String>> {
        let Some(clients) = self.clients.clone() else {
            return Task::ready(Err("Wallet is not configured".to_string()));
        };
        cx.spawn(async move |_this, _cx| {
            let Some(indexer) = clients.indexer.as_ref() else {
                return Err("Transaction history is not configured".to_string());
            };
            let transaction = indexer
                .get_transaction_by_hash(&hash)
                .await
                .map_err(|error| error.to_string())?;
            Ok(map_transaction(transaction, &address))
        })
    }

    pub fn claim_amount_red_envelope(
        &self,
        id: String,
        user_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<ClaimRedEnvelopeQrResponse, String>> {
        let Some(clients) = self.clients.clone() else {
            return Task::ready(Err("Wallet is not configured".to_string()));
        };
        let proof_b64 = self
            .zk_proofs
            .as_ref()
            .map(|z| z.proof.clone())
            .unwrap_or_default();
        let public_b64 = self
            .zk_proofs
            .as_ref()
            .map(|z| z.public_input.clone())
            .unwrap_or_default();
        let publickey = self
            .ephemeral
            .as_ref()
            .map(|e| e.public_key.clone())
            .unwrap_or_default();
        cx.spawn(async move |_this, _cx| {
            clients
                .dong
                .claim_amount_red_envelope_qr(ClaimRedEnvelopeQrRequest {
                    id,
                    user_id,
                    proof_b64,
                    public_b64,
                    publickey,
                })
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn on_token_sent(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::TokenSent(token) = event else {
            return;
        };
        let Some(my_id) = self.auth_user.clone() else {
            return;
        };
        let amount = token.amount as i64;
        if amount == 0 {
            return;
        }
        let sender_id = token.sender_id.to_string();
        let receiver_id = token.receiver_id.to_string();
        let Ok(scaled) = scale_amount_to_decimals(&amount.to_string(), DECIMALS) else {
            return;
        };

        match token_direction(&my_id, &sender_id, &receiver_id) {
            TokenDirection::Received => {
                self.apply_balance_delta(&scaled, true, cx);
                cx.emit(WalletEvent::TokenReceived { amount, sender_id });
                self.play_bank_sound(cx);
                cx.notify();
            }
            TokenDirection::Sent => {
                self.apply_balance_delta(&scaled, false, cx);
                cx.notify();
            }
            TokenDirection::Unrelated => {}
        }
    }

    fn play_bank_sound(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.bank_player {
            player.play();
            return;
        }
        if self.bank_sound_loading {
            return;
        }
        self.bank_sound_loading = true;
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { decode_audio(BANK_SOUND.to_vec()) })
                .await;
            this.update(cx, |this, _| {
                this.bank_sound_loading = false;
                let Ok(pcm) = decoded else {
                    return;
                };
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                player.play();
                this.bank_player = Some(player);
            })
            .ok();
        })
        .detach();
    }

    fn apply_balance_delta(&mut self, scaled: &str, add: bool, cx: &mut Context<Self>) {
        let updated = self
            .wallet
            .as_ref()
            .and_then(|wallet| balance_after_delta(&wallet.balance, scaled, add));
        match (updated, self.wallet.as_mut()) {
            (Some(next), Some(wallet)) => wallet.balance = next,
            _ => {
                self.balance_freshness.mark_stale();
                if let Some(user_id) = self.auth_user.clone() {
                    self.refresh_wallet(user_id, cx);
                }
            }
        }
    }

    pub fn reset_generation(&self) -> u64 {
        self.reset_generation
    }

    fn persist_wallet_state(&self) {
        let Some(user_id) = self.enabled_user.clone() else {
            return;
        };
        if !self.is_enabled {
            return;
        }
        let state = PersistedWalletState {
            user_id,
            is_enabled: self.is_enabled,
            wallet: self.wallet.clone(),
            zk_proofs: self.zk_proofs.clone(),
            ephemeral: self.ephemeral.clone(),
            proof_minted_at: self.proof_minted_at,
        };
        if let Err(error) = wallet_persist::save_wallet(&state) {
            tracing::warn!(%error, "wallet: failed to persist wallet state");
        }
    }

    fn clear_persisted_wallet() {
        if let Err(error) = wallet_persist::clear_wallet() {
            tracing::warn!(%error, "wallet: failed to clear persisted wallet state");
        }
    }

    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        if !self.is_enabled
            && self.wallet.is_none()
            && self.zk_proofs.is_none()
            && self.ephemeral.is_none()
            && self.enable_task.is_none()
            && self.enabling_user.is_none()
        {
            return;
        }
        self.reset_generation += 1;
        self.enable_generation = self.enable_generation.wrapping_add(1);
        self.enable_task = None;
        self.enabling_user = None;
        self.wallet = None;
        self.zk_proofs = None;
        self.ephemeral = None;
        self.proof_minted_at = None;
        self.failed_id_token_exp = None;
        self.is_enabled = false;
        self.pending_give_coffee = false;
        self.pending_give_flower = false;
        self.enabled_user = None;
        self.auth_user = None;
        self.balance_user = None;
        self.balance_refreshing = false;
        self.balance_freshness.mark_stale();
        Self::clear_persisted_wallet();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenDirection, balance_after_delta, should_refresh_balance, token_direction};

    #[test]
    fn a_transfer_is_classified_from_the_signed_in_user() {
        assert_eq!(token_direction("u1", "u2", "u1"), TokenDirection::Received);
        assert_eq!(token_direction("u1", "u1", "u2"), TokenDirection::Sent);
        assert_eq!(token_direction("u1", "u2", "u3"), TokenDirection::Unrelated);
    }

    #[test]
    fn a_self_transfer_counts_as_received() {
        assert_eq!(token_direction("u1", "u1", "u1"), TokenDirection::Received);
    }

    #[test]
    fn an_unknown_user_ignores_every_transfer() {
        assert_eq!(token_direction("", "u1", "u2"), TokenDirection::Unrelated);
        assert_eq!(token_direction("", "u1", ""), TokenDirection::Unrelated);
    }

    #[test]
    fn a_delta_moves_the_balance_in_both_directions() {
        assert_eq!(
            balance_after_delta("1000", "250", true).as_deref(),
            Some("1250")
        );
        assert_eq!(
            balance_after_delta("1000", "250", false).as_deref(),
            Some("750")
        );
    }

    #[test]
    fn a_balance_never_goes_negative() {
        assert_eq!(
            balance_after_delta("100", "250", false).as_deref(),
            Some("0")
        );
    }

    #[test]
    fn an_unparsable_balance_yields_no_update() {
        assert_eq!(balance_after_delta("", "250", true), None);
        assert_eq!(balance_after_delta("abc", "250", true), None);
        assert_eq!(balance_after_delta("1000", "", true), None);
    }

    #[test]
    fn an_in_flight_refresh_blocks_every_caller() {
        assert!(!should_refresh_balance(true, false, false, false));
        assert!(!should_refresh_balance(true, true, false, false));
    }

    #[test]
    fn a_forced_refresh_ignores_freshness() {
        assert!(should_refresh_balance(false, true, true, true));
    }

    #[test]
    fn an_unforced_refresh_is_skipped_only_when_fresh_for_the_same_user() {
        assert!(!should_refresh_balance(false, false, true, true));
        assert!(should_refresh_balance(false, false, true, false));
        assert!(should_refresh_balance(false, false, false, true));
    }
}
