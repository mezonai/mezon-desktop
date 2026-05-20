use mezon_client::transport::ApiChannelDesc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Text,
    Voice,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub unread: bool,
    pub private: bool,
    pub clan_id: String,
    pub category_name: String,
    pub category_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub content: String,
    pub sender_id: String,
    pub sender_name: String,
    pub create_time: i64,
}

#[derive(Debug, Clone)]
pub struct Category {
    pub clan_id: String,
    pub name: String,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelList {
    pub categories: Vec<Category>,
    pub active_channel_id: Option<String>,
}

impl ChannelList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_channel(&self) -> Option<&Channel> {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.find_channel(id))
    }

    pub fn categories_for_clan(&self, clan_id: &str) -> Vec<&Category> {
        self.categories
            .iter()
            .filter(|c| c.clan_id == clan_id)
            .collect()
    }

    pub fn select_channel(&mut self, id: &str) {
        self.active_channel_id = Some(id.to_string());
        self.mark_read(id);
    }

    pub fn mark_read(&mut self, id: &str) {
        if let Some(ch) = self
            .categories
            .iter_mut()
            .flat_map(|c| &mut c.channels)
            .find(|ch| ch.id == id)
        {
            ch.unread = false;
        }
    }

    pub fn find_channel(&self, channel_id: &str) -> Option<&Channel> {
        self.categories
            .iter()
            .flat_map(|category| &category.channels)
            .find(|channel| channel.id == channel_id)
    }
}

impl From<ApiChannelDesc> for Channel {
    fn from(c: ApiChannelDesc) -> Self {
        Self {
            id: c.channel_id,
            name: c.channel_label,
            channel_type: ChannelType::Text,
            unread: c.count_mess_unread > 0,
            private: c.channel_private != 0,
            clan_id: c.clan_id,
            category_name: c.category_name,
            category_id: Some(c.category_id).filter(|s| !s.is_empty() && s != "0"),
        }
    }
}
