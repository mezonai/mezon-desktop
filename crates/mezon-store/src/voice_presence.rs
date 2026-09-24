use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct VoicePresence {
    peers: HashMap<(i64, i64, i64), HashSet<i32>>,
    revisions: HashMap<i64, u64>,
}

impl VoicePresence {
    pub fn revision(&self, clan: i64) -> u64 {
        self.revisions.get(&clan).copied().unwrap_or_default()
    }

    fn changed(&mut self, clan: i64) {
        *self.revisions.entry(clan).or_default() += 1;
    }

    pub fn joined(&mut self, clan: i64, channel: i64, user: i64, peer: i32) {
        self.changed(clan);
        if peer > 0 {
            self.peers
                .entry((clan, channel, user))
                .or_default()
                .insert(peer);
        }
    }

    pub fn left(&mut self, clan: i64, channel: i64, user: i64, peer: i32) -> bool {
        self.changed(clan);
        let key = (clan, channel, user);
        if let Some(peers) = self.peers.get_mut(&key) {
            peers.remove(&peer);
            if !peers.is_empty() {
                return false;
            }
        }
        self.peers.remove(&key);
        true
    }

    pub fn peer_ids(&self, clan: i64, channel: i64, user: i64) -> Vec<i32> {
        let mut ids: Vec<_> = self
            .peers
            .get(&(clan, channel, user))
            .into_iter()
            .flatten()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    pub fn forget_clan(&mut self, clan: i64) {
        self.changed(clan);
        self.peers.retain(|(c, _, _), _| *c != clan);
    }

    pub fn forget_channel(&mut self, clan: i64, channel: i64) {
        self.changed(clan);
        self.peers
            .retain(|(c, ch, _), _| *c != clan || *ch != channel);
    }

    pub fn replace_clan(&mut self, clan: i64, peers: &[(i64, i64, i32)]) {
        let mut next = HashMap::<_, HashSet<_>>::new();
        for &(channel, user, peer) in peers {
            let ids = next.entry((clan, channel, user)).or_default();
            if peer > 0 {
                ids.insert(peer);
            }
        }
        for (key, ids) in &mut next {
            if ids.is_empty() {
                if let Some(previous) = self.peers.get(key) {
                    ids.extend(previous);
                }
            }
        }
        self.forget_clan(clan);
        self.peers.extend(next);
    }
}
