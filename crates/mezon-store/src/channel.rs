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
