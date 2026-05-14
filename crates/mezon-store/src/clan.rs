use mezon_client::transport::ApiClanDesc;

#[derive(Debug, Clone)]
pub struct Clan {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub unread_count: u32,
}

impl From<ApiClanDesc> for Clan {
    fn from(c: ApiClanDesc) -> Self {
        Self {
            id: c.clan_id,
            name: c.clan_name,
            avatar_url: None,
            unread_count: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClanList {
    pub clans: Vec<Clan>,
    pub active_clan_id: Option<String>,
}

impl ClanList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_clan(&self) -> Option<&Clan> {
        self.active_clan_id
            .as_ref()
            .and_then(|id| self.clans.iter().find(|c| &c.id == id))
    }

    pub fn is_active_clan(&self, clan_id: &str) -> bool {
        self.active_clan_id.as_deref() == Some(clan_id)
    }

    pub fn select_clan(&mut self, id: &str) {
        self.active_clan_id = Some(id.to_string());
    }

    pub fn update_clans(&mut self, clans: Vec<Clan>) {
        self.clans = clans;
        if !self.clans.is_empty() {
            if let Some(active_id) = &self.active_clan_id {
                if !self.clans.iter().any(|c| &c.id == active_id) {
                    self.active_clan_id = Some(self.clans[0].id.clone());
                }
            } else {
                self.active_clan_id = Some(self.clans[0].id.clone());
            }
        }
    }
}
