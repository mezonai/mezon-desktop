use mezon_client::transport::ApiChannelDesc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Text,
    Voice,
}

impl ChannelType {
    pub fn as_proto_int(&self) -> i32 {
        match self {
            ChannelType::Text => 0,
            ChannelType::Voice => 1,
        }
    }
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
    pub member_count: u32,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub content: String,
    pub sender_id: String,
    pub sender_name: String,
    pub create_time: i64,
    pub reactions: Vec<String>,
    pub attachments: Vec<String>,
}

impl Message {
    pub fn new(
        id: impl Into<String>,
        content: impl Into<String>,
        sender_id: impl Into<String>,
        sender_name: impl Into<String>,
        create_time: i64,
    ) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            sender_id: sender_id.into(),
            sender_name: sender_name.into(),
            create_time,
            reactions: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Category {
    pub clan_id: String,
    pub name: String,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub struct PendingSubscribe {
    pub clan_id: String,
    pub channel_id: String,
    pub channel_type: i32,
    pub is_public: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelList {
    pub categories: Vec<Category>,
    pub active_channel_id: Option<String>,
    pub pending_subscribe: Option<PendingSubscribe>,
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
            member_count: c.member_count as u32,
        }
    }
}
