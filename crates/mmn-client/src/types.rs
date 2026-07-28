use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize as _;

pub const TRANSFER_TYPE_GIVE_COFFEE: &str = "give_coffee";
pub const TRANSFER_TYPE_TRANSFER_TOKEN: &str = "transfer_token";
pub const TRANSFER_TYPE_UNLOCK_ITEM: &str = "unlock_item";

pub const TX_TYPE_TRANSFER_BY_ZK: u8 = 0;
pub const TX_TYPE_TRANSFER_BY_KEY: u8 = 1;
pub const TX_TYPE_USER_CONTENT: u8 = 2;

pub const DECIMALS: u32 = 6;
pub const DECIMAL_FACTOR: i128 = 10i128.pow(DECIMALS);

const REDACTED: &str = "<redacted>";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EphemeralKeyPair {
    pub private_key: String,
    pub public_key: String,
}

impl fmt::Debug for EphemeralKeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EphemeralKeyPair")
            .field("private_key", &REDACTED)
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl Drop for EphemeralKeyPair {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtraInfo {
    #[serde(rename = "type")]
    pub transfer_type: String,

    #[serde(rename = "ItemId", skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,

    #[serde(rename = "ItemType", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,

    #[serde(rename = "ChannelId", skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,

    #[serde(rename = "ClanId", skip_serializing_if = "Option::is_none")]
    pub clan_id: Option<String>,

    #[serde(rename = "MessageRefId", skip_serializing_if = "Option::is_none")]
    pub message_ref_id: Option<String>,

    #[serde(rename = "UserReceiverId", skip_serializing_if = "Option::is_none")]
    pub user_receiver_id: Option<String>,

    #[serde(rename = "UserSenderId", skip_serializing_if = "Option::is_none")]
    pub user_sender_id: Option<String>,

    #[serde(rename = "UserSenderUsername", skip_serializing_if = "Option::is_none")]
    pub user_sender_username: Option<String>,

    #[serde(rename = "ExtraAttribute", skip_serializing_if = "Option::is_none")]
    pub extra_attribute: Option<String>,

    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ExtraInfo {
    pub fn new(transfer_type: impl Into<String>) -> Self {
        Self {
            transfer_type: transfer_type.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxMsg {
    #[serde(rename = "type")]
    pub tx_type: u8,
    pub sender: String,
    pub recipient: String,
    pub amount: String,
    pub timestamp: i64,
    pub text_data: String,
    pub nonce: i64,
    pub extra_info: String,
    pub zk_proof: String,
    pub zk_pub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTx {
    pub tx_msg: TxMsg,
    pub signature: String,
}

#[derive(Clone, Default)]
pub struct SendTransactionBase {
    pub sender: String,
    pub recipient: String,
    pub amount: String,
    pub nonce: i64,
    pub timestamp: Option<i64>,
    pub text_data: Option<String>,
    pub private_key: String,
    pub extra_info: Option<ExtraInfo>,
}

impl fmt::Debug for SendTransactionBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendTransactionBase")
            .field("sender", &self.sender)
            .field("recipient", &self.recipient)
            .field("amount", &self.amount)
            .field("nonce", &self.nonce)
            .field("timestamp", &self.timestamp)
            .field("text_data", &self.text_data)
            .field("private_key", &REDACTED)
            .field("extra_info", &self.extra_info)
            .finish()
    }
}

#[derive(Clone, Default)]
pub struct SendTransactionRequest {
    pub sender: String,
    pub recipient: String,
    pub amount: String,
    pub nonce: i64,
    pub timestamp: Option<i64>,
    pub text_data: Option<String>,
    pub private_key: String,
    pub extra_info: Option<ExtraInfo>,
    pub zk_proof: String,
    pub zk_pub: String,
    pub public_key: String,
}

impl fmt::Debug for SendTransactionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendTransactionRequest")
            .field("sender", &self.sender)
            .field("recipient", &self.recipient)
            .field("amount", &self.amount)
            .field("nonce", &self.nonce)
            .field("timestamp", &self.timestamp)
            .field("text_data", &self.text_data)
            .field("private_key", &REDACTED)
            .field("extra_info", &self.extra_info)
            .field("zk_proof", &self.zk_proof)
            .field("zk_pub", &self.zk_pub)
            .field("public_key", &self.public_key)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddTxResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub tx_hash: String,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetCurrentNonceResponse {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub nonce: i64,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetAccountByAddressResponse {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub balance: String,
    #[serde(default)]
    pub nonce: i64,
    #[serde(default)]
    pub decimals: u32,
}

#[derive(Debug, Clone)]
pub struct NonceTag(pub &'static str);

impl NonceTag {
    pub const LATEST: NonceTag = NonceTag("latest");
    pub const PENDING: NonceTag = NonceTag("pending");
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transaction {
    #[serde(default)]
    pub chain_id: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub nonce: i64,
    #[serde(default)]
    pub block_hash: String,
    #[serde(default)]
    pub block_number: i64,
    #[serde(default)]
    pub block_timestamp: i64,
    #[serde(default)]
    pub transaction_index: i64,
    #[serde(default)]
    pub from_address: String,
    #[serde(default)]
    pub to_address: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub gas: i64,
    #[serde(default)]
    pub gas_price: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub function_selector: String,
    #[serde(default)]
    pub transaction_type: i64,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(default)]
    pub transaction_timestamp: i64,
    #[serde(default)]
    pub text_data: String,
    #[serde(default)]
    pub extra_info: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub chain_id: i64,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub total_items: Option<i64>,
    #[serde(default)]
    pub total_pages: Option<i64>,
    #[serde(default)]
    pub has_more: Option<bool>,
    #[serde(default)]
    pub next_timestamp: Option<String>,
    #[serde(default)]
    pub next_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletDetail {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub balance: String,
    #[serde(default)]
    pub account_nonce: i64,
    #[serde(default)]
    pub last_balance_update: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletDetailResponse {
    pub data: WalletDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListTransactionResponse {
    pub meta: Meta,
    #[serde(default)]
    pub data: Option<Vec<Transaction>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransactionDetailInner {
    pub transaction: Transaction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransactionDetailResponse {
    pub data: TransactionDetailInner,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ZkClientType {
    #[default]
    Mezon,
    Oauth,
}

impl ZkClientType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ZkClientType::Mezon => "mezon",
            ZkClientType::Oauth => "oauth",
        }
    }
}

#[derive(Clone)]
pub struct GetZkProofRequest {
    pub user_id: String,
    pub ephemeral_public_key: String,
    pub jwt: String,
    pub address: String,
    pub client_type: ZkClientType,
}

impl fmt::Debug for GetZkProofRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GetZkProofRequest")
            .field("user_id", &self.user_id)
            .field("ephemeral_public_key", &self.ephemeral_public_key)
            .field("jwt", &REDACTED)
            .field("address", &self.address)
            .field("client_type", &self.client_type)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZkProof {
    #[serde(default)]
    pub proof: String,
    #[serde(default)]
    pub public_input: String,
}

#[derive(Debug, Clone)]
pub struct ClaimRedEnvelopeQrRequest {
    pub id: String,
    pub user_id: String,
    pub proof_b64: String,
    pub public_b64: String,
    pub publickey: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaimRedEnvelopeQrResponse {
    #[serde(default)]
    pub split_money_id: i64,
    #[serde(default)]
    pub amount: i64,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ExecuteClaimRedEnvelopeQrRequest {
    pub split_money_id: i64,
    pub user_id: String,
    pub proof_b64: String,
    pub public_b64: String,
    pub publickey: String,
}
