//! Collision-safe session selection from message-history prefixes.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Existing(String),
    New { id: String, collision: bool },
}

#[derive(Debug, Default)]
pub struct Registry {
    histories: HashMap<String, Vec<String>>,
    /// Content digests already observed in each isolated session. This is kept
    /// separate from history matching so dedup runs only after session choice.
    seen_blobs: HashMap<String, HashSet<String>>,
    next_id: u64,
}

impl Registry {
    pub fn select(&mut self, history: &[String]) -> Match {
        let best = self
            .histories
            .iter()
            .filter_map(|(id, known)| {
                let common = history
                    .iter()
                    .zip(known)
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
            return Match::Existing(candidates[0].0.clone());
        }
        let id = self.new_id(history);
        Match::New {
            id,
            collision: longest > 0,
        }
    }

    pub fn record(&mut self, id: String, history: Vec<String>) {
        self.histories.insert(id, history);
    }

    /// Return true only for a uniquely selected existing session that has
    /// already observed these bytes. New and collision sessions fail open.
    pub fn seen_in_existing_session(&self, selection: &Match, bytes: &[u8]) -> bool {
        let Match::Existing(id) = selection else {
            return false;
        };
        self.seen_blobs
            .get(id)
            .is_some_and(|seen| seen.contains(&content_digest(bytes)))
    }

    /// Record a file read after session selection. Collision sessions are never
    /// shared with another session, even if their generated id is reused later.
    pub fn remember(&mut self, selection: &Match, bytes: &[u8]) {
        let id = match selection {
            Match::Existing(id) | Match::New { id, .. } => id,
        };
        self.seen_blobs
            .entry(id.clone())
            .or_default()
            .insert(content_digest(bytes));
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
        registry.record("a".into(), history(&["one", "two"]));
        registry.record("b".into(), history(&["one", "other"]));
        assert_eq!(
            registry.select(&history(&["one", "two", "three"])),
            Match::Existing("a".into())
        );
    }
    #[test]
    fn isolates_tied_prefix_collisions() {
        let mut registry = Registry::default();
        registry.record("a".into(), history(&["one"]));
        registry.record("b".into(), history(&["one"]));
        assert!(matches!(
            registry.select(&history(&["one", "two"])),
            Match::New {
                collision: true,
                ..
            }
        ));
    }
}
