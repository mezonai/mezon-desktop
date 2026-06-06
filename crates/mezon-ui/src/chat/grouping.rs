use mezon_store::Message;

pub const COMBINE_TIME_WINDOW: i64 = 300;

#[derive(Debug)]
pub struct MessageGroup<'a> {
    pub messages: Vec<&'a Message>,
    pub combined: Vec<bool>,
}

pub fn group_messages(messages: &[Message]) -> Vec<MessageGroup<'_>> {
    let mut groups: Vec<MessageGroup> = Vec::new();

    for msg in messages {
        if let Some(last_group) = groups.last_mut()
            && let Some(last_msg) = last_group.messages.last()
            && last_msg.sender_id == msg.sender_id
            && (msg.create_time - last_msg.create_time).abs() < COMBINE_TIME_WINDOW
        {
            last_group.combined.push(true);
            last_group.messages.push(msg);
            continue;
        }

        groups.push(MessageGroup {
            combined: vec![false],
            messages: vec![msg],
        });
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(id: &str, sender: &str, time: i64) -> Message {
        Message::new(id, "", sender, sender, time)
    }

    #[test]
    fn same_sender_within_window_combines() {
        let msgs = vec![make_msg("1", "alice", 1000), make_msg("2", "alice", 1100)];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].messages.len(), 2);
        assert_eq!(groups[0].combined, vec![false, true]);
    }

    #[test]
    fn different_sender_breaks_group() {
        let msgs = vec![make_msg("1", "alice", 1000), make_msg("2", "bob", 1100)];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn time_window_expires_breaks_group() {
        let msgs = vec![make_msg("1", "alice", 1000), make_msg("2", "alice", 1400)];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn mock_messages_produce_expected_groups() {
        let now = 10000i64;
        let msgs = vec![
            make_msg("1", "alice", now - 360),
            make_msg("2", "bob", now - 300),
            make_msg("3", "charlie", now - 240),
            make_msg("4", "bob", now - 180),
            make_msg("5", "alice", now - 120),
        ];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 5);
        for group in &groups {
            assert_eq!(group.messages.len(), 1);
            assert_eq!(group.combined, vec![false]);
        }
    }
}
