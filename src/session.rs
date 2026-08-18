//! Collision-safe session selection from message-history prefixes.

use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};

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

/// Bounded, content-free inputs used to detect repeated tool activity within a
/// selected session. Raw tool names, arguments, results, and identifiers never
/// leave process memory: callers supply only hashed event keys, result digests,
/// and hashed argument tokens.
#[derive(Debug, Clone)]
pub enum ThrashEvent {
    ToolCall {
        event_key: String,
        call_key: String,
        tool: String,
        tokens: BTreeSet<String>,
        is_edit: bool,
    },
    ToolResult {
        event_key: String,
        call_key: Option<String>,
        digest: String,
        wire_cheap: bool,
        failed: bool,
    },
}

impl ThrashEvent {
    pub fn mark_wire_cheap(&mut self, event_key: &str) {
        if let Self::ToolResult {
            event_key: candidate,
            wire_cheap,
            ..
        } = self
            && candidate == event_key
        {
            *wire_cheap = true;
        }
    }
}

/// Per-request contribution to a deterministic session thrash score. The score
/// is the sum of three distinct repeat indicators; a failed edit/retry cycle is
/// not inferred from a generic failed tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThrashObservation {
    pub exact_repeated_tool_results: u32,
    pub expensive_exact_repeated_tool_results: u32,
    pub wire_cheap_exact_repeated_tool_results: u32,
    pub near_repeated_tool_calls: u32,
    pub edit_fail_edit_cycles: u32,
}

impl ThrashObservation {
    pub fn score(&self) -> u32 {
        self.exact_repeated_tool_results
            .saturating_add(self.near_repeated_tool_calls)
            .saturating_add(self.edit_fail_edit_cycles)
    }

    pub fn is_empty(&self) -> bool {
        self.score() == 0
    }
}

#[derive(Debug, Clone)]
struct ToolCallSignature {
    tool: String,
    tokens: BTreeSet<String>,
}

#[derive(Debug)]
struct SessionEntry {
    history: Vec<String>,
    /// Content digests already observed in this isolated session. Kept with the
    /// history so dedup runs only after session choice and shares its lifetime.
    seen_blobs: HashSet<String>,
    last_access_secs: u64,
    advisory_arm_inject: Option<bool>,
    advisory_attempted: bool,
    seen_tool_event_keys: HashSet<String>,
    seen_tool_result_digests: HashSet<String>,
    prior_tool_calls: Vec<ToolCallSignature>,
    edit_call_keys: HashSet<String>,
    pending_failed_edit: bool,
    next_turn: u64,
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
    /// Reserve this session's one eligible advisory turn. The arm is retained
    /// locally for the session lifetime; `randomized` only controls the stable
    /// assignment for a newly seen session.
    pub fn reserve_advisory_turn(&mut self, id: &str, randomized: bool) -> Option<bool> {
        let entry = self.sessions.get_mut(id)?;
        if entry.advisory_attempted {
            return None;
        }
        entry.advisory_attempted = true;
        let inject = *entry.advisory_arm_inject.get_or_insert_with(|| {
            randomized && content_digest(id.as_bytes()).as_bytes()[0] % 2 == 0
        });
        Some(inject)
    }

    /// Reserve a monotonically increasing request turn before it is dispatched.
    /// The ordinal is local-only and lets transcript and cost receipts join even
    /// when asynchronous responses complete out of append order.
    pub fn reserve_turn(&mut self, selection: &Match, now_secs: u64) -> u64 {
        let id = match selection {
            Match::Existing(id) | Match::New { id, .. } => id,
        };
        let entry = self
            .sessions
            .entry(id.clone())
            .or_insert_with(|| SessionEntry::new(now_secs));
        entry.last_access_secs = now_secs;
        entry.next_turn = entry.next_turn.saturating_add(1);
        entry.next_turn
    }

    /// Incorporate new, opaque tool events for one selected session. Replayed
    /// request history is ignored by event key, while equal result digests from
    /// distinct tool results are counted as exact repetition.
    pub fn observe_thrash(
        &mut self,
        selection: &Match,
        events: impl IntoIterator<Item = ThrashEvent>,
        now_secs: u64,
    ) -> ThrashObservation {
        let id = match selection {
            Match::Existing(id) | Match::New { id, .. } => id,
        };
        let entry = self
            .sessions
            .entry(id.clone())
            .or_insert_with(|| SessionEntry::new(now_secs));
        entry.last_access_secs = now_secs;
        let mut observation = ThrashObservation::default();
        for event in events {
            let event_key = match &event {
                ThrashEvent::ToolCall { event_key, .. }
                | ThrashEvent::ToolResult { event_key, .. } => event_key,
            };
            if !entry.seen_tool_event_keys.insert(event_key.clone()) {
                continue;
            }
            match event {
                ThrashEvent::ToolCall {
                    call_key,
                    tool,
                    tokens,
                    is_edit,
                    ..
                } => {
                    if entry.prior_tool_calls.iter().any(|prior| {
                        prior.tool == tool
                            && prior.tokens != tokens
                            && token_similarity(&prior.tokens, &tokens) >= 0.60
                    }) {
                        observation.near_repeated_tool_calls += 1;
                    }
                    if is_edit && entry.pending_failed_edit {
                        observation.edit_fail_edit_cycles += 1;
                        entry.pending_failed_edit = false;
                    }
                    if is_edit {
                        entry.edit_call_keys.insert(call_key);
                    }
                    entry
                        .prior_tool_calls
                        .push(ToolCallSignature { tool, tokens });
                    if entry.prior_tool_calls.len() > 128 {
                        entry.prior_tool_calls.remove(0);
                    }
                }
                ThrashEvent::ToolResult {
                    call_key,
                    digest,
                    wire_cheap,
                    failed,
                    ..
                } => {
                    if !entry.seen_tool_result_digests.insert(digest) {
                        observation.exact_repeated_tool_results += 1;
                        if wire_cheap {
                            observation.wire_cheap_exact_repeated_tool_results += 1;
                        } else {
                            observation.expensive_exact_repeated_tool_results += 1;
                        }
                    }
                    if failed
                        && call_key
                            .as_ref()
                            .is_some_and(|key| entry.edit_call_keys.contains(key))
                    {
                        entry.pending_failed_edit = true;
                    }
                }
            }
        }
        observation
    }

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
            advisory_arm_inject: None,
            advisory_attempted: false,
            seen_tool_event_keys: HashSet::new(),
            seen_tool_result_digests: HashSet::new(),
            prior_tool_calls: Vec::new(),
            edit_call_keys: HashSet::new(),
            pending_failed_edit: false,
            next_turn: 0,
        }
    }
}

fn token_similarity(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(right).count() as f64 / union as f64
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
    fn reserves_monotonic_turns_for_each_session() {
        let mut registry = Registry::default();
        let first = Match::New {
            id: "s".into(),
            collision: false,
        };
        assert_eq!(registry.reserve_turn(&first, 0), 1);
        assert_eq!(registry.reserve_turn(&Match::Existing("s".into()), 1), 2);
        assert_eq!(
            registry.reserve_turn(
                &Match::New {
                    id: "other".into(),
                    collision: false,
                },
                2,
            ),
            1
        );
    }

    #[test]
    fn thrash_observation_distinguishes_exact_near_and_edit_retry_events() {
        let mut registry = Registry::default();
        let session = Match::New {
            id: "s".into(),
            collision: false,
        };
        let edit = |event_key: &str, call_key: &str, tokens: &[&str]| ThrashEvent::ToolCall {
            event_key: event_key.into(),
            call_key: call_key.into(),
            tool: "edit".into(),
            tokens: tokens.iter().map(|token| (*token).into()).collect(),
            is_edit: true,
        };
        let result =
            |event_key: &str, call_key: &str, digest: &str, wire_cheap: bool, failed: bool| {
                ThrashEvent::ToolResult {
                    event_key: event_key.into(),
                    call_key: Some(call_key.into()),
                    digest: digest.into(),
                    wire_cheap,
                    failed,
                }
            };

        assert!(
            registry
                .observe_thrash(
                    &session,
                    [
                        edit("call-1", "edit-1", &["file", "src", "line", "one"]),
                        result("result-1", "edit-1", "digest-a", false, true)
                    ],
                    0,
                )
                .is_empty()
        );
        let observation = registry.observe_thrash(
            &session,
            [
                edit("call-2", "edit-2", &["file", "src", "line", "two"]),
                result("result-2", "edit-2", "digest-a", true, false),
            ],
            1,
        );
        assert_eq!(observation.near_repeated_tool_calls, 1);
        assert_eq!(observation.edit_fail_edit_cycles, 1);
        assert_eq!(observation.exact_repeated_tool_results, 1);
        assert_eq!(observation.wire_cheap_exact_repeated_tool_results, 1);
        assert_eq!(observation.expensive_exact_repeated_tool_results, 0);
        assert_eq!(observation.score(), 3);

        // Replayed history is not a new observation.
        assert!(
            registry
                .observe_thrash(
                    &session,
                    [result("result-2", "edit-2", "digest-a", true, false)],
                    2,
                )
                .is_empty()
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
