//! Production, loss and asset, kept apart on purpose.
//!
//! The plan asks for five ledgers: production, loss, combat, income and asset. A [`Snapshot`]
//! can answer the first, second and fifth today. Combat needs damage attribution and income
//! needs a house's credit balance, neither of which the snapshot carries, so those two stay
//! open in the plan rather than being filled with a guess a reader could not tell from a real
//! reading. Two rules shape the whole design and both come from mistakes that are easy to make
//! and hard to notice.
//!
//! **Flow versus stock.** "Produced 30 tanks" and "has 30 tanks" are different numbers and a
//! ledger that stores one cannot answer the other. Production is a flow; what a house owns is a
//! stock. Mixing them produces the classic report where a house looks rich because it built a
//! lot and lost most of it.
//!
//! **No double counting.** The review gave two concrete cases. An engineer capturing a
//! building is *one ownership transfer*: it is not us building it, and it is not the enemy
//! destroying it. A unit that is mind-controlled is a *loss and a control change*, recorded
//! separately, and getting it back is not re-production. Recording either as ordinary
//! production would inflate a house's output for something it never made.
//!
//! ## Why the ledger is built by comparing snapshots rather than by consuming events
//!
//! The engine's `LifecycleOutput` stream is rich but not a ledger. Reading it to find deaths
//! turned up `ObjectPointerExpired`, which is a Techno *pointer* expiring - a navigation
//! cleanup that fires regardless of whether the unit is alive - and the only capture event is
//! `ConstructionYardCaptured`, specific to one building. An absence in that stream is not
//! evidence of absence, and counting on it would miss ordinary deaths while counting things
//! that never happened.
//!
//! So the authority here is the world itself: every ledger is derived by comparing what a house
//! owned at the previous observation with what it owns now. That is slower than consuming
//! events and it is correct for every cause - destroyed, captured, mind-controlled, sold,
//! teleported out of the map - because it looks at the result rather than at the mechanism.
//! Events can refine *why* later without changing the *what*.
//!
//! The cost is a convention worth stating: an object that disappears and returns between two
//! observations is missed. Observations are therefore taken often enough for that not to
//! matter, and the alternative - trusting a stream that does not carry deaths - misses far more.

use std::collections::{BTreeMap, BTreeSet};

use vera20k_core::sim::intern::InternedId;

use crate::world::Snapshot;

/// What a house owns at one moment. The stock side of every ledger.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holding {
    /// How many objects of each type, keyed by upper-case type name.
    pub by_type: BTreeMap<String, usize>,
    /// Every object id, so a later comparison can tell a loss from a replacement.
    pub ids: BTreeSet<u64>,
    /// The type of each held id, so a destroyed object can still be named after its row is
    /// gone from the world. Without this a loss report would have to say "unknown", which is
    /// useless exactly when a loss matters.
    pub types_by_id: BTreeMap<u64, String>,
}

impl Holding {
    /// Read a snapshot into a holding for one house.
    pub fn of(snapshot: &Snapshot, house: InternedId) -> Self {
        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        let mut ids = BTreeSet::new();
        let mut types_by_id = BTreeMap::new();
        for object in snapshot.objects.values().filter(|o| o.owner == house) {
            *by_type.entry(object.type_name.clone()).or_insert(0) += 1;
            ids.insert(object.id);
            types_by_id.insert(object.id, object.type_name.clone());
        }
        Self {
            by_type,
            ids,
            types_by_id,
        }
    }

    /// How many objects of `type_name` this holding has.
    pub fn count_of(&self, type_name: &str) -> usize {
        self.by_type.get(type_name).copied().unwrap_or(0)
    }

    /// Everything, in total.
    pub fn total(&self) -> usize {
        self.ids.len()
    }
}

/// One object that left a house's ownership, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departure {
    /// Which object.
    pub id: u64,
    /// Its type, so a report can say what was lost without looking anything up.
    pub type_name: String,
    /// Who holds it now, when the answer is "somebody else".
    ///
    /// `None` means it is gone from the world entirely. The difference is the whole reason
    /// capture and death are recorded apart: a captured building is still standing.
    pub now_owned_by: Option<InternedId>,
}

impl Departure {
    /// Whether the object still exists under another house.
    pub fn was_transferred(&self) -> bool {
        self.now_owned_by.is_some()
    }

    /// Whether it left the world.
    pub fn was_destroyed(&self) -> bool {
        self.now_owned_by.is_none()
    }
}

/// What changed for one house between two observations.
///
/// Every field is a flow - a count of things that happened in the interval - except the
/// holdings themselves, which are the stock at the end. Keeping them in one struct makes the
/// distinction visible at the point of use rather than in a comment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Movement {
    /// Objects this house gained and did not have before.
    ///
    /// A captured building appears here as well as in `transferred_in`, and the two are
    /// deliberately not merged: `gained` answers "what do I have now that I did not", while
    /// `transferred_in` answers "what changed hands". An engineer's capture is one event and
    /// must not be counted as production, so a caller reporting production reads
    /// `gained` minus `transferred_in` - and the data to do that subtraction is present rather
    /// than lost.
    pub gained: Vec<(String, usize)>,
    /// Objects taken from this house by another.
    ///
    /// The loss ledger's transfer side. Not a kill, and not counted as one.
    pub transferred_out: Vec<Departure>,
    /// Objects taken into this house from another.
    pub transferred_in: Vec<Departure>,
    /// Objects that left the world, which is the loss ledger's destruction side.
    pub destroyed: Vec<Departure>,
    /// The stock after the interval.
    pub holding: Holding,
}

impl Movement {
    /// Objects newly built, excluding anything that merely changed hands.
    ///
    /// This is the function that keeps an engineer's capture out of the production count. The
    /// plan states the rule; this is where it is enforced.
    pub fn produced(&self) -> Vec<(String, usize)> {
        let mut transferred: BTreeMap<&str, usize> = BTreeMap::new();
        for departure in &self.transferred_in {
            *transferred.entry(departure.type_name.as_str()).or_insert(0) += 1;
        }
        self.gained
            .iter()
            .filter_map(|(name, count)| {
                let borrowed = transferred.get(name.as_str()).copied().unwrap_or(0);
                count
                    .checked_sub(borrowed)
                    .filter(|&left| left > 0)
                    .map(|left| (name.clone(), left))
            })
            .collect()
    }

    /// Whether anything at all moved.
    pub fn is_empty(&self) -> bool {
        self.gained.is_empty()
            && self.transferred_out.is_empty()
            && self.transferred_in.is_empty()
            && self.destroyed.is_empty()
    }
}

/// Compare two observations of one house and report what moved.
///
/// `previous` may be `None` for the first observation, in which case everything currently held
/// is reported as the starting stock and nothing is reported as produced - a match does not
/// begin by building its own starting units, and counting them would make the first report
/// wildly wrong.
///
/// A bare snapshot cannot tell a capture from a build: the object's previous owner is not in
/// it, so [`Ledger::observe`] fills `transferred_in` from the owner history it keeps.
/// Departures are complete here, because the current world says whether a departed id still
/// exists under another house or left entirely. Keeping that split is why there is no
/// `owner_before` here: a function that always returned `None` with a comment explaining why
/// would be worse than not having one.
pub fn compare(previous: Option<&Holding>, now: &Snapshot, house: InternedId) -> Movement {
    let current = Holding::of(now, house);
    let empty = Holding::default();
    let before = previous.unwrap_or(&empty);

    // Gains are counted by identity, not by comparing stock totals. "Built one, lost one"
    // leaves a type's count unchanged but is still one produced; a stock difference would
    // report nothing, which is the flow/stock mistake this ledger exists to avoid.
    let mut gained: BTreeMap<String, usize> = BTreeMap::new();
    for id in &current.ids {
        if before.ids.contains(id) {
            continue;
        }
        let name = current.id_type(*id).unwrap_or("unknown");
        *gained.entry(name.to_string()).or_insert(0) += 1;
    }

    // Anything held before and gone now is a loss; the snapshot says whether it still exists
    // under another house or left the world entirely. Naming the type comes from the *previous*
    // holding, because the row may already be gone and the current world cannot name it.
    let mut transferred_out = Vec::new();
    let mut destroyed = Vec::new();
    for id in &before.ids {
        if current.ids.contains(id) {
            continue;
        }
        match now.objects.get(id) {
            Some(object) => transferred_out.push(Departure {
                id: *id,
                type_name: object.type_name.clone(),
                now_owned_by: Some(object.owner),
            }),
            None => destroyed.push(Departure {
                id: *id,
                type_name: before.id_type(*id).unwrap_or("unknown").to_string(),
                now_owned_by: None,
            }),
        }
    }
    let transferred_in = Vec::new();

    Movement {
        gained: gained.into_iter().collect(),
        transferred_out,
        transferred_in,
        destroyed,
        holding: current,
    }
}

impl Holding {
    /// The recorded type of an id, for naming a destroyed object whose row is gone from the
    /// world. Returns `None` only for an id this holding never recorded, which would be a
    /// programming error rather than a real case.
    pub fn id_type(&self, id: u64) -> Option<&str> {
        self.types_by_id.get(&id).map(String::as_str)
    }
}

/// The ledgers for one match, held across observations.
///
/// Belongs to the match, like the trace and the dispatcher: nothing survives into the next one.
#[derive(Debug, Default)]
pub struct Ledger {
    /// The last observation of each house, so the next comparison has a "before".
    holdings: BTreeMap<InternedId, Holding>,
    /// The last known owner of every object, which is what turns a capture into a transfer
    /// rather than a disappearance plus an appearance.
    owners: BTreeMap<u64, InternedId>,
    /// Total objects produced, by type, across the whole match. A flow that only accumulates.
    produced: BTreeMap<String, usize>,
    /// Total objects destroyed, by type.
    destroyed: BTreeMap<String, usize>,
    /// Total objects transferred between houses, by type.
    transferred: BTreeMap<String, usize>,
    /// How many observations have been taken.
    observations: usize,
}

impl Ledger {
    /// A new match's ledger, with nothing recorded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one observation and fold it into the running totals.
    ///
    /// Returns the movement for every house the snapshot describes, so a caller can report per
    /// house without asking twice.
    pub fn observe(&mut self, snapshot: &Snapshot) -> BTreeMap<InternedId, Movement> {
        let mut out = BTreeMap::new();
        // Who owns what right now, which becomes the "before" for the next call.
        let current_owners: BTreeMap<u64, InternedId> = snapshot
            .objects
            .values()
            .map(|object| (object.id, object.owner))
            .collect();

        let mut houses: BTreeSet<InternedId> = snapshot
            .objects
            .values()
            .map(|object| object.owner)
            .collect();
        // A house that lost everything still has to be reported, or its loss would vanish
        // exactly when it matters most.
        houses.extend(self.holdings.keys().copied());

        for house in houses {
            let before = self.holdings.get(&house).cloned();
            let current = Holding::of(snapshot, house);
            let mut movement = compare(before.as_ref(), snapshot, house);

            // `compare` already named every departure from the snapshot. Only a transfer
            // *in* needs the owner history a bare snapshot cannot supply: an id new to this
            // house that the ledger last saw under somebody else changed hands, while one it
            // never saw was built.
            for id in &current.ids {
                if before.as_ref().is_some_and(|b| b.ids.contains(id)) {
                    continue;
                }
                match self.owners.get(id) {
                    // The object existed before and belonged to somebody else.
                    Some(previous) if *previous != house => {
                        let type_name = snapshot
                            .objects
                            .get(id)
                            .map(|o| o.type_name.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        movement.transferred_in.push(Departure {
                            id: *id,
                            type_name,
                            now_owned_by: Some(*previous),
                        });
                    }
                    _ => {}
                }
            }

            // The first observation is a baseline, not a measurement. A match does not begin
            // by building its starting units, so nothing may be counted as produced simply
            // because it is new to the ledger. Without this the opening report would claim a
            // house built everything it starts with, which is the flow/stock mistake at its
            // most basic.
            if before.is_none() {
                self.holdings.insert(house, current);
                continue;
            }

            if movement.is_empty() {
                self.holdings.insert(house, current);
                continue;
            }

            for (name, count) in movement.produced() {
                *self.produced.entry(name).or_insert(0) += count;
            }
            for departure in &movement.destroyed {
                *self
                    .destroyed
                    .entry(departure.type_name.clone())
                    .or_insert(0) += 1;
            }
            for departure in &movement.transferred_in {
                *self
                    .transferred
                    .entry(departure.type_name.clone())
                    .or_insert(0) += 1;
            }

            self.holdings.insert(house, current);
            out.insert(house, movement);
        }

        self.owners = current_owners;
        self.observations += 1;
        out
    }

    /// Total produced, by type, across the match.
    pub fn produced(&self) -> &BTreeMap<String, usize> {
        &self.produced
    }

    /// Total destroyed, by type.
    pub fn destroyed(&self) -> &BTreeMap<String, usize> {
        &self.destroyed
    }

    /// Total transferred between houses, by type.
    pub fn transferred(&self) -> &BTreeMap<String, usize> {
        &self.transferred
    }

    /// How many observations have been taken.
    pub fn observations(&self) -> usize {
        self.observations
    }

    /// The last stock for one house.
    pub fn holding(&self, house: InternedId) -> Option<&Holding> {
        self.holdings.get(&house)
    }
}
