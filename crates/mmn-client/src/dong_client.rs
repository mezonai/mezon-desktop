use std::sync::Arc;
use std::time::Duration;

use http_client::{HttpClient, http::Method};
use serde::Deserialize;

use crate::error::Result;
use crate::http::{self, Headers};
use crate::types::{
    ClaimRedEnvelopeQrRequest, ClaimRedEnvelopeQrResponse, ExecuteClaimRedEnvelopeQrRequest,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct DataEnvelope<R> {
    data: R,
}

pub struct DongClient {
    http: Arc<dyn HttpClient>,
    endpoint: String,
    timeout: Duration,
    headers: Headers,
}

impl DongClient {
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

    pub async fn claim_amount_red_envelope_qr(
        &self,
        params: ClaimRedEnvelopeQrRequest,
    ) -> Result<ClaimRedEnvelopeQrResponse> {
        let base = format!("{}/api/v1/red-envelopes/qr/claim-amount", self.endpoint);
        let url = http::build_query(&base, &[("id", params.id.clone())]);
        let body = serde_json::to_vec(&serde_json::json!({
            "user_id": params.user_id,
            "proof_b64": params.proof_b64,
            "public_b64": params.public_b64,
            "publickey": params.publickey,
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

        let envelope: DataEnvelope<ClaimRedEnvelopeQrResponse> = http::deserialize(&bytes)?;
        Ok(envelope.data)
    }

    pub async fn claim_red_envelope_qr(
        &self,
        id: &str,
        params: ExecuteClaimRedEnvelopeQrRequest,
    ) -> Result<()> {
        let url = format!("{}/api/v1/red-envelopes/qr/{}/claim", self.endpoint, id);
        let body = serde_json::to_vec(&serde_json::json!({
            "split_money_id": params.split_money_id,
            "user_id": params.user_id,
            "proof_b64": params.proof_b64,
            "public_b64": params.public_b64,
            "publickey": params.publickey,
        }))?;

        http::execute(
            self.http.clone(),
            Method::POST,
            url,
            self.headers.clone(),
            Some(body),
            self.timeout,
        )
        .await?;

        Ok(())
    }
}
