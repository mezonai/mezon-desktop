use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_client::{HttpClient, http::Method};
use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::error::{MmnError, Result};
use crate::http::{self, Headers};
use crate::types::{
    AddTxResponse, DECIMALS, EphemeralKeyPair, ExtraInfo, GetAccountByAddressResponse,
    GetCurrentNonceResponse, SendTransactionBase, SendTransactionRequest, SignedTx,
    TX_TYPE_TRANSFER_BY_KEY, TX_TYPE_TRANSFER_BY_ZK, TX_TYPE_USER_CONTENT, TxMsg,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize)]
struct JsonRpcRequest<'a, P: Serialize> {
    jsonrpc: &'a str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<P>,
    id: u64,
}

#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

struct TxParams<'a> {
    tx_type: u8,
    sender: &'a str,
    recipient: &'a str,
    amount: &'a str,
    timestamp: Option<i64>,
    text_data: Option<&'a str>,
    nonce: i64,
    extra_info: Option<&'a ExtraInfo>,
    private_key: &'a str,
    zk_proof: &'a str,
    zk_pub: &'a str,
}

pub struct MmnClient {
    http: Arc<dyn HttpClient>,
    base_url: String,
    timeout: Duration,
    headers: Headers,
    request_id: AtomicU64,
}

impl MmnClient {
    pub fn new(http: Arc<dyn HttpClient>, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            timeout: DEFAULT_TIMEOUT,
            headers: default_json_headers(),
            request_id: AtomicU64::new(0),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    async fn make_request<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: serde::de::DeserializeOwned,
    {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params: Some(params),
            id,
        };
        let body = serde_json::to_vec(&request)?;
        let bytes = http::execute(
            self.http.clone(),
            Method::POST,
            self.base_url.clone(),
            self.headers.clone(),
            Some(body),
            self.timeout,
        )
        .await?;

        let response: JsonRpcResponse<R> = http::deserialize(&bytes)?;
        if let Some(error) = response.error {
            return Err(MmnError::JsonRpc {
                code: error.code,
                message: error.message,
            });
        }
        response.result.ok_or(MmnError::EmptyResult)
    }

    pub fn generate_ephemeral_key_pair(&self) -> Result<EphemeralKeyPair> {
        crypto::generate_ephemeral_key_pair()
    }

    pub fn get_address_from_user_id(&self, user_id: &str) -> String {
        crypto::address_from_user_id(user_id)
    }

    pub fn validate_address(&self, addr: &str) -> bool {
        crypto::validate_address(addr)
    }

    pub fn scale_amount_to_decimals(&self, amount: &str) -> Result<String> {
        crypto::scale_amount_to_decimals(amount, DECIMALS)
    }

    pub fn scale_amount_with_decimals(&self, amount: &str, decimals: u32) -> Result<String> {
        crypto::scale_amount_to_decimals(amount, decimals)
    }

    pub fn validate_amount(&self, balance: &str, amount_scaled: &str) -> bool {
        crypto::validate_amount(balance, amount_scaled)
    }

    fn create_and_sign_tx(&self, params: TxParams<'_>) -> Result<SignedTx> {
        if !crypto::validate_address(params.sender) {
            return Err(MmnError::Invalid("Invalid sender address".into()));
        }
        if !crypto::validate_address(params.recipient) {
            return Err(MmnError::Invalid("Invalid recipient address".into()));
        }
        if params.sender == params.recipient {
            return Err(MmnError::Invalid(
                "Sender and recipient addresses cannot be the same".into(),
            ));
        }

        let extra_info = match params.extra_info {
            Some(info) => serde_json::to_string(info)?,
            None => String::new(),
        };

        let tx_msg = TxMsg {
            tx_type: params.tx_type,
            sender: params.sender.to_string(),
            recipient: params.recipient.to_string(),
            amount: params.amount.to_string(),
            timestamp: params.timestamp.unwrap_or_else(now_millis),
            text_data: params.text_data.unwrap_or("").to_string(),
            nonce: params.nonce,
            extra_info,
            zk_proof: params.zk_proof.to_string(),
            zk_pub: params.zk_pub.to_string(),
        };

        let signature = crypto::sign_transaction(&tx_msg, params.private_key)?;
        Ok(SignedTx { tx_msg, signature })
    }

    async fn add_tx(&self, signed_tx: SignedTx) -> Result<AddTxResponse> {
        self.make_request("tx.addtx", signed_tx).await
    }

    pub async fn send_transaction(&self, params: SendTransactionRequest) -> Result<AddTxResponse> {
        let from_address = crypto::address_from_user_id(&params.sender);
        let to_address = crypto::address_from_user_id(&params.recipient);
        let signed_tx = self.create_and_sign_tx(TxParams {
            tx_type: TX_TYPE_TRANSFER_BY_ZK,
            sender: &from_address,
            recipient: &to_address,
            amount: &params.amount,
            timestamp: params.timestamp,
            text_data: params.text_data.as_deref(),
            nonce: params.nonce,
            extra_info: params.extra_info.as_ref(),
            private_key: &params.private_key,
            zk_proof: &params.zk_proof,
            zk_pub: &params.zk_pub,
        })?;
        self.add_tx(signed_tx).await
    }

    pub async fn send_transaction_by_address(
        &self,
        params: SendTransactionRequest,
    ) -> Result<AddTxResponse> {
        let signed_tx = self.create_and_sign_tx(TxParams {
            tx_type: TX_TYPE_TRANSFER_BY_ZK,
            sender: &params.sender,
            recipient: &params.recipient,
            amount: &params.amount,
            timestamp: params.timestamp,
            text_data: params.text_data.as_deref(),
            nonce: params.nonce,
            extra_info: params.extra_info.as_ref(),
            private_key: &params.private_key,
            zk_proof: &params.zk_proof,
            zk_pub: &params.zk_pub,
        })?;
        self.add_tx(signed_tx).await
    }

    pub async fn send_transaction_by_private_key(
        &self,
        params: SendTransactionBase,
    ) -> Result<AddTxResponse> {
        let signed_tx = self.create_and_sign_tx(TxParams {
            tx_type: TX_TYPE_TRANSFER_BY_KEY,
            sender: &params.sender,
            recipient: &params.recipient,
            amount: &params.amount,
            timestamp: params.timestamp,
            text_data: params.text_data.as_deref(),
            nonce: params.nonce,
            extra_info: params.extra_info.as_ref(),
            private_key: &params.private_key,
            zk_proof: "",
            zk_pub: "",
        })?;
        self.add_tx(signed_tx).await
    }

    pub async fn post_donation_campaign_feed(
        &self,
        params: SendTransactionRequest,
    ) -> Result<AddTxResponse> {
        let signed_tx = self.create_and_sign_tx(TxParams {
            tx_type: TX_TYPE_USER_CONTENT,
            sender: &params.sender,
            recipient: &params.recipient,
            amount: &params.amount,
            timestamp: params.timestamp,
            text_data: params.text_data.as_deref(),
            nonce: params.nonce,
            extra_info: params.extra_info.as_ref(),
            private_key: &params.private_key,
            zk_proof: &params.zk_proof,
            zk_pub: &params.zk_pub,
        })?;
        self.add_tx(signed_tx).await
    }

    pub async fn get_current_nonce(
        &self,
        user_id: &str,
        tag: &str,
    ) -> Result<GetCurrentNonceResponse> {
        let address = crypto::address_from_user_id(user_id);
        self.make_request(
            "account.getcurrentnonce",
            serde_json::json!({ "address": address, "tag": tag }),
        )
        .await
    }

    pub async fn get_current_nonce_by_address(
        &self,
        address: &str,
        tag: &str,
    ) -> Result<GetCurrentNonceResponse> {
        self.make_request(
            "account.getcurrentnonce",
            serde_json::json!({ "address": address, "tag": tag }),
        )
        .await
    }

    pub async fn get_account_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<GetAccountByAddressResponse> {
        let address = crypto::address_from_user_id(user_id);
        self.make_request(
            "account.getaccount",
            serde_json::json!({ "address": address }),
        )
        .await
    }
}

pub(crate) fn default_json_headers() -> Headers {
    let mut headers = Headers::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
