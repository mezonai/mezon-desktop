use std::borrow::Borrow;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

struct Entry<V> {
    value: V,
    fetched_at: Option<Instant>,
}

/// A keyed cache with per-entry TTL and optional LRU bound — the reusable core of the stores'
/// "fetch once, reuse from store, refetch when stale/reconnect" model (cf. React `cache-metadata`
/// + entity adapter). The store owns the value type and decides what each key means.
pub struct KeyedCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    order: VecDeque<K>,
    max: Option<usize>,
}

impl<K: Eq + Hash + Clone, V> KeyedCache<K, V> {
    pub fn new(max: Option<usize>) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            max,
        }
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.entries.contains_key(key)
    }

    /// `true` if the key is cached **and** fetched within `ttl` (the `shouldForceApiCall` check).
    pub fn is_fresh<Q>(&self, key: &Q, ttl: Duration) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.entries
            .get(key)
            .and_then(|e| e.fetched_at)
            .is_some_and(|t| t.elapsed() < ttl)
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.entries.get(key).map(|e| &e.value)
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.entries.get_mut(key).map(|e| &mut e.value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.values_mut().map(|e| &mut e.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, e)| (k, &e.value))
    }

    /// Insert (or replace) `key` with a fresh timestamp; evicts the LRU entry past `max`,
    /// never evicting `protect` (e.g. the active channel).
    pub fn insert(&mut self, key: K, value: V, protect: Option<&K>) {
        self.order.retain(|k| k != &key);
        self.entries.insert(
            key.clone(),
            Entry {
                value,
                fetched_at: Some(Instant::now()),
            },
        );
        self.order.push_back(key);
        self.evict(protect);
    }

    /// Mark `key` most-recently-used (no-op if absent).
    pub fn touch<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if let Some(pos) = self.order.iter().position(|k| k.borrow() == key)
            && let Some(k) = self.order.remove(pos)
        {
            self.order.push_back(k);
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.order.retain(|k| k != key);
        self.entries.remove(key).map(|e| e.value)
    }

    /// Drop every entry (force a full refetch with an empty cache).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Force a refetch on next access **without** dropping values — `is_fresh` returns false for
    /// every key but `get` still returns the last value (no empty flash on resync).
    pub fn mark_all_stale(&mut self) {
        for entry in self.entries.values_mut() {
            entry.fetched_at = None;
        }
    }

    fn evict(&mut self, protect: Option<&K>) {
        let Some(max) = self.max else {
            return;
        };
        while self.entries.len() > max {
            let Some(victim) = self.order.front().cloned() else {
                break;
            };
            if protect == Some(&victim) {
                if self.order.len() < 2 {
                    break;
                }
                self.order.pop_front();
                self.order.push_back(victim);
                continue;
            }
            self.order.pop_front();
            self.entries.remove(&victim);
        }
    }
}

pub struct Freshness {
    fetched_at: Option<Instant>,
}

impl Freshness {
    pub fn new() -> Self {
        Self { fetched_at: None }
    }

    pub fn is_fresh(&self, ttl: Duration) -> bool {
        self.fetched_at.is_some_and(|t| t.elapsed() < ttl)
    }

    pub fn mark_fetched(&mut self) {
        self.fetched_at = Some(Instant::now());
    }

    pub fn mark_stale(&mut self) {
        self.fetched_at = None;
    }
}

impl Default for Freshness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_starts_stale_then_tracks_fetch_and_stale() {
        let mut f = Freshness::new();
        assert!(!f.is_fresh(Duration::from_secs(60)));

        f.mark_fetched();
        assert!(f.is_fresh(Duration::from_secs(60)));
        assert!(!f.is_fresh(Duration::from_millis(0)));

        f.mark_stale();
        assert!(!f.is_fresh(Duration::from_secs(60)));
    }

    #[test]
    fn fresh_then_stale_keeps_value() {
        let mut c: KeyedCache<String, i32> = KeyedCache::new(None);
        c.insert("a".into(), 1, None);
        assert!(c.is_fresh("a", Duration::from_secs(60)));
        assert_eq!(c.get("a"), Some(&1));

        c.mark_all_stale();
        assert!(!c.is_fresh("a", Duration::from_secs(60)));
        assert_eq!(c.get("a"), Some(&1));
    }

    #[test]
    fn lru_evicts_oldest_past_max() {
        let mut c: KeyedCache<String, i32> = KeyedCache::new(Some(2));
        c.insert("a".into(), 1, None);
        c.insert("b".into(), 2, None);
        c.insert("c".into(), 3, None);
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("b"), Some(&2));
        assert_eq!(c.get("c"), Some(&3));
    }

    #[test]
    fn touch_changes_eviction_order() {
        let mut c: KeyedCache<String, i32> = KeyedCache::new(Some(2));
        c.insert("a".into(), 1, None);
        c.insert("b".into(), 2, None);
        c.touch("a");
        c.insert("c".into(), 3, None);
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("a"), Some(&1));
        assert_eq!(c.get("c"), Some(&3));
    }

    #[test]
    fn protect_keeps_protected_key_but_preserves_hard_bound() {
        let mut c: KeyedCache<String, i32> = KeyedCache::new(Some(2));
        c.insert("a".into(), 1, None);
        c.insert("b".into(), 2, None);
        c.insert("c".into(), 3, Some(&"a".to_string()));
        assert_eq!(c.get("a"), Some(&1));
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("c"), Some(&3));
    }

    #[test]
    fn protect_never_grows_past_max_while_other_victims_exist() {
        let protected = "keep".to_string();
        let mut c: KeyedCache<String, i32> = KeyedCache::new(Some(3));
        c.insert(protected.clone(), 0, None);
        for i in 1..20 {
            c.insert(format!("k{i}"), i, Some(&protected));
            assert!(c.get(&protected).is_some());
        }
        assert_eq!(c.get(&protected), Some(&0));
    }

    #[test]
    fn protect_may_exceed_max_only_when_no_other_victim() {
        let mut c: KeyedCache<String, i32> = KeyedCache::new(Some(1));
        c.insert("a".into(), 1, None);
        c.insert("b".into(), 2, Some(&"b".to_string()));
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("b"), Some(&2));
    }
}
