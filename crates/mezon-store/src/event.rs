use mezon_proto::realtime;

#[derive(Debug, Clone)]
pub enum RealtimeEvent {
    ChannelMessage(mezon_proto::api::ChannelMessage),
    Other { variant: String, payload: Vec<u8> },
}

impl RealtimeEvent {
    pub fn from_envelope(envelope: realtime::Envelope, raw_payload: Vec<u8>) -> Option<Self> {
        match envelope.message? {
            realtime::envelope::Message::ChannelMessage(msg) => Some(Self::ChannelMessage(msg)),
            other => {
                let variant = format!("{:?}", other)
                    .split('(')
                    .next()
                    .unwrap_or("Unknown")
                    .to_string();
                Some(Self::Other {
                    variant,
                    payload: raw_payload,
                })
            }
        }
    }
}
