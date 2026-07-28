use std::sync::Arc;
use std::time::Duration;

use http_client::{HttpClient, http::Method};

use crate::error::{MmnError, Result};
use crate::http::{self, Headers};
use crate::types::{
    ListTransactionResponse, Transaction, TransactionDetailResponse, WalletDetail,
    WalletDetailResponse,
};

pub const FILTER_ALL: i32 = 0;
pub const FILTER_RECEIVED: i32 = 1;
pub const FILTER_SENT: i32 = 2;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 1000;
const DEFAULT_PAGE_LIMIT: i64 = 50;

pub struct IndexerClient {
    http: Arc<dyn HttpClient>,
    endpoint: String,
    chain_id: String,
    timeout: Duration,
    headers: Headers,
}

impl IndexerClient {
    pub fn new(
        http: Arc<dyn HttpClient>,
        endpoint: impl Into<String>,
        chain_id: impl Into<String>,
    ) -> Self {
        let mut headers = Headers::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        Self {
            http,
            endpoint: endpoint.into(),
            chain_id: chain_id.into(),
            timeout: DEFAULT_TIMEOUT,
            headers,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers.extend(headers);
        self
    }

    async fn get_json<R>(&self, path: &str, params: &[(&str, String)]) -> Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let url = http::build_query(&format!("{}/{}", self.endpoint, path), params);
        let bytes = http::execute(
            self.http.clone(),
            Method::GET,
            url,
            self.headers.clone(),
            None,
            self.timeout,
        )
        .await?;
        http::deserialize(&bytes)
    }

    pub async fn get_transaction_by_hash(&self, hash: &str) -> Result<Transaction> {
        let path = format!("{}/tx/{}/detail", self.chain_id, hash);
        let response: TransactionDetailResponse = self.get_json(&path, &[]).await?;
        Ok(response.data.transaction)
    }

    pub async fn get_transactions_by_wallet_before_timestamp(
        &self,
        wallet: &str,
        filter: i32,
        limit: Option<i64>,
        timestamp_lt: Option<&str>,
        last_hash: Option<&str>,
    ) -> Result<ListTransactionResponse> {
        if wallet.is_empty() {
            return Err(MmnError::Invalid("wallet address cannot be empty".into()));
        }

        let mut final_limit = limit.filter(|value| *value > 0).unwrap_or(DEFAULT_LIMIT);
        if final_limit > MAX_LIMIT {
            final_limit = MAX_LIMIT;
        }

        let mut params: Vec<(&str, String)> = vec![("limit", final_limit.to_string())];
        if let Some(value) = timestamp_lt {
            params.push(("timestamp_lt", value.to_string()));
        }
        if let Some(value) = last_hash {
            params.push(("last_hash", value.to_string()));
        }
        if let Some(entry) = filter_param(filter, wallet) {
            params.push(entry);
        }

        let path = format!("{}/transactions/infinite", self.chain_id);
        self.get_json(&path, &params).await
    }

    pub async fn get_transaction_by_wallet(
        &self,
        wallet: &str,
        page: i64,
        limit: i64,
        filter: i32,
        sort_by: Option<&str>,
        sort_order: Option<&str>,
    ) -> Result<ListTransactionResponse> {
        if wallet.is_empty() {
            return Err(MmnError::Invalid("wallet address cannot be empty".into()));
        }

        let page = if page < 1 { 1 } else { page };
        let mut limit = if limit <= 0 {
            DEFAULT_PAGE_LIMIT
        } else {
            limit
        };
        if limit > MAX_LIMIT {
            limit = MAX_LIMIT;
        }

        let mut params: Vec<(&str, String)> = vec![
            ("page", (page - 1).to_string()),
            ("limit", limit.to_string()),
            (
                "sort_by",
                sort_by.unwrap_or("transaction_timestamp").to_string(),
            ),
            ("sort_order", sort_order.unwrap_or("desc").to_string()),
        ];
        if let Some(entry) = filter_param(filter, wallet) {
            params.push(entry);
        }

        let path = format!("{}/transactions", self.chain_id);
        self.get_json(&path, &params).await
    }

    pub async fn get_wallet_detail(&self, wallet: &str) -> Result<WalletDetail> {
        if wallet.is_empty() {
            return Err(MmnError::Invalid("wallet address cannot be empty".into()));
        }
        let path = format!("{}/wallets/{}/detail", self.chain_id, wallet);
        let response: WalletDetailResponse = self.get_json(&path, &[]).await?;
        Ok(response.data)
    }
}

fn filter_param(filter: i32, wallet: &str) -> Option<(&'static str, String)> {
    match filter {
        FILTER_ALL => Some(("wallet_address", wallet.to_string())),
        FILTER_SENT => Some(("filter_from_address", wallet.to_string())),
        FILTER_RECEIVED => Some(("filter_to_address", wallet.to_string())),
        _ => None,
    }
}
