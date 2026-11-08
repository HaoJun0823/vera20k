//! Who gets to write an order when more than one party wants to.
//!
//! B3's remaining two items are the same problem seen from two sides: more than one source
//! can command the same unit, and one of those sources is the player. The engine does not
//! help here, and it is worth stating why before the design: a `CommandEnvelope` carries an
//! owner, a due frame and a payload, and nothing else. There is no field recording who
//! issued the order or on whose behalf. Two envelopes that would write the same mission are
//! indistinguishable once they reach the simulation.
//!
//! So the ranking has to live here. That is a decision with a cost worth naming: this crate
//! cannot stop some other writer - the native team AI, a trigger - from reaching the same
//! unit, because it never sees their orders. What it *can* do is refuse to add to the pile
//! when a higher-priority source is already there, which is the part that would otherwise
//! make an automated player silently override a player's click.
//!
//! The ranking is deliberately short. Every source above the automated player exists to
//! prevent it from talking over somebody with a better claim:
//!
//! - [`Source::Player`] - a human clicked. The plan's rule is absolute: never covered.
//! - [`Source::Script`] - the map's triggers. Authored content outranks a policy.
//! - [`Source::AutoPlay`] - this crate.
//!
//! Ties go to the incumbent. Two orders from the same source on the same frame do not
//! escalate, and a lower source retrying does not climb the ranking - it stays where it is
//! and keeps being refused while a better claim holds the unit.

use std::collections::BTreeMap;

use vera20k_core::sim::intern::InternedId;

/// Where an order came from, ranked.
///
/// The numeric order is the ranking, and it is written so a comparison reads correctly: a
/// higher value outranks a lower one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// This crate.
    AutoPlay = 0,
    /// The map's trigger machinery.
    Script = 1,
    /// A human, or the local player's own input path.
    Player = 2,
}

impl Source {
    /// One word for a report.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::AutoPlay => "autoplay",
            Self::Script => "script",
            Self::Player => "player",
        }
    }
}

/// Why an order may not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conceded {
    /// A higher-priority source holds the unit.
    Outranked {
        /// Which source holds it.
        holder: Source,
        /// Which asked.
        asker: Source,
    },
    /// The unit belongs to another house, so no source in this crate may write it.
    NotThisHouse {
        /// Who holds the unit.
        owner: InternedId,
        /// Who this player speaks for.
        house: InternedId,
    },
}

impl Conceded {
    /// One line, naming both sides so a report can say who beat whom.
    pub fn describe(&self) -> String {
        match self {
            Self::Outranked { holder, asker } => format!(
                "{} outranks {} on this unit",
                holder.describe(),
                asker.describe()
            ),
            Self::NotThisHouse { owner, house } => {
                format!("unit belongs to {owner:?}, this player speaks for {house:?}")
            }
        }
    }
}

/// Who currently holds each unit, and with what claim.
///
/// Owned by the match. A fresh one is a fresh match, for the same reason the dispatcher and
/// the trace are: a claim from the last game must not block the first order of this one.
#[derive(Debug, Default)]
pub struct Claims {
    held: BTreeMap<u64, Source>,
    conceded: usize,
}

impl Claims {
    /// Nothing held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `asker` may write an order for `unit`.
    ///
    /// A source may always write over itself: the same party changing its mind is not a
    /// conflict. Anything else has to outrank the incumbent.
    pub fn may_write(&self, unit: u64, asker: Source) -> Result<(), Conceded> {
        match self.held.get(&unit) {
            None => Ok(()),
            Some(&holder) if asker >= holder => Ok(()),
            Some(&holder) => Err(Conceded::Outranked { holder, asker }),
        }
    }

    /// Record that `source` wrote an order for `unit`.
    ///
    /// Only call this for an order that was actually accepted; recording a refused one
    /// would let a losing source take the unit by failing loudly enough.
    pub fn claim(&mut self, unit: u64, source: Source) {
        self.held.insert(unit, source);
    }

    /// Release a claim, as a unit's controller should when it gives the unit up.
    ///
    /// Necessary for the release case: an automated player that conceded a unit must let go
    /// of it, or a player's later order would be compared against a claim nobody holds.
    pub fn release(&mut self, unit: u64) {
        self.held.remove(&unit);
    }

    /// The holder of `unit`, if any.
    pub fn holder(&self, unit: u64) -> Option<Source> {
        self.held.get(&unit).copied()
    }

    /// Count a refusal, so a report can say how often AutoPlay stepped aside.
    pub fn note_conceded(&mut self) {
        self.conceded += 1;
    }

    /// How many orders were refused for a better claim.
    pub const fn conceded(&self) -> usize {
        self.conceded
    }

    /// Forget a unit on destruction, so a recycled id does not inherit a claim.
    pub fn forget(&mut self, unit: u64) {
        self.held.remove(&unit);
    }
}
