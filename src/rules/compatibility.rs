//! Build capability recorded by the retained Rules owner and save metadata.
//!
//! This is a VERA compatibility boundary, not a field from gamemd, Ares or
//! Phobos. Keeping one wire variant per compiled capability lets a binary
//! explicitly refuse the other mode. Extension catalogs added later belong to
//! the retained Rules registry, not a second mutable policy on RuleSet or
//! Simulation.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RulesCompatibility {
    Retail,
    Ares,
    Phobos,
    AresPhobos,
}

impl Default for RulesCompatibility {
    fn default() -> Self {
        match (super::ARES_ENABLED, super::PHOBOS_ENABLED) {
            (true, true) => Self::AresPhobos,
            (true, false) => Self::Ares,
            (false, true) => Self::Phobos,
            (false, false) => Self::Retail,
        }
    }
}

impl RulesCompatibility {
    pub(crate) fn matches_build(self) -> bool {
        self == Self::default()
    }
}

#[cfg(test)]
#[path = "compatibility_tests.rs"]
mod tests;
