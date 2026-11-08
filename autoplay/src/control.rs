//! Who is allowed to command what, checked every time.
//!
//! The review of the plan put one rule above the rest: **every operation that can
//! change game state passes a single control check**, and specifically noted that
//! being legal when a team was created does not make it legal later - a player can
//! take the units, a mind control can flip them, a house can be defeated, and the
//! automated player can be switched off mid-match.
//!
//! So this is not a startup flag. It is a value that is asked, per object, per term,
//! and it answers three separate questions that a single boolean would blur:
//!
//! - Is the automated player on at all, and is this subsystem of it on?
//! - Does this house belong to the automated player?
//! - May this particular object be commanded right now?
//!
//! The third is the one that changes during a match, and the one whose absence made
//! the first B1 probe conclude that movement did not work: a command naming an object
//! the issuing house does not own is admitted by the queue and then silently dropped.

use crate::claims::Conceded;
use vera20k_core::sim::intern::InternedId;

/// The automated player's authority over one house, at one moment.
///
/// Constructed from the launch, then asked per object. Cheap to copy so a caller can
/// take it by value, and deliberately not `Default`: a zeroed control state grants
/// nothing, which is the safe direction, but writing `Default` at a call site would
/// hide that the house is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlState {
    /// Whether the automated player is on at all.
    ///
    /// Off stops every subsystem, including any team already formed. That last part
    /// is the review's explicit worry: an automated player switched off while a team
    /// it created is still running would keep commanding units after being told to
    /// stop.
    global_enabled: bool,
    /// Whether it may queue production and place buildings.
    production_enabled: bool,
    /// Whether it may order units to move, attack or guard.
    combat_enabled: bool,
    /// Whether it may use special abilities, deploy, or undeploy.
    ability_enabled: bool,
    /// The house it acts for. Everything else is somebody else's.
    house: InternedId,
}

impl ControlState {
    /// A state that grants nothing, for a house that has no automated player.
    pub const fn disabled(house: InternedId) -> Self {
        Self {
            global_enabled: false,
            production_enabled: false,
            combat_enabled: false,
            ability_enabled: false,
            house,
        }
    }

    /// Full authority for one house.
    pub const fn enabled(house: InternedId) -> Self {
        Self {
            global_enabled: true,
            production_enabled: true,
            combat_enabled: true,
            ability_enabled: true,
            house,
        }
    }

    /// The house this state acts for.
    pub const fn house(&self) -> InternedId {
        self.house
    }

    /// Whether anything at all may be commanded.
    pub const fn is_global_enabled(&self) -> bool {
        self.global_enabled
    }

    /// Switch the whole automated player off or on.
    ///
    /// Turning it off leaves the subsystem flags as they were, so switching it back
    /// on restores the previous arrangement rather than silently enabling everything.
    pub fn set_global_enabled(&mut self, enabled: bool) {
        self.global_enabled = enabled;
    }

    /// Switch one subsystem, without touching the others.
    pub fn set_subsystem_enabled(&mut self, subsystem: Subsystem, enabled: bool) {
        let slot = match subsystem {
            Subsystem::Production => &mut self.production_enabled,
            Subsystem::Combat => &mut self.combat_enabled,
            Subsystem::Ability => &mut self.ability_enabled,
        };
        *slot = enabled;
    }

    /// Whether one subsystem is on, taking the global switch into account.
    pub fn is_subsystem_enabled(&self, subsystem: Subsystem) -> bool {
        self.global_enabled
            && match subsystem {
                Subsystem::Production => self.production_enabled,
                Subsystem::Combat => self.combat_enabled,
                Subsystem::Ability => self.ability_enabled,
            }
    }

    /// Whether an object of `owner` may be given a command in `subsystem`.
    ///
    /// The ownership test is not redundant with the house field: `house` says which
    /// house the automated player speaks for, and `owner` says who actually holds the
    /// object now. They diverge whenever control changes hands mid-match, which is
    /// exactly the case the review asked to be handled - so both are checked, every
    /// time, rather than trusting a value captured when a team was formed.
    pub fn may_command(&self, owner: InternedId, subsystem: Subsystem) -> bool {
        owner == self.house && self.is_subsystem_enabled(subsystem)
    }
}

/// Which part of the automated player an operation belongs to.
///
/// Separate switches because the review asked for them to run independently: a
/// player may want production handled while fighting manually, and the plan's own
/// control-state section lists them separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    /// Queuing production and placing buildings.
    Production,
    /// Moving, attacking and guarding.
    Combat,
    /// Deploying, undeploying and special abilities.
    Ability,
}

/// Why a command was refused.
///
/// A refusal is data, not a silently dropped order: the plan's executor has to be able
/// to say why, and the first B1 probe showed what silence costs. Each variant names
/// something a caller can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The automated player is off, so nothing may be commanded.
    GlobalDisabled,
    /// This subsystem is off.
    SubsystemDisabled(Subsystem),
    /// The object belongs to a house the automated player does not speak for.
    ///
    /// The case that blocked B1: the engine admits such a command and then ignores it,
    /// so a caller that does not check here will believe it succeeded.
    NotOurObject {
        /// Who holds the object now.
        owner: InternedId,
        /// Who the automated player speaks for.
        house: InternedId,
    },
    /// No object with that id exists, or it has been destroyed.
    NoSuchObject { id: u64 },
    /// A better claim holds the unit, so this order would be covering somebody else's.
    ///
    /// Separate from the others because it is not a defect: the caller did everything right
    /// and the world simply has a competing claim, which the plan says to respect.
    Conceded(Conceded),
}

impl Refusal {
    /// One line saying what to fix, for a log.
    pub fn describe(&self) -> String {
        match self {
            Self::GlobalDisabled => "the automated player is switched off".to_string(),
            Self::SubsystemDisabled(subsystem) => {
                format!("the {subsystem:?} subsystem is switched off")
            }
            Self::NotOurObject { owner, house } => {
                format!("object belongs to {owner:?}, this player speaks for {house:?}")
            }
            Self::NoSuchObject { id } => format!("no object with id {id}"),
            Self::Conceded(conceded) => conceded.describe(),
        }
    }
}
