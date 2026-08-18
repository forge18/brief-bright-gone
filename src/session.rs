//! Collision-safe session selection from message-history prefixes.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Existing(String),
    New { id: String, collision: bool },
}

/// Default inactivity window before a session is evicted. Eviction is what
/// makes `pinned_digests` liveness-based: an abandoned session stops pinning
/// its blobs so `collect_unpinned` can free them without a proxy restart.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 3600;
/// Hard ceiling on retained sessions; the least-recently-accessed are evicted
/// past this even within the TTL, bounding memory under session churn.
pub const DEFAULT_MAX_SESSIONS: usize = 1024;

#[derive(Debug)]
struct SessionEntry {
    history: Vec<String>,
    /// Content digests already observed in this isolated session. Kept with the
    /// history so dedup runs only after session choice and shares its lifetime.
    seen_blobs: HashSet<String>,
    last_access_secs: u64,
}

#[derive(Debug)]
pub struct Registry {
    sessions: HashMap<String, SessionEntry>,
    next_id: u64,
    ttl_secs: u64,
    max_sessions: usize,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(DEFAULT_SESSION_TTL_SECS, DEFAULT_MAX_SESSIONS)
    }
}

impl Registry {
    pub fn new(ttl_secs: u64, max_sessions: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            next_id: 0,
            ttl_secs,
            max_sessions,
        }
    }

    pub fn select(&mut self, history: &[String], now_secs: u64) -> Match {
        self.evict(now_secs);
        let best = self
            .sessions
            .iter()
            .filter_map(|(id, entry)| {
                let common = history
                    .iter()
                    .zip(&entry.history)
                    .take_while(|(a, b)| a == b)
                    .count();
                (common > 0).then_some((id.clone(), common))
            })
            .collect::<Vec<_>>();
        let longest = best.iter().map(|(_, length)| *length).max().unwrap_or(0);
        let candidates = best
            .into_iter()
            .filter(|(_, length)| *length == longest)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            let id = candidates[0].0.clone();
            // Selecting a session is an access; keep it warm against eviction.
            if let Some(entry) = self.sessions.get_mut(&id) {
                entry.last_access_secs = now_secs;
            }
            return Match::Existing(id);
        }
        let id = self.new_id(history);
        Match::New {
            id,
            collision: longest > 0,
        }
    }

    pub fn record(&mut self, id: String, history: Vec<String>, now_secs: u64) {
        let entry = self
            .sessions
            .entry(id)
            .or_insert_with(|| SessionEntry::new(now_secs));
        entry.history = history;
        entry.last_access_secs = now_secs;
        self.evict(now_secs);
    }

    /// Return true only for a uniquely selected existing session that has
    /// already observed these bytes. New and collision sessions fail open.
    pub fn seen_in_existing_session(&self, selection: &Match, bytes: &[u8]) -> bool {
        let Match::Existing(id) = selection else {
            return false;
        };
        self.sessions
            .get(id)
            .is_some_and(|entry| entry.seen_blobs.contains(&content_digest(bytes)))
    }

    /// Record a file read after session selection. Collision sessions are never
    /// shared with another session, even if their generated id is reused later.
    pub fn remember(&mut self, selection: &Match, bytes: &[u8], now_secs: u64) {
        let id = match selection {
            Match::Existing(id) | Match::New { id, .. } => id.clone(),
        };
        let entry = self
            .sessions
            .entry(id)
            .or_insert_with(|| SessionEntry::new(now_secs));
        entry.seen_blobs.insert(content_digest(bytes));
        entry.last_access_secs = now_secs;
    }

    /// Digests referenced by active in-memory sessions. Callers use this as
    /// the liveness set for conservative collection.
    pub fn pinned_digests(&self) -> HashSet<String> {
        self.sessions
            .values()
            .flat_map(|entry| entry.seen_blobs.iter().cloned())
            .collect()
    }

    /// Drop sessions idle past the TTL, then, if still over the cap, drop the
    /// least-recently-accessed until at the cap. Called on every mutation so
    /// growth is bounded without a background task.
    fn evict(&mut self, now_secs: u64) {
        self.sessions
            .retain(|_, entry| now_secs.saturating_sub(entry.last_access_secs) <= self.ttl_secs);
        if self.sessions.len() <= self.max_sessions {
            return;
        }
        let mut by_access = self
            .sessions
            .iter()
            .map(|(id, entry)| (entry.last_access_secs, id.clone()))
            .collect::<Vec<_>>();
        by_access.sort_unstable();
        let excess = self.sessions.len() - self.max_sessions;
        for (_, id) in by_access.into_iter().take(excess) {
            self.sessions.remove(&id);
        }
    }

    fn new_id(&mut self, history: &[String]) -> String {
        self.next_id += 1;
        let mut hasher = Sha256::new();
        for message in history {
            hasher.update(message.as_bytes());
            hasher.update([0]);
        }
        hasher.update(self.next_id.to_be_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl SessionEntry {
    fn new(now_secs: u64) -> Self {
        Self {
            history: Vec::new(),
            seen_blobs: HashSet::new(),
            last_access_secs: now_secs,
        }
    }
}

fn content_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn history(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn selects_unique_longest_prefix() {
        let mut registry = Registry::default();
        registry.record("a".into(), history(&["one", "two"]), 0);
        registry.record("b".into(), history(&["one", "other"]), 0);
        assert_eq!(
            registry.select(&history(&["one", "two", "three"]), 0),
            Match::Existing("a".into())
        );
    }
    #[test]
    fn isolates_tied_prefix_collisions() {
        let mut registry = Registry::default();
        registry.record("a".into(), history(&["one"]), 0);
        registry.record("b".into(), history(&["one"]), 0);
        assert!(matches!(
            registry.select(&history(&["one", "two"]), 0),
            Match::New {
                collision: true,
                ..
            }
        ));
    }

    #[test]
    fn exposes_all_live_session_pins() {
        let mut registry = Registry::default();
        let session = Match::New {
            id: "s".into(),
            collision: false,
        };
        registry.remember(&session, b"first", 0);
        registry.remember(&session, b"second", 0);
        let pins = registry.pinned_digests();
        assert!(pins.contains(&content_digest(b"first")));
        assert!(pins.contains(&content_digest(b"second")));
    }

    #[test]
    fn evicts_sessions_idle_past_the_ttl_and_unpins_their_blobs() {
        let mut registry = Registry::new(100, 1024);
        let session = Match::New {
            id: "old".into(),
            collision: false,
        };
        registry.remember(&session, b"blob", 0);
        registry.record("old".into(), history(&["one"]), 0);
        assert!(registry.pinned_digests().contains(&content_digest(b"blob")));

        // A later request past the TTL evicts the idle session, so its blob is
        // no longer pinned and becomes collectible.
        assert!(matches!(
            registry.select(&history(&["fresh"]), 200),
            Match::New { .. }
        ));
        assert!(registry.pinned_digests().is_empty());
    }

    #[test]
    fn active_sessions_survive_eviction_when_touched() {
        let mut registry = Registry::new(100, 1024);
        registry.record("keep".into(), history(&["one", "two"]), 0);
        // Re-selecting within the TTL refreshes last access.
        assert_eq!(
            registry.select(&history(&["one", "two"]), 90),
            Match::Existing("keep".into())
        );
        // A request at t=180 is 180s after the original record but only 90s
        // after the select, so the touched session survives.
        registry.record("other".into(), history(&["x"]), 180);
        assert_eq!(
            registry.select(&history(&["one", "two"]), 185),
            Match::Existing("keep".into())
        );
    }

    #[test]
    fn lru_cap_bounds_retained_sessions() {
        let mut registry = Registry::new(1_000_000, 2);
        registry.record("a".into(), history(&["a"]), 1);
        registry.record("b".into(), history(&["b"]), 2);
        registry.record("c".into(), history(&["c"]), 3);
        // Cap of 2: the least-recently-accessed ("a") is dropped.
        assert!(matches!(
            registry.select(&history(&["a", "z"]), 4),
            Match::New { .. }
        ));
        assert_eq!(
            registry.select(&history(&["c"]), 5),
            Match::Existing("c".into())
        );
    }
}
