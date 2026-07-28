use std::sync::Arc;
use std::time::Duration;

use http_client::{HttpClient, http::Method};
use serde::Deserialize;

use crate::error::Result;
use crate::http::{self, Headers};
use crate::types::{GetZkProofRequest, ZkProof};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct ZkProofEnvelope {
    data: ZkProof,
}

pub struct ZkClient {
    http: Arc<dyn HttpClient>,
    endpoint: String,
    timeout: Duration,
    headers: Headers,
}

impl ZkClient {
    pub fn new(http: Arc<dyn HttpClient>, endpoint: impl Into<String>) -> Self {
        let mut headers = Headers::new();
        headers.insert("Accept".to_string(), "application/json".to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            http,
            endpoint: endpoint.into(),
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

    pub async fn get_zk_proofs(&self, request: GetZkProofRequest) -> Result<ZkProof> {
        let url = format!("{}/prove", self.endpoint);
        let body = serde_json::to_vec(&serde_json::json!({
            "user_id": request.user_id,
            "ephemeral_pk": request.ephemeral_public_key,
            "jwt": request.jwt,
            "address": request.address,
            "client_type": request.client_type.as_str(),
        }))?;

        let bytes = http::execute(
            self.http.clone(),
            Method::POST,
            url,
            self.headers.clone(),
            Some(body),
            self.timeout,
        )
        .await?;

        let envelope: ZkProofEnvelope = http::deserialize(&bytes)?;
        Ok(envelope.data)
    }
}
