use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_audio::{AudioPlayer, decode_audio};
use mezon_client::RealtimeEvent;
use mezon_client::transport_runtime::http_client_arc;
use mmn_client::{
    AddTxResponse, ClaimRedEnvelopeQrRequest, ClaimRedEnvelopeQrResponse, DECIMALS, DongClient,
    EphemeralKeyPair, ExtraInfo, GetZkProofRequest, IndexerClient, MmnClient,
    SendTransactionRequest, ZkClient, ZkClientType, ZkProof, address_from_user_id,
    generate_ephemeral_key_pair, is_secure_endpoint, scale_amount_to_decimals,
};

use mezon_client::Session;

use crate::AuthState;
use crate::config::{AppConfig, INDEXER_CHAIN_ID};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::wallet_persist::{self, PersistedWalletState};

const GIVE_COFFEE_AMOUNT: i64 = 10_000;

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
}

#[derive(Debug, Clone)]
pub enum WalletEvent {
    Enabled,
    BalanceChanged,
    TransactionSent { tx_hash: String },
    TokenReceived { amount: i64, sender_id: String },
    CoffeeSent,
    SendFailed { message: String },
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
    indexer: IndexerClient,
    zk: ZkClient,
    dong: DongClient,
}

pub struct WalletStore {
    auth_state: Entity<AuthState>,
    clients: Option<Arc<WalletClients>>,
    wallet: Option<WalletDetail>,
    zk_proofs: Option<ZkProof>,
    ephemeral: Option<EphemeralKeyPair>,
    is_enabled: bool,
    pending_give_coffee: bool,
    enabled_user: Option<String>,
    enabling_user: Option<String>,
    reset_generation: u64,
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
                is_enabled: false,
                pending_give_coffee: false,
                enabled_user: None,
                enabling_user: None,
                reset_generation: 0,
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

    pub fn give_coffee_amount() -> i64 {
        GIVE_COFFEE_AMOUNT
    }

    fn build_clients(cx: &App) -> Option<Arc<WalletClients>> {
        let config = AppConfig::try_global(cx)?;
        if config.mmn_api_url.is_empty() || config.zk_api_url.is_empty() {
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
        let http = http_client_arc();
        Some(Arc::new(WalletClients {
            mmn: MmnClient::new(http.clone(), config.mmn_api_url.clone()),
            indexer: IndexerClient::new(
                http.clone(),
                config.indexer_api_url.clone(),
                INDEXER_CHAIN_ID,
            ),
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
        self.try_restore_persisted(&user_id, cx);
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
        self.is_enabled = true;
        self.enabled_user = Some(user_id.to_string());
        cx.notify();
    }

    pub fn fetch_zk_proofs_after_login(&mut self, session: &Session, cx: &mut Context<Self>) {
        if session.id_token.is_empty() || session.user_id.is_empty() {
            return;
        }
        self.enable_wallet(session.id_token.clone(), session.user_id.clone(), true, cx);
    }

    pub fn enable_wallet_for_current_user(&mut self, cx: &mut Context<Self>) {
        let Some((jwt, user_id)) = self
            .auth_state
            .read(cx)
            .session_credentials()
            .filter(|(jwt, uid)| !jwt.is_empty() && !uid.is_empty())
        else {
            return;
        };
        self.enable_wallet(jwt, user_id, false, cx);
    }

    fn enable_wallet(&mut self, jwt: String, user_id: String, force: bool, cx: &mut Context<Self>) {
        if jwt.is_empty() || user_id.is_empty() {
            return;
        }
        if !force && self.is_enabled && self.enabled_user.as_deref() == Some(user_id.as_str()) {
            return;
        }
        if self.enabling_user.as_deref() == Some(user_id.as_str()) {
            return;
        }
        let Some(clients) = self.clients.clone() else {
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
        self.enabling_user = Some(user_id.clone());
        self.enable_task = Some(cx.spawn(async move |this, cx| {
            let ephemeral = generate_ephemeral_key_pair().ok();
            let (zk_proofs, account) = match &ephemeral {
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
                            let account = clients.mmn.get_account_by_user_id(&user_id).await.ok();
                            (Some(proofs), account)
                        }
                        Err(error) => {
                            tracing::warn!(%error, "wallet: zk proof fetch failed");
                            (None, None)
                        }
                    }
                }
                None => (None, None),
            };
            this.update(cx, |this, cx| {
                if this.enabling_user.as_deref() == Some(user_id.as_str()) {
                    this.enabling_user = None;
                }
                if this.reset_generation != generation {
                    return;
                }
                let (Some(ephemeral), Some(zk_proofs)) = (ephemeral, zk_proofs) else {
                    return;
                };
                this.ephemeral = Some(ephemeral);
                this.zk_proofs = Some(zk_proofs);
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
        let Some(clients) = self.clients.clone() else {
            return;
        };
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let account = clients.mmn.get_account_by_user_id(&user_id).await;
            this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                match account {
                    Ok(account) => {
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

            let response = result.map_err(|error| error.to_string())?;
            if !response.ok {
                let message = if response.error.is_empty() {
                    "Transaction failed".to_string()
                } else {
                    response.error.clone()
                };
                return Err(message);
            }

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

    pub fn list_wallet_transactions(
        &self,
        address: String,
        page: i64,
        limit: i64,
        filter: i32,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<WalletTransaction>, String>> {
        let Some(clients) = self.clients.clone() else {
            return Task::ready(Err("Wallet is not configured".to_string()));
        };
        cx.spawn(async move |_this, _cx| {
            let response = clients
                .indexer
                .get_transaction_by_wallet(&address, page, limit, filter, None, None)
                .await
                .map_err(|error| error.to_string())?;
            let rows = response
                .data
                .unwrap_or_default()
                .into_iter()
                .map(|tx| {
                    let sent = tx.from_address == address;
                    let counterparty = if sent { tx.to_address } else { tx.from_address };
                    WalletTransaction {
                        sent,
                        value: tx.value,
                        counterparty,
                        note: tx.text_data,
                        hash: tx.hash,
                    }
                })
                .collect();
            Ok(rows)
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
        let Some(my_id) = self.enabled_user.clone() else {
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

        if receiver_id == my_id {
            self.apply_balance_delta(&scaled, true);
            cx.emit(WalletEvent::TokenReceived { amount, sender_id });
            self.play_bank_sound(cx);
            cx.notify();
        } else if sender_id == my_id {
            self.apply_balance_delta(&scaled, false);
            cx.notify();
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

    fn apply_balance_delta(&mut self, scaled: &str, add: bool) {
        let Some(wallet) = self.wallet.as_mut() else {
            return;
        };
        let Ok(current) = wallet.balance.trim().parse::<i128>() else {
            return;
        };
        let delta: i128 = scaled.trim().parse().unwrap_or(0);
        let next = if add {
            current.saturating_add(delta)
        } else {
            current.saturating_sub(delta)
        };
        wallet.balance = next.max(0).to_string();
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
        self.enable_task = None;
        self.enabling_user = None;
        self.wallet = None;
        self.zk_proofs = None;
        self.ephemeral = None;
        self.is_enabled = false;
        self.pending_give_coffee = false;
        self.enabled_user = None;
        Self::clear_persisted_wallet();
        cx.notify();
    }
}
