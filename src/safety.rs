//! Safety-class registry (S0–S4), the honesty contract of every transform.
//!
//! Modeled on the general agent-communication-compression principle that each
//! transform class is *inherent* to the method, not a user choice, and answers
//! two questions: does it change model-visible bytes, and does it require a
//! recoverable record before it may run.

/// Position on the safety ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Byte-safe behavior (metadata, accounting). No model-visible bytes change.
    S0,
    /// Provider-native hints (cache, routing). No model-visible bytes change.
    S1,
    /// Structural changes that need SDK cooperation.
    S2,
    /// Behavioral changes (routing, reasoning); eval-gated in cloud deployments.
    S3,
    /// Lossy structural compression. Alters model-visible bytes: opt-in,
    /// must be reversible (original stored), and discloses what it dropped.
    S4,
}

/// The honesty contract of a safety class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub class: Class,
    pub name: &'static str,
    /// True when the class never alters model-visible bytes.
    pub byte_safe: bool,
    /// True when lossy and may only run if original bytes are stored first.
    pub requires_ccr: bool,
    /// True when the transform is lossless to the model (full value retained).
    pub reversible: bool,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::S0 => "S0",
            Class::S1 => "S1",
            Class::S2 => "S2",
            Class::S3 => "S3",
            Class::S4 => "S4",
        }
    }

    pub fn info(self) -> Info {
        match self {
            Class::S0 => Info { class: self, name: "S0", byte_safe: true, requires_ccr: false, reversible: true },
            Class::S1 => Info { class: self, name: "S1", byte_safe: true, requires_ccr: false, reversible: true },
            Class::S2 => Info { class: self, name: "S2", byte_safe: false, requires_ccr: false, reversible: true },
            Class::S3 => Info { class: self, name: "S3", byte_safe: false, requires_ccr: false, reversible: false },
            Class::S4 => Info { class: self, name: "S4", byte_safe: false, requires_ccr: true, reversible: false },
        }
    }
}

/// The outcome of evaluating whether a transform may run.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Safe to run without recovery.
    Allowed,
    /// Allowed only if the original bytes are recorded (CCR) first.
    AllowedWithCcr,
    /// Not allowed under current policy.
    Denied,
}

/// Gate a transform: given a safety class and whether reversible recovery is
/// currently provisioned, return a verdict.
///
/// Policy: S0/S1 always allowed. S2 allowed (structural, SDK-cooperative).
/// S3 allowed but behavioral (eval-gated). S4 requires CCR; without it, denied
/// (fail closed).
pub fn gate(class: Class, ccr_available: bool) -> Verdict {
    let info = class.info();
    if info.byte_safe {
        return Verdict::Allowed;
    }
    if info.requires_ccr {
        return if ccr_available { Verdict::AllowedWithCcr } else { Verdict::Denied };
    }
    Verdict::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_safe_classes_always_allow() {
        assert_eq!(gate(Class::S0, false), Verdict::Allowed);
        assert_eq!(gate(Class::S1, false), Verdict::Allowed);
    }

    #[test]
    fn structural_is_allowed() {
        assert_eq!(gate(Class::S2, false), Verdict::Allowed);
    }

    #[test]
    fn lossy_requires_ccr_and_fails_closed_without_it() {
        assert_eq!(gate(Class::S4, false), Verdict::Denied);
        assert_eq!(gate(Class::S4, true), Verdict::AllowedWithCcr);
    }
}