//! What a unit can actually do right now, read rather than recomputed.
//!
//! The plan's C phase splits a unit's fighting ability into three layers, and the split is
//! the point: they fail for different reasons and a planner that conflates them asks the wrong
//! question.
//!
//! - **Capability** - what the type can do at all. It has an anti-air weapon. This comes from
//!   the rules and does not change during a match.
//! - **Condition** - what makes that capability apply. It needs a particular passenger, it has
//!   to be deployed first. Also from the rules, but conditional on state.
//! - **Runtime availability** - whether it can do it *now*. Out of ammo, reloading, loaded in a
//!   transport, disabled.
//!
//! Only the third layer is read from the live world, and the plan's hard rule governs it:
//!
//! > AutoPlay only observes the engine's dynamic state; it never simulates ammunition,
//! > reloading, passengers or weapon switching itself.
//!
//! That rule is why this module has no arithmetic. It could compute "rounds divided by rate
//! equals time to empty" and it would be wrong the first time the engine's reload rule had a
//! subtlety, because a second calculation of the same thing is a second owner. Every field here
//! is a read.
//!
//! What is not available, and is named rather than guessed: whether a unit is paralysed, EMP'd
//! or otherwise disabled. The engine has no query for those today - `disable_weapons` exists as
//! a `pub(crate)` field inside the Phobos multiplier struct, which is not a general answer. A
//! planner that needs it must get it added to the engine first, and until then it must not
//! assume a unit is free to act just because this struct is silent.
//!
//! Mind control, by contrast, *is* readable and was wrongly assumed unavailable once already:
//! `TechnoClass+0x2C0` is exposed through `MindControlLink::controller()`. It is included because
//! it is the case that defeats ownership checks - a mind-controlled unit keeps its original
//! owner and would pass `eligibility` while obeying somebody else.

use vera20k_core::sim::combat::TargetKind;
use vera20k_core::sim::world::Simulation;

use crate::world::Object;

/// Whether a unit can act right now, as far as the engine will say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    /// Rounds left, for a type that has ammunition.
    ///
    /// `None` for a type with no `Ammo=` at all - a structure, an infantryman - which is
    /// different from zero. Zero means it is out and must reload or return; `None` means the
    /// question does not apply.
    pub ammo: Option<i32>,
    /// Whether it is riding inside another unit.
    ///
    /// Read rather than inferred: a passenger is present in the entity store and has a
    /// position, so every other field would look normal while it cannot actually act.
    pub inside_transport: Option<u64>,
    /// Whether it is deployed, for a type that can deploy.
    pub deployed: bool,
    /// The target it has been given, if any.
    pub target: Option<TargetKind>,
    /// How many passengers it carries, for a transport.
    pub cargo: Option<u32>,
    /// The capacity of its hold, for a transport.
    pub cargo_capacity: Option<u32>,
    /// Who controls it, when that is not its owner.
    ///
    /// Mind control is the case that makes ownership alone insufficient: a unit keeps its
    /// owner and looks normal in every field here while obeying somebody else. Read from
    /// `TechnoClass+0x2C0` rather than inferred, because inferring it would mean recomputing
    /// the engine's control rules.
    pub mind_controlled_by: Option<u64>,
}

impl Availability {
    /// Whether it has ammunition left, treating "has none at all" as available.
    ///
    /// A rifleman with no `Ammo=` is not out of ammunition, and a planner that read `None` as
    /// zero would refuse to use every infantry unit.
    pub fn has_ammo(&self) -> bool {
        self.ammo.is_none_or(|rounds| rounds > 0)
    }

    /// Whether it could be ordered to move or fight at all.
    ///
    /// Does not cover being paralysed or disabled, because the engine has no query for those
    /// today - see the module doc. The name says "not obviously blocked" rather than "able",
    /// so a caller is not misled into thinking this is the whole answer.
    pub fn is_not_obviously_blocked(&self) -> bool {
        self.inside_transport.is_none() && self.has_ammo() && !self.is_mind_controlled()
    }

    /// Whether somebody other than the owner is driving it.
    ///
    /// The case ownership checks miss: a mind-controlled unit still reports its original
    /// owner, so [`crate::eligibility`] would call it eligible while it obeys somebody else.
    /// Anything that commands units has to consult this too.
    pub fn is_mind_controlled(&self) -> bool {
        self.mind_controlled_by.is_some()
    }

    /// Whether it is a transport carrying something.
    pub fn is_laden(&self) -> bool {
        self.cargo.is_some_and(|count| count > 0)
    }
}

/// Read one object's availability from the live world.
///
/// Returns `None` when the object is gone, which is an answer rather than an error: a plan
/// that names a destroyed unit is out of date.
pub fn availability_of(sim: &Simulation, id: u64) -> Option<Availability> {
    let entity = sim.entities().get(id)?;
    // `ammo()` returns `None` for a type with no ammunition, which this preserves rather than
    // flattening to zero: the distinction between "out" and "does not apply" decides whether a
    // planner waits or picks a different unit.
    let ammo = entity.ammo().map(|tracker| tracker.current());
    let (cargo, cargo_capacity) = match entity.passenger_role.cargo() {
        Some(hold) => (Some(hold.count()), Some(hold.capacity())),
        None => (None, None),
    };
    Some(Availability {
        ammo,
        inside_transport: entity
            .passenger_role
            .inside_transport()
            .map(|(transport, _open_topped)| transport),
        deployed: entity.is_deployed(),
        target: entity.archive_target(),
        cargo,
        cargo_capacity,
        mind_controlled_by: entity.mind_control.controller(),
    })
}

/// A whole snapshot's worth of availability, keyed the same way as the snapshot.
///
/// Built on demand rather than stored on [`Object`]: most decisions need the world shape, and
/// only some need this. Bundling it into every snapshot would make every snapshot pay for
/// fields most callers never read.
pub fn availability_map(
    sim: &Simulation,
    snapshot: &crate::world::Snapshot,
) -> std::collections::BTreeMap<u64, Availability> {
    snapshot
        .objects
        .values()
        .filter_map(|object: &Object| availability_of(sim, object.id).map(|a| (object.id, a)))
        .collect()
}
