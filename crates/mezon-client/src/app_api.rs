use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{ApiAccount, ApiChannelDesc, ApiClanDesc},
};

#[derive(Clone)]
pub struct AppApi {
    transport: Arc<TransportClient>,
}

impl AppApi {
    pub fn new(transport: Arc<TransportClient>) -> Self {
        Self { transport }
    }

    pub async fn get_account(&self) -> Result<ApiAccount> {
        self.transport.get_account().await
    }

    pub async fn list_channel_descs(&self, clan_id: &str) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_descs(clan_id).await
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        self.transport.list_clan_descs().await
    }

    pub async fn is_open(&self) -> bool {
        self.transport.is_open().await
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        self.transport.ping_roundtrip().await
    }
}
