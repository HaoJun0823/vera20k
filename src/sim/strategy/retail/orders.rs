//! The army's direct orders, for the units no team has taken: an idle combat
//! unit goes on area guard, an engineer walks into the bridge hut whose span is
//! down or into a building it may capture, a spy walks into the building worth
//! most, and an `Occupier=` rifleman garrisons a building near the base.
//!
//! Each is a click the player already has, issued as the same command: the
//! Capture click (engineers, spies, bridge huts) and the Enter click (garrisons).
//! Nothing here decides what a click *does* — `InfantryClass::PerCellProcess`,
//! `BuildingClass::CanBeOccupiedBy` and the entry queries own that.
//!
//! Native teams carry the attack. An `AITrigger` creates a team, its script
//! assigns missions, and `team_script_vm` writes them onto the members. Those
//! missions have no player equivalent, and a team's script already runs for a
//! brain-driven house, so this module only covers **the units no team has
//! taken**: an idle one goes on area guard where it stands, and the sim's own
//! Guard mission decides what it engages and where it returns to. That is the
//! native posture for a unit no team has taken — it sits, it does not charge,
//! and it is the sim, not this module, that picks its fight.
//!
//! Sending every idle unit at the nearest hostile instead is what a play test
//! loses with: the opening squad walks into the enemy's own pre-placed force
//! and dies, the plan loses the units it was going to recruit, and the base
//! keeps spending on replacements it cannot afford.
//!
//! That leaves the offensive half to the teams, which is where the retail AI
//! puts it: a hostile the enemy owns is what an `AITrigger` needs, and the
//! enemy search that sets it runs in the house's own Strategy tick
//! (`sim::house_strategy`), which the brain asks for.
//!
//! Every order here passes through [`super::tracked`], which remembers what
//! each unit has been told and refuses to say it again until the intent has
//! expired, has changed, or its target has moved. That is what keeps a toggle
//! from undoing itself and a walk-in from being restarted every frame — and
//! what keeps the whole per-frame budget from going on one army's standing
//! orders.

use std::collections::BTreeSet;

use crate::rules::object_type::{ObjectCategory, ObjectType};
use crate::rules::ruleset::RuleSet;
use crate::rules::superweapon_type::SuperWeaponKind;
use crate::sim::control::{Goal, MicroKind, MicroParams};
use crate::sim::control::brain::Outbox;
use crate::sim::control::intent::Intent;
use crate::sim::command::Command;
use crate::sim::control::observation::{EnemyEntity, Observation};
use crate::sim::mission::{MissionId, MissionType};
use crate::sim::strategy::retail::RetailBrain;

use super::squared_distance;
use super::tracked::TrackedKind;

/// How many new unit orders one frame may carry. The board drops everything
/// past its own per-frame budget, so asking for more would only spend the frame
/// on orders that never leave; this keeps the surviving orders to the newest
/// units first, in storage order.
const FORCE_ORDERS_PER_FRAME: usize = 8;

/// The intents that mean a unit is already on its way somewhere: the advance
/// step owns those, and a guard here would pull it back out of the push.
const PUSHING: &[TrackedKind] = &[TrackedKind::Attack, TrackedKind::AttackMove, TrackedKind::Move];

/// How near one of the house's own buildings a bridge or a garrison has to be
/// for the policy to spend a unit on it, in cells. A garrison is only worth
/// holding where the base is, and a bridge nobody crosses is an engineer spent
/// for nothing.
const HOME_REACH_CELLS: i32 = 12;

// Why there is no boarding step here. Wiring one — an idle rifleman climbing
// into his own empty carrier, a correct Enter click (`Goal::Enter` and
// `can_enter_transport` both answered as expected) — cost the reference match
// nine thousand ticks: `won at 25636` became `won at 34897`, and with the army's
// strength count corrected to skip units the gate would refuse, the match
// stopped winning inside its 36000-tick cap. Whether the carrier loses value
// once loaded or the boarded unit is simply not where the push needs it is not
// established, so the step stays out and recorded rather than guessed at.

/// Every unit that walks into an enemy building takes one it may: the Capture
/// click, which an engineer's capture and a spy's infiltration both are.
///
/// Natively the capture mission starts from the same order path the player's
/// click uses; a brain-driven house never receives that click, so the brain
/// issues the order. The two units differ only in what the building's own
/// per-cell handler does when they arrive — `InfantryClass::PerCellProcess`
/// captures it for an `Engineer=` and infiltrates it for an `Infiltrate=` — so
/// one order serves both, and neither unit would otherwise do anything: a spy
/// carries no weapon worth ranking and an engineer carries none at all.
///
/// What they walk at differs, because the click's own admission does: a spy
/// needs `Spyable=` (see [`Observation::spyable_buildings`]) while an engineer
/// needs `Capturable=`. And a spy's target is picked by what the building is
/// worth, not by what is nearest — walking the house's only spy into the
/// derrick next door is how the feature is wasted.
pub(super) fn captures(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    if !brain
        .census
        .units
        .iter()
        .any(|unit| walks_into_buildings_flags(&unit.traits))
    {
        return;
    }
    let engineer_targets: Vec<EnemyEntity> = obs.capturable_buildings(rules).collect();
    // Which of those belong to an enemy, and where that enemy's units stand.
    // `capturable_buildings` is every `Capturable=` building the house can see —
    // the map's own oil derricks and hospitals included — and the nearest of
    // them was how an engineer walked straight into a base: the derrick inside
    // the enemy's yard is near, and the engineer that goes for it is a 500-credit
    // gift. Buildings do not guard anything, so only units count as pressure.
    let hostile: BTreeSet<u64> = obs
        .enemy_entities()
        .map(|entity| entity.entity_id)
        .collect();
    let guards: Vec<(u16, u16)> = obs
        .enemy_entities()
        .filter(|entity| {
            obs.object_type(rules, entity.type_id)
                .is_some_and(|ty| ty.category != ObjectCategory::Building)
        })
        .map(|entity| entity.cell)
        .collect();
    let spy_targets: Vec<(EnemyEntity, u8)> = obs
        .spyable_buildings(rules)
        .filter_map(|target| {
            let ty = obs.object_type(rules, target.type_id)?;
            Some((target, infiltration_value(rules, ty)))
        })
        .collect();
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.orderable || unit.hand || ordered_this_frame(out, unit.entity_id) {
            continue;
        }
        let Some(ty) = obs.object_type(rules, unit.type_id) else {
            continue;
        };
        if !walks_into_buildings(rules, ty) {
            continue;
        }
        // A spy takes what is worth most; anyone else takes what is nearest.
        let chosen = if ty.agent {
            spy_targets
                .iter()
                .max_by_key(|(target, value)| (*value, -squared_distance(unit.cell, target.cell)))
                .map(|(target, _)| *target)
        } else {
            // One building takes one engineer. Without this both were sent at
            // the same derrick, the first took it, and the second arrived at a
            // building that was already ours and stood there.
            let free: Vec<EnemyEntity> = engineer_targets
                .iter()
                .copied()
                .filter(|target| {
                    !brain.tracked.claimed_by_other(
                        tick,
                        TrackedKind::Capture,
                        target.entity_id,
                        unit.entity_id,
                    )
                })
                .collect();
            engineer_preference(&free, &hostile, &guards, unit.cell)
        };
        let Some(target) = chosen else {
            continue;
        };
        // L4: it has been told to take this building, and the walk is not
        // over, so it is not told again.
        if !brain.tracked.admits(
            tick,
            unit.entity_id,
            TrackedKind::Capture,
            Some(target.entity_id),
            Some(target.cell),
        ) {
            continue;
        }
        brain.tracked.record(
            tick,
            unit.entity_id,
            TrackedKind::Capture,
            Some(target.entity_id),
            Some(target.cell),
        );
        out.order(
            unit.entity_id,
            Goal::Capture {
                target: target.entity_id,
            },
        );
        issued += 1;
    }
}

/// Send an engineer into a `BridgeRepairHut=` whose span is down.
///
/// Walking in is the only thing that rebuilds a bridge: the hut carries no
/// timer and no `Capturable=`, and `BridgeRepairHut` is retail's only consumer
/// ([`Observation::bridge_repair_huts`]). The engineer is consumed by it, which
/// is also retail's price, so this only pays for a bridge inside the house's
/// own reach.
pub(super) fn bridges(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    if !brain.census.units.iter().any(|unit| unit.traits.engineer) {
        return;
    }
    let huts: Vec<(u64, (u16, u16))> = obs
        .bridge_repair_huts(rules)
        .filter(|site| within(home_cells(brain), site.cell, HOME_REACH_CELLS))
        .map(|site| (site.building_id, site.cell))
        .collect();
    if huts.is_empty() {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        let Some(ty) = obs.object_type(rules, unit.type_id) else {
            continue;
        };
        if !ty.engineer
            || !unit.orderable
            || unit.hand
            || ordered_this_frame(out, unit.entity_id)
            || holds_or_fights(unit.mission)
        {
            continue;
        }
        let Some(target) = nearest_site(&huts, unit.cell) else {
            continue;
        };
        if !brain.tracked.admits(
            tick,
            unit.entity_id,
            TrackedKind::Capture,
            Some(target),
            None,
        ) {
            continue;
        }
        brain
            .tracked
            .record(tick, unit.entity_id, TrackedKind::Capture, Some(target), None);
        out.order(unit.entity_id, Goal::Capture { target });
        issued += 1;
    }
}

/// Idle `Occupier=` infantry garrison a building near the house's own base.
///
/// `BuildingClass::CanBeOccupiedBy` decides whether this infantryman may walk
/// in, so a full building, a red one and a stranger's are all refused by the
/// owner rather than by a second rule here.
pub(super) fn garrison(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    // Where the army stands counts as home for this. A soldier walking into a
    // building on the way is how a push holds the ground it takes, and a
    // garrison that only ever answers inside the base is a garrison that never
    // leaves it — the base fills up and the army walks past every building it
    // could have held the street from.
    let anchors: Vec<(u16, u16)> = home_cells(brain)
        .chain(
            brain
                .census
                .units
                .iter()
                .filter(|unit| unit.traits.combat)
                .map(|unit| unit.cell),
        )
        .collect();
    let billets: Vec<_> = obs
        .occupiable_buildings(rules)
        .filter(|billet| within(anchors.iter().copied(), billet.cell, HOME_REACH_CELLS))
        .collect();
    if billets.is_empty() {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.traits.occupier
            || !unit.orderable
            || unit.hand
            || holds_or_fights(unit.mission)
            || ordered_this_frame(out, unit.entity_id)
        {
            continue;
        }
        let Some(billet) = billets
            .iter()
            .filter(|billet| squared_distance(unit.cell, billet.cell) <= HOME_REACH_CELLS.pow(2))
            .filter(|billet| obs.can_garrison(rules, unit.entity_id, billet.building_id))
            .min_by_key(|billet| squared_distance(unit.cell, billet.cell))
        else {
            continue;
        };
        // The garrison is the one that had no memory at all: it was told to
        // walk into the same building every frame, and a unit being told to
        // enter what it is already entering is a unit that never arrives.
        if !brain.tracked.admits(
            tick,
            unit.entity_id,
            TrackedKind::Enter,
            Some(billet.building_id),
            Some(billet.cell),
        ) {
            continue;
        }
        brain.tracked.record(
            tick,
            unit.entity_id,
            TrackedKind::Enter,
            Some(billet.building_id),
            Some(billet.cell),
        );
        out.order(
            unit.entity_id,
            Goal::Enter {
                target: billet.building_id,
            },
        );
        issued += 1;
    }
}

/// What a spy gets out of a building, in retail's own categories.
///
/// The order follows `AIMD.INI`'s own spy teams — `General Spy vs Soviet
/// Power` / `vs Allied Power` before `vs Allied War` / `vs Soviet War`, and
/// `vs Sov Tech` after them (`ini/aimd.ini:1058-1158`) — and the categories are
/// the rules' `BuildCat=` lists, so a mod that moves a building keeps its
/// weight. A power plant blacks the enemy out, a refinery hands over its ore
/// and a radar its map; a barracks or a war factory makes its army veteran; a
/// lab is worth the walk and little more.
fn infiltration_value(rules: &RuleSet, ty: &ObjectType) -> u8 {
    if rules.build_power_types.iter().any(|name| *name == ty.id) {
        4
    } else if ty.resource_destination || ty.radar {
        3
    } else if rules.build_weapons_types.iter().any(|name| *name == ty.id)
        || rules.build_barracks_types.iter().any(|name| *name == ty.id)
    {
        2
    } else {
        1
    }
}

/// The cells of the house's own buildings: the yard, the plants, the factories,
/// the defenses. A bridge or a garrison is only worth a unit near one of them.
fn home_cells(brain: &RetailBrain) -> impl Iterator<Item = (u16, u16)> + '_ {
    brain.census.home_cells.iter().copied()
}

/// Whether a unit's type is one the Capture click reaches: an engineer's by
/// `Engineer=` and a spy's by `Infiltrate=`.
fn walks_into_buildings_flags(traits: &super::census::Traits) -> bool {
    traits.engineer || traits.infiltrator
}

/// Whether any of `cells` is within `reach` cells (Chebyshev-independent, all
/// orders in this file use squared Euclidean distance) of `cell`.
fn within(cells: impl Iterator<Item = (u16, u16)>, cell: (u16, u16), reach: i32) -> bool {
    cells
        .into_iter()
        .any(|other| squared_distance(cell, other) <= reach * reach)
}

/// How near an enemy unit has to stand to a capture target for the walk to look
/// like a gift rather than a job, in cells: the range of the weapons that shoot
/// infantry, near enough.
const CAPTURE_CLEAR_CELLS: i32 = 6;

/// Which building an engineer should walk into.
///
/// In the order a player weighs it: the map's own building before the enemy's
/// (an oil derrick is income with no defence and no owner to answer), an
/// unguarded one before a guarded one (a 500-credit engineer is not worth a
/// derrick it dies before reaching), and the nearer before the further. Ties by
/// id, so the answer does not depend on the order the entity store yields.
fn engineer_preference(
    targets: &[EnemyEntity],
    hostile: &BTreeSet<u64>,
    guards: &[(u16, u16)],
    from: (u16, u16),
) -> Option<EnemyEntity> {
    let clear = CAPTURE_CLEAR_CELLS * CAPTURE_CLEAR_CELLS;
    targets
        .iter()
        .map(|target| {
            let owned = u8::from(hostile.contains(&target.entity_id));
            let guarded =
                u8::from(guards.iter().any(|cell| squared_distance(*cell, target.cell) <= clear));
            (
                (owned, guarded, squared_distance(from, target.cell), target.entity_id),
                *target,
            )
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, target)| target)
}

/// The nearest of `(id, cell)` sites, ties broken by id so the answer does not
/// depend on the order the entity store happens to yield.
fn nearest_site(sites: &[(u64, (u16, u16))], from: (u16, u16)) -> Option<u64> {
    sites
        .iter()
        .min_by_key(|(id, cell)| (squared_distance(from, *cell), *id))
        .map(|(id, _)| *id)
}

/// Whether an earlier step already gave this unit an order this frame. The
/// steps run in priority order — a bridge before a capture, a capture before a
/// guard — and a unit is one unit: the first answer wins.
fn ordered_this_frame(out: &Outbox, actor: u64) -> bool {
    out.orders.iter().any(|order| order.actor == actor)
}

/// Put every idle combat unit no team has taken on area guard where it stands.
pub(super) fn forces(
    brain: &mut RetailBrain,
    _rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let tick = obs.tick();
    // Forget the units the house no longer has, so the memory stays bounded by
    // the army and a recycled id cannot inherit an old order.
    let ids: BTreeSet<u64> = brain.census.units.iter().map(|unit| unit.entity_id).collect();
    brain.tracked.retain(&ids);
    if !brain.census.units.iter().any(|unit| unit.traits.combat) {
        return;
    }
    let mut issued = 0;
    for unit in &brain.census.units {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        if !unit.traits.combat {
            continue;
        }
        // A team's script or one of the player's control groups owns its
        // members; this step is for the units no hand has taken.
        if unit.hand {
            continue;
        }
        // A unit that is already holding or fighting is doing exactly this, and
        // its mission owns it: a second Guard would cancel the fight.
        if holds_or_fights(unit.mission) {
            continue;
        }
        // The advance step owns a unit it is pushing: a guard here would pull
        // it back out of the push.
        if brain.tracked.stands(tick, unit.entity_id, PUSHING) {
            continue;
        }
        if !brain
            .tracked
            .admits(tick, unit.entity_id, TrackedKind::Guard, None, None)
        {
            continue;
        }
        brain
            .tracked
            .record(tick, unit.entity_id, TrackedKind::Guard, None, None);
        out.order(unit.entity_id, Goal::Guard);
        issued += 1;
    }
}

/// How near a hostile has to be before a soldier takes its deployed position,
/// and how far it has to be before it gives the position up, in cells.
///
/// Deploying is a weapon change and a movement stop — `DeployFire=` swaps the
/// primary for the deployed one — so it is worth taking when the fight has
/// arrived, not when it is a rumour across the map.
///
/// The two numbers are deliberately different. One radius for both made the
/// soldier sit and stand at almost the same distance, and `6` sat right on top
/// of the reach the deployed gun covers, so a hostile pacing between five and
/// seven cells had it kneeling and rising forever — and a soldier mid-animation
/// is a soldier with its gun silent, which is why a dug-in position sometimes
/// shot and sometimes did not. The gap is the dead band: nothing inside
/// [`DEPLOY_RANGE_CELLS`] changes the pose until it is outside
/// [`UNDEPLOY_RANGE_CELLS`].
const DEPLOY_RANGE_CELLS: i32 = 6;
const UNDEPLOY_RANGE_CELLS: i32 = 10;

/// How many toggles one frame carries. The D key is a click like any other,
/// and a frame that spends its budget on soldiers standing up has none left
/// for the orders that move them.
const DEPLOYS_PER_FRAME: usize = 4;

/// How long a soldier is left alone after the D key is [`TrackedKind::term`]'s
/// answer now: the deploy animation, and the command's round trip.

/// The D key on the soldiers that fight better deployed: `DeployFire=yes`.
///
/// `[E1]` and `[GGI]` carry it with `Deployer=yes`, and what it buys is the
/// deployed weapon — a GI's sandbag posture, a Guardian GI's missiles — at the
/// price of not moving. A player presses D when the enemy is in front of the
/// soldier, which is what the range above is.
///
/// The command toggles (`None → Deploying`, `Deployed → Undeploying`, and a
/// no-op mid-transition), so it is only issued on the edge: a soldier that is
/// already down stays down, and one that is standing stays up. Issuing it
/// every frame would walk them in and out of the pose, and a deployed soldier
/// that never stands up is a soldier that never takes ground.
pub(super) fn deploy(brain: &mut RetailBrain, obs: &Observation<'_>, out: &mut Outbox) {
    let tick = obs.tick();
    // Forget the soldiers the house no longer has, so a recycled id cannot
    // inherit another soldier's pose.
    let live: BTreeSet<u64> = brain.census.units.iter().map(|unit| unit.entity_id).collect();
    brain.tracked.retain(&live);
    // Collected before anything is written: the toggle has to be recorded
    // against the soldier, and the ledger is borrowed while it is read.
    let wanted: Vec<(u64, i32)> = brain
        .census
        .units
        .iter()
        .filter(|unit| unit.traits.deployer && !unit.hand && unit.orderable)
        .map(|unit| (unit.entity_id, nearest_distance(&brain.census.enemies, unit.cell)))
        .collect();
    let mut issued = 0;
    for (entity_id, distance) in wanted {
        if issued >= DEPLOYS_PER_FRAME {
            break;
        }
        let deployed = obs.is_deployed(entity_id);
        let Some(down) = pose_to_take(deployed, distance) else {
            continue;
        };
        log::debug!(
            "auto play: deploy {} -> {} nearest={}",
            entity_id,
            if down { "dug in" } else { "standing" },
            distance
        );
        // The toggle's own term is the deploy animation, and `Is_Deployed`
        // only follows it — so until then the soldier still reads as standing
        // and would be toggled straight back up. L4 refuses the second press.
        if !brain
            .tracked
            .admits(tick, entity_id, TrackedKind::Deploy, None, None)
        {
            continue;
        }
        out.command(Command::ToggleInfantryDeploy { entity_id });
        brain
            .tracked
            .record(tick, entity_id, TrackedKind::Deploy, None, None);
        issued += 1;
    }
}

/// The pose the soldier should take, or `None` for "leave it as it is".
///
/// Down when the fight is here, up only once it is clearly gone, and nothing in
/// between changes anything: that gap is the dead band. One radius for both
/// made a hostile pacing between the two kneel and rise the soldier forever, and
/// a soldier mid-animation is a soldier whose gun is silent.
fn pose_to_take(deployed: bool, distance: i32) -> Option<bool> {
    let down = distance <= DEPLOY_RANGE_CELLS * DEPLOY_RANGE_CELLS;
    let up = distance > UNDEPLOY_RANGE_CELLS * UNDEPLOY_RANGE_CELLS;
    match (deployed, down, up) {
        (false, true, _) => Some(true),
        (true, _, true) => Some(false),
        _ => None,
    }
}

/// How far the nearest hostile is from `cell`, as a squared cell distance so
/// the square root is not needed; `i32::MAX` when there is none.
fn nearest_distance(enemies: &[EnemyEntity], cell: (u16, u16)) -> i32 {
    enemies
        .iter()
        .map(|enemy| squared_distance(enemy.cell, cell))
        .min()
        .unwrap_or(i32::MAX)
}

/// Whether the object is already walking or fighting, so re-tasking it would
/// cancel something.
///
/// `Move` is here because a unit the house is still walking somewhere is not
/// idle, and replacing that move with a guard would stop it half-way. `Attack`
/// is here because a second order would cancel the fight.
///
/// `AreaGuard` is deliberately **not** here, and used to be. Every unit a
/// scenario spawns for the player starts on area guard, and treating that as
/// "busy" left the whole opening force standing in the yard for the whole
/// match: the brain never gave it a job because the mission it was born with
/// said it had one. A guard is the posture to *replace*, not a reason to leave
/// a unit alone — the intent table ([`super::tracked`]) is what stops it being
/// re-issued every frame.
fn holds_or_fights(mission: MissionId) -> bool {
    [MissionType::Attack, MissionType::Move]
        .into_iter()
        .any(|kind| mission == MissionId::from_known(kind))
}

/// The health a building is worth the wrench at, in permille of its full
/// strength.
///
/// Below nine tenths: a house that waits for a red bar repairs the building
/// after it has already lost the fight it was holding, and a house that repairs
/// a scratch spends credits it cannot spare. The repair costs the damage, so
/// the threshold is where a scratch becomes a wound.
const REPAIR_HEALTH_PERMILLE: i32 = 900;

/// How long a building is left with the wrench the house has already switched
/// on, in frames: a minute (900 frames at 15 a second). A repair takes minutes,
/// so a click that never took is worth asking again after a quarter of one.
const REPAIR_REARM_FRAMES: u64 = 900;

/// Turn the wrench on the house's own damaged buildings.
///
/// `Command::ToggleRepair` is the Display REPAIR15 click
/// (`Display BandBox_LeftUp4AC0E7`): the building spends the house's credits to
/// heal over time and switches itself off at full strength. It is a toggle, so
/// asking twice turns a repair *off* — the brain therefore remembers the
/// buildings it has already switched on, and forgets one only when it is whole
/// again. Nothing about the click is per-frame, so nothing here is either.
pub(super) fn repairs(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    let mut issued = 0;
    let mut whole: Vec<u64> = Vec::new();
    let mut hurt: Vec<u64> = Vec::new();
    for entity in obs.own_entities() {
        let Some(ty) = obs.object_type(rules, entity.type_id) else {
            continue;
        };
        if ty.category != ObjectCategory::Building {
            continue;
        }
        let Some(full) = obs.max_health(rules, entity.type_id).filter(|full| *full > 0) else {
            continue;
        };
        let permille = i32::try_from(i64::from(entity.health.max(0)) * 1_000 / i64::from(full))
            .unwrap_or(0);
        if permille >= 1_000 {
            whole.push(entity.entity_id);
        } else if permille < REPAIR_HEALTH_PERMILLE && brain.repaired.contains(&entity.entity_id) {
            hurt.push(entity.entity_id);
        }
    }
    // A building is worth the wrench once it has stood whole: a structure still
    // going up is at a fraction of its strength by definition, and healing it
    // would spend the house's credits on the build it has already paid for.
    for entity_id in &whole {
        brain.repaired.insert(*entity_id);
        brain.repairing.remove(entity_id);
    }
    for entity_id in hurt {
        if issued >= FORCE_ORDERS_PER_FRAME {
            break;
        }
        // The memory has a term. A click that was refused — the budget, the
        // domain, the gate — leaves nothing behind, and a house that remembers
        // a repair it never started never asks again, which is why some
        // buildings healed and others never did.
        if brain
            .repairing
            .get(&entity_id)
            .is_some_and(|until| obs.tick() < *until)
        {
            continue;
        }
        brain
            .repairing
            .insert(entity_id, obs.tick().saturating_add(REPAIR_REARM_FRAMES));
        out.command(Command::ToggleRepair { entity_id });
        issued += 1;
    }
}

/// Whether a unit's click on an enemy building is a Capture click: the two
/// flags the per-cell handler branches on, an engineer's by `Engineer=` and a
/// spy's by `Infiltrate=` (which `Engineer=` also implies, per
/// `InfantryTypeClass::ReadINI` `0x0052466E..0x0052469A`).
pub(super) fn walks_into_buildings(rules: &RuleSet, ty: &ObjectType) -> bool {
    super::census::Traits::of(rules, ty).captures
}

/// The health fraction at which a fighting unit belongs to the retreat routine
/// rather than to the brain's own orders.
///
/// A unit the sim's own mission owns fights to the death, which is the wrong
/// trade for a house that pays for every replacement: a third of its strength
/// left is the point where a player walks it home. The routine hands the unit
/// back as soon as it is healthy again, so the threshold is the whole rule.
const RETREAT_HEALTH_PERMILLE: u16 = 300;

/// Hand the house's hurt fighters to the L1 retreat routine.
///
/// This is the one place the brain uses a managed routine. It asks through
/// `Outbox::attach` because the table of attached objects is L2's, and from
/// then on the routine — not this brain — is that object's issuer. Asking
/// again for a unit the table already holds is free: `MicroTable::attach`
/// refuses a second attach, so no bookkeeping is kept here and a routine that
/// hands an object back is picked up again the next frame its condition holds.
///
/// The post is the base's own anchor, so a unit that breaks off walks home
/// rather than freezing where the fight left it.
pub(super) fn managed(brain: &mut RetailBrain, tick: u64, out: &mut Outbox) {
    // The depot when the house has one, the base's anchor otherwise. A vehicle
    // that breaks off and walks to the yard stands there at half strength for
    // the rest of the match: only a `UnitRepair=` shed puts it back together,
    // and a house that has one has it for exactly this.
    let post = brain.census.repair_cell.or(brain.census.base_cell);
    // Collected before anything is written: the routine takes the unit, and
    // the ledger is borrowed while it is read.
    let retreating: Vec<u64> = brain
        .census
        .units
        .iter()
        .filter(|unit| unit.traits.combat && !unit.hand && unit.orderable)
        .filter(|unit| unit.health_permille <= RETREAT_HEALTH_PERMILLE)
        .map(|unit| unit.entity_id)
        .collect();
    for entity_id in retreating {
        out.attach(
            entity_id,
            MicroKind::Retreat,
            MicroParams {
                health_floor: RETREAT_HEALTH_PERMILLE,
                standoff_cells: 4,
                radius_cells: 10,
                hold_frames: HOLD_FRAMES as u32,
            },
            post,
        );
        // The routine owns the unit while it walks it out, so the layers above
        // leave it alone rather than ordering it back into the fight.
        brain
            .tracked
            .hold(entity_id, tick.saturating_add(HOLD_FRAMES));
    }
}

/// How long a routine keeps a unit it has taken, in frames: the retreat's own
/// `hold_frames`.
const HOLD_FRAMES: u64 = 15;

/// How long a fired super waits before this brain tries it again, in frames.
///
/// The charge itself is the real gate — a launched super is not ready until it
/// recharges — so this only covers the refusal case, where the click did not
/// take (no warhead for the type, the house's wallet for a paid launch) and the
/// button stays lit. Forty seconds (600 frames at 15 a second): long enough
/// not to spend the frame budget on a refused command, short enough to catch a
/// refusal that clears.
const SUPER_RETRY_FRAMES: u64 = 600;

/// Fire the house's charged supers at the enemy, the way a player clicks the
/// button and then the target.
///
/// Native has this decision (`AI_TryFireSW @ 0x005098F0`) but gates it on the
/// house not being human (`is_controlled_by_human`), and a brain-driven house
/// is a human's: without this step it never fires one. The decision is the
/// native one's in shape — for each charged super, in the rules' order, find a
/// target — with the target read off the ledger instead of the native value
/// table: the enemy's Construction Yard when one stands, otherwise the first
/// enemy structure.
///
/// Only powers aimed at an enemy cell are fired. The Iron Curtain, the Force
/// Shield and the Chrono pair are spent on the house's *own* units and
/// buildings, and firing them at the enemy burns the charge for nothing; they
/// belong to a defensive decision this brain does not make yet.
pub(super) fn supers(
    brain: &mut RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    out: &mut Outbox,
) {
    if !obs.super_weapons() {
        return;
    }
    let ready = obs.ready_super_weapons(rules);
    if ready.is_empty() {
        return;
    }
    let tick = obs.tick();
    let mut fired = 0;
    for name in &rules.super_weapon_order {
        if fired >= SUPERS_PER_FRAME {
            break;
        }
        let Some(type_id) = obs.type_id(name) else {
            continue;
        };
        if !ready.contains(&type_id) {
            continue;
        }
        if brain
            .super_at
            .get(&type_id)
            .is_some_and(|at| tick < *at)
        {
            continue;
        }
        let Some(super_weapon) = rules.super_weapon(name) else {
            continue;
        };
        if !spent_on_the_enemy(super_weapon.kind) {
            continue;
        }
        // Where it lands is the power's own answer, not one cell for all of
        // them: a paradrop is soldiers, and soldiers dropped into the enemy's
        // own base land inside everything that stands there.
        let Some(cell) = landing_cell(brain, rules, obs, aim_of(super_weapon.kind)) else {
            continue;
        };
        log::info!("auto play: fire {name} at {cell:?}");
        brain
            .super_at
            .insert(type_id, tick.saturating_add(SUPER_RETRY_FRAMES));
        out.intent(Intent::LaunchSuperWeapon {
            sw_type_id: type_id,
            cell,
        });
        fired += 1;
    }
}

/// How many powers one frame fires.
///
/// One. Everything that is ready was being launched on the same frame, so a
/// house that had charged a missile and a paradrop together spent both on the
/// same second and had nothing when the fight it was saving them for arrived.
const SUPERS_PER_FRAME: usize = 1;

/// Where a power of this kind belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aim {
    /// Where the enemy's base is: a missile, a storm, a dominator.
    Enemy,
    /// Where the house's own army stands: a paradrop lands soldiers, and they
    /// have to form up before they walk into anything.
    Rally,
    /// Where the house cannot see: a spy plane, a psychic reveal.
    Recon,
}

/// What a power is for, by its kind: `Type=` in the rules.
fn aim_of(kind: SuperWeaponKind) -> Aim {
    match kind {
        SuperWeaponKind::ParaDrop
        | SuperWeaponKind::AmerParaDrop
        | SuperWeaponKind::UnitDelivery => Aim::Rally,
        SuperWeaponKind::SpyPlane | SuperWeaponKind::PsychicReveal => Aim::Recon,
        _ => Aim::Enemy,
    }
}

/// The cell an aim resolves to this frame.
fn landing_cell(
    brain: &RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
    aim: Aim,
) -> Option<(u16, u16)> {
    match aim {
        Aim::Enemy => enemy_structure_cell(brain, rules, obs),
        // Dropped where the army is, so they arrive as a squad rather than as
        // eight soldiers walking out of the enemy's own base one at a time.
        // With no army yet, the base: they are defenders until there is one.
        Aim::Rally => brain
            .census
            .units
            .iter()
            .find(|unit| unit.traits.combat)
            .map(|unit| unit.cell)
            .or(brain.census.base_cell),
        // What the house cannot see. The shroud has no projection yet, so this
        // is the enemy's base for now — the one place worth looking — and it
        // belongs to the exploration layer when that has a target source.
        Aim::Recon => enemy_structure_cell(brain, rules, obs),
    }
}

/// Whether a power is aimed at an enemy cell rather than held for the house's
/// own force.
fn spent_on_the_enemy(kind: SuperWeaponKind) -> bool {
    match kind {
        SuperWeaponKind::MultiMissile
        | SuperWeaponKind::LightningStorm
        | SuperWeaponKind::ParaDrop
        | SuperWeaponKind::AmerParaDrop
        | SuperWeaponKind::SpyPlane
        | SuperWeaponKind::PsychicReveal
        | SuperWeaponKind::PsychicDominator
        | SuperWeaponKind::GeneticConverter
        | SuperWeaponKind::GenericWarhead
        | SuperWeaponKind::UnitDelivery
        // Ares' seeker picks its own target among the enemy's
        // (`AiTargetingMode::HunterSeeker`, `HouseMask::ENEMIES`), so a cell
        // of theirs is still what it is fired at.
        | SuperWeaponKind::HunterSeeker => true,
        // Spent on the house's own units and buildings, or on nothing.
        SuperWeaponKind::IronCurtain
        | SuperWeaponKind::ChronoSphere
        | SuperWeaponKind::ChronoWarp
        | SuperWeaponKind::ForceShield
        | SuperWeaponKind::NoEffect => false,
    }
}

/// The cell a super is fired at: the enemy Construction Yard when one stands,
/// otherwise the first enemy structure in storage order.
///
/// The yard is native's own most-valued target (`AI_FindBestRallyTarget`'s
/// value table puts a Construction Yard at 100 against a power plant's 60 and a
/// tank's 2), and it is the cell a single missile decides a match at.
fn enemy_structure_cell(
    brain: &RetailBrain,
    rules: &RuleSet,
    obs: &Observation<'_>,
) -> Option<(u16, u16)> {
    let mut first = None;
    for enemy in &brain.census.enemies {
        let Some(ty) = obs.object_type(rules, enemy.type_id) else {
            continue;
        };
        if ty.category != ObjectCategory::Building {
            continue;
        }
        if ty.build_const_eligible {
            return Some(enemy.cell);
        }
        first.get_or_insert(enemy.cell);
    }
    first
}

/// Whether an object is one these orders step around: an infantry, vehicle or
/// aircraft that is neither a gatherer nor a Construction Yard maker.
/// Buildings and terrain are not.
pub(super) fn is_combat_unit(rules: &RuleSet, ty: &ObjectType) -> bool {
    super::census::Traits::of(rules, ty).combat
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sim::world::Simulation;

    /// A snapshot arena for the steps that do not read the scenario's own
    /// layering: the repository's own `ini/`, so this holds without `RA2_DIR`.
    fn snapshot(seed: u64) -> Option<(Simulation, RuleSet)> {
        let rules = crate::rules::retail_ini_fixture::retail_unlayered_rules()?;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// A transport carries the army; it does not fight in it. The night hawk
    /// carries a cannon, so "has a primary weapon" counted it as army and the
    /// house's fighting strength was measured in passenger seats.
    #[test]
    fn a_transport_is_not_counted_as_army() {
        let Some((sim, rules)) = snapshot(0xA17) else {
            return;
        };
        let _ = sim;
        let night_hawk = rules.object("SHAD").expect("the night hawk");
        assert!(
            !is_combat_unit(&rules, night_hawk),
            "a five-seat transport is not army"
        );
        let gi = rules.object("E1").expect("a GI");
        assert!(is_combat_unit(&rules, gi), "a soldier is");
    }

    /// A paradrop is soldiers, and soldiers dropped into the enemy's own base
    /// land inside everything standing there. They land on the army. What the
    /// house cannot see is what a spy plane is for.
    #[test]
    fn a_paradrop_lands_on_the_army_not_in_the_enemy_base() {
        assert_eq!(aim_of(SuperWeaponKind::AmerParaDrop), Aim::Rally);
        assert_eq!(aim_of(SuperWeaponKind::ParaDrop), Aim::Rally);
        assert_eq!(aim_of(SuperWeaponKind::SpyPlane), Aim::Recon);
        assert_eq!(aim_of(SuperWeaponKind::MultiMissile), Aim::Enemy);
    }

    /// A `DeployFire=` soldier takes its deployed position when a hostile
    /// reaches it. The D key is a toggle, so the step only answers on the edge:
    /// a soldier standing in an empty field is not sent down, and one that has
    /// taken the pose is not walked back out of it.
    #[test]
    fn a_soldier_deploys_when_the_fight_reaches_it() {
        let Some((mut sim, rules)) = snapshot(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("E1", "Americans", 20, 20, 255, &rules)
            .expect("a GI");
        // Interned before the projection borrows the world: the ledger's
        // enemies are named by id, and the interner is the world's.
        let hostile = sim.interner.intern("E2");
        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        brain.census.refresh(
            &crate::rules::unit_role::RoleTable::build(&rules),
            &rules,
            &obs,
        );
        let Some(gi) = brain.census.units.first().map(|unit| unit.entity_id) else {
            return;
        };

        // Nothing in front of it: an empty field is not a fight.
        let mut out = Outbox::default();
        deploy(&mut brain, &obs, &mut out);
        assert!(
            out.commands.is_empty(),
            "no hostile, no D key: {:?}",
            out.commands
        );

        // A hostile three cells away is.
        brain.census.enemies.push(EnemyEntity {
            entity_id: gi.wrapping_add(1),
            type_id: hostile,
            cell: (23, 20),
        });
        let mut out = Outbox::default();
        deploy(&mut brain, &obs, &mut out);
        assert!(
            out.commands.iter().any(|command| matches!(
                command,
                Command::ToggleInfantryDeploy { entity_id } if *entity_id == gi
            )),
            "the soldier takes the deployed position: {:?}",
            out.commands
        );

        // The pose is an animation and `Is_Deployed` only follows it, so the
        // soldier still reads as standing. Asking again now would toggle it
        // straight back up, every frame.
        let mut again = Outbox::default();
        deploy(&mut brain, &obs, &mut again);
        assert!(
            again.commands.is_empty(),
            "one D key per soldier: {:?}",
            again.commands
        );
    }

    /// The retail rival arena, needed to field real object types. `None`
    /// without `RA2_DIR`, as the rest of the suite does: these orders read the
    /// scenario's own layering, which the unlayered snapshot does not carry.
    fn arena(seed: u64) -> Option<(Simulation, RuleSet)> {
        let retail = crate::rules::retail_ini_fixture::retail_battle_rules()?;
        let rules = retail.rules;
        let mut sim = crate::sim::arena_fixture::rival_arena(&rules, seed);
        sim.intern_rule_type_ids(&rules);
        sim.resolve_type_handles(&rules);
        Some((sim, rules))
    }

    /// A spy's click on a building is the same Capture click an engineer's is:
    /// one order, and the per-cell handler decides which of the two it is. A
    /// spy has no combat value to rank, so without this order it stands where
    /// it was built.
    #[test]
    fn a_spy_walks_into_a_building_the_way_an_engineer_does() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let spy = sim
            .spawn_object("SPY", "Americans", 8, 8, 64, &rules)
            .expect("a spy");
        sim.spawn_object("NAPOWR", "Russians", 20, 20, 64, &rules)
            .expect("a power plant a spy may enter");
        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        captures(&mut brain, &rules, &obs, &mut out);

        assert!(
            out.orders
                .iter()
                .any(|order| order.actor == spy && matches!(order.goal, Goal::Capture { .. })),
            "a spy is sent into the building, not left standing: {:?}",
            out.orders
        );
        assert!(
            walks_into_buildings(&rules, rules.object("SPY").expect("the spy")),
            "Infiltrate= is a Capture click"
        );
        assert!(
            walks_into_buildings(&rules, rules.object("ENGINEER").expect("the engineer")),
            "and so is Engineer="
        );
        assert!(
            !walks_into_buildings(&rules, rules.object("E1").expect("a GI")),
            "a rifleman is not"
        );
    }

    /// A spy takes what is worth the walk, and only what a spy may enter: the
    /// power plant over the refinery beside it, and neither over a derrick the
    /// click would have refused.
    #[test]
    fn a_spy_targets_the_building_worth_the_most() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let spy = sim
            .spawn_object("SPY", "Americans", 8, 8, 64, &rules)
            .expect("a spy");
        // Nearest of all, and `Capturable=`, but not `Spyable=`.
        let derrick = sim
            .spawn_object("CAOILD", "Russians", 10, 10, 64, &rules)
            .expect("a derrick");
        let refinery = sim
            .spawn_object("NAREFN", "Russians", 12, 12, 64, &rules)
            .expect("a refinery");
        let power = sim
            .spawn_object("NAPOWR", "Russians", 22, 22, 64, &rules)
            .expect("a power plant");

        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        captures(&mut brain, &rules, &obs, &mut out);

        let target = out
            .orders
            .iter()
            .find(|order| order.actor == spy)
            .and_then(|order| match order.goal {
                Goal::Capture { target } => Some(target),
                _ => None,
            })
            .expect("the spy is given a target");
        assert_eq!(
            target,
            power,
            "the power plant blacks the enemy out; the derrick and the refinery are nearer"
        );
        assert_ne!(target, derrick);
        assert_ne!(target, refinery);
    }

    /// `Occupier=` infantry garrison the house's own bunker, through the same
    /// Enter click.
    #[test]
    fn riflemen_garrison_a_bunker_of_their_own() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let gi = sim
            .spawn_object("E1", "Americans", 10, 12, 64, &rules)
            .expect("a GI");
        // Above `ConditionRed`: a bunker at red health admits nobody, the same
        // rule the sim's own garrison ask enforces.
        let bunker = sim
            .spawn_object("NABNKR", "Americans", 12, 12, 64, &rules)
            .expect("a battle bunker");

        let obs = Observation::new(&sim, owner);
        let mut brain = RetailBrain::new(0xA17);
        let mut out = Outbox::default();
        garrison(&mut brain, &rules, &obs, &mut out);

        assert!(
            out.orders
                .iter()
                .any(|order| order.actor == gi
                    && matches!(order.goal, Goal::Enter { target } if target == bunker)),
            "the rifleman takes the bunker: {:?}",
            out.orders
        );
    }

    /// A charged power is fired at the enemy's Construction Yard. Native has
    /// this decision (`AI_TryFireSW`), but it is gated on the house not being
    /// human — and a brain-driven house is a human's, so without this step the
    /// button is never pressed.
    #[test]
    fn a_charged_super_is_fired_at_the_enemy_yard() {
        use crate::sim::superweapon::SuperWeaponInstance;

        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        sim.spawn_object("NACNST", "Russians", 24, 24, 255, &rules)
            .expect("an enemy Construction Yard");
        let Some(name) = rules
            .super_weapon_order
            .iter()
            .find(|name| {
                rules
                    .super_weapon(name)
                    .is_some_and(|sw| spent_on_the_enemy(sw.kind))
            })
            .cloned()
        else {
            return;
        };
        let type_id = sim.interner.intern(&name);
        let mut instance = SuperWeaponInstance::new(type_id, owner, 0);
        instance.set_is_active_for_test(true);
        instance.set_is_ready_for_test(true);
        instance.set_charge_start_tick_for_test(0);
        instance.set_charge_duration_for_test(9_000);
        sim.super_weapons_mut_for_test()
            .entry(owner)
            .or_default()
            .insert(type_id, instance);

        let obs = Observation::new(&sim, owner);
        assert!(obs.super_weapons(), "the arena plays with powers on");
        assert_eq!(
            obs.ready_super_weapons(&rules),
            vec![type_id],
            "the charged power is the one live button"
        );

        let mut brain = RetailBrain::new(0xA17);
        brain.census.refresh(
            &crate::rules::unit_role::RoleTable::build(&rules),
            &rules,
            &obs,
        );
        let mut out = Outbox::default();
        supers(&mut brain, &rules, &obs, &mut out);

        assert_eq!(out.intents.len(), 1, "{:?}", out.intents);
        match out.intents[0] {
            Intent::LaunchSuperWeapon { sw_type_id, cell } => {
                assert_eq!(sw_type_id, type_id);
                assert_eq!(cell, (24, 24), "fired at the enemy yard");
            }
            ref other => panic!("a launch, not {other:?}"),
        }
        // The same frame again: the cooldown keeps a refused or recharging
        // button from being clicked every frame.
        let mut again = Outbox::default();
        supers(&mut brain, &rules, &obs, &mut again);
        assert!(again.intents.is_empty(), "{:?}", again.intents);
    }

    /// The Iron Curtain and the Force Shield are spent on the house's own
    /// force. Firing them at the enemy burns the charge for nothing.
    #[test]
    fn a_power_held_for_our_own_force_is_not_fired_at_the_enemy() {
        assert!(!spent_on_the_enemy(SuperWeaponKind::IronCurtain));
        assert!(!spent_on_the_enemy(SuperWeaponKind::ForceShield));
        assert!(!spent_on_the_enemy(SuperWeaponKind::ChronoSphere));
        assert!(spent_on_the_enemy(SuperWeaponKind::MultiMissile));
        assert!(spent_on_the_enemy(SuperWeaponKind::LightningStorm));
    }

    /// The wrench is a toggle, so the house clicks it once per wound: a second
    /// click turns the repair off, and a house that asks every frame never heals
    /// anything. A building still going up is not a wound either.
    #[test]
    fn the_wrench_is_clicked_once_per_wound() {
        let Some((mut sim, rules)) = arena(0xA17) else {
            return;
        };
        let owner = sim.interner.intern("Americans");
        let hurt = sim
            .spawn_object("GAPOWR", "Americans", 16, 16, 200, &rules)
            .expect("a power plant");

        let mut brain = RetailBrain::new(0xA17);
        brain.census.refresh(
            &crate::rules::unit_role::RoleTable::build(&rules),
            &rules,
            &Observation::new(&sim, owner),
        );
        let obs = Observation::new(&sim, owner);
        // Nothing but a healthy building: nothing to click.
        let mut out = Outbox::default();
        repairs(&mut brain, &rules, &obs, &mut out);
        assert!(
            out.commands.is_empty(),
            "a building that has never stood whole is not a wound: {:?}",
            out.commands
        );

        brain.repaired.insert(hurt);
        let mut out = Outbox::default();
        repairs(&mut brain, &rules, &obs, &mut out);
        assert_eq!(
            out.commands
                .iter()
                .filter(|command| matches!(command, Command::ToggleRepair { .. }))
                .count(),
            1,
            "one click for one wound, not one per frame: {:?}",
            out.commands
        );
        let mut again = Outbox::default();
        repairs(&mut brain, &rules, &obs, &mut again);
        assert!(
            again.commands.is_empty(),
            "the wrench is already on: {:?}",
            again.commands
        );
    }

    /// An engineer takes the map's own building before the enemy's, and an
    /// unguarded one before a guarded one — the nearest capturable was how it
    /// walked into a base and died for the derrick inside it.
    #[test]
    fn an_engineer_prefers_the_map_over_the_enemy_and_the_clear_over_the_guarded() {
        let sighting = |id: u64, cell: (u16, u16)| crate::sim::control::observation::EnemyEntity {
            entity_id: id,
            type_id: crate::sim::intern::InternedId::default(),
            cell,
        };
        let targets = [
            sighting(1, (14, 14)), // enemy base, 2 cells away, guarded
            sighting(2, (20, 10)), // the map's own derrick, 6 cells away, clear
        ];
        let hostile: std::collections::BTreeSet<u64> = [1].into_iter().collect();
        let guards = [(15u16, 14u16)];
        let chosen =
            engineer_preference(&targets, &hostile, &guards, (16, 10)).expect("a target");
        assert_eq!(
            chosen.entity_id, 2,
            "the derrick nobody is standing next to, not the one inside the base"
        );
    }

    /// A hostile pacing across the old single radius had the soldier kneeling
    /// and rising forever, and a soldier mid-animation cannot shoot — which is
    /// what "sometimes it fires, sometimes it does not" looked like on the
    /// screen. The pose now has a dead band, so nothing between the two radii
    /// changes it.
    #[test]
    fn a_dug_in_soldier_keeps_its_pose_until_the_fight_is_gone() {
        let squared = |cells: i32| cells * cells;
        assert_eq!(pose_to_take(false, squared(5)), Some(true), "down it goes");
        assert_eq!(
            pose_to_take(true, squared(8)),
            None,
            "still kneeling: the fight has not left"
        );
        assert_eq!(
            pose_to_take(true, squared(12)),
            Some(false),
            "the fight is gone, stand up"
        );
        assert_eq!(
            pose_to_take(true, i32::MAX),
            Some(false),
            "nothing at all: stand up"
        );
        assert_eq!(
            pose_to_take(false, squared(8)),
            None,
            "a standing soldier does not kneel for a distant one"
        );
    }

    /// Every unit a scenario spawns for the player starts on area guard. The
    /// brain used to read that as "this unit already has a job" and left the
    /// whole opening force standing in the yard for the match; a guard is the
    /// posture to replace, not a reason to leave a unit alone.
    #[test]
    fn an_area_guard_unit_is_not_a_unit_with_a_job() {
        assert!(
            !holds_or_fights(MissionId::from_known(MissionType::AreaGuard)),
            "the posture it was born with is the posture to replace"
        );
        assert!(
            holds_or_fights(MissionId::from_known(MissionType::Attack)),
            "a unit in a fight is not to be re-tasked"
        );
        assert!(
            holds_or_fights(MissionId::from_known(MissionType::Move)),
            "a unit on its way somewhere is not idle"
        );
    }
}
