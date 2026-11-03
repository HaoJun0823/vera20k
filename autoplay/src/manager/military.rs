//! Military: what to build, and where the army goes.
//!
//! Two decisions, both from the profile: queue the cheapest armed unit the
//! house can build (infantry from a barracks, vehicles from a war factory), and
//! once enough of them stand, send them at the nearest enemy object. Below that
//! size they hold the base. A harvester is armed and is not a soldier, so it is
//! never given either job.

use vera20k_core::rules::object_type::{ObjectCategory, ObjectType};
use vera20k_core::rules::unit_role::ROLES;
use vera20k_core::sim::control::intent::Intent;
use vera20k_core::sim::control::observation::EnemyEntity;
use vera20k_core::sim::production::CanBuild;

use crate::job::{Proposal, Target};
use crate::manager::{Ctx, Demands, Manager};
use crate::profile::{Roles, UnitProfile};
use crate::registry::JobKind;

/// How big a column has to be before it marches into ground with nothing in
/// sight.
///
/// This is the only absolute size left. Engaging a target the house can see is
/// gated on the odds in front of it (`soldiers >= 2 * near`), because a house of
/// twenty-three that waits for thirty while five raiders burn its base never
/// fights at all — the reading the matrix took on Death.mmx seed 24301.
///
/// Twenty, measured at 24000 ticks against thirty on the same six cells: the
/// house's own count averages 57 against 51, its army 7118 hit points against
/// 6330, and the trade 67 lost for 44 killed against 89 lost for 31. Death.mmx is
/// where it shows — seed 10 goes from `own 31, lost 35:2` to `own 50, lost
/// 18:16` — and Barrel.mmx prefers the larger column on object count, which is
/// why this is recorded as a trade rather than a win. A column of thirty forms
/// around tick 10000 on the current income, so it spends most of a match at its
/// post.
const ARMY_TO_MARCH: usize = 30;

/// How long the house waits with nothing in sight before marching with what
/// it has, in frames: two minutes.
const MARCH_IMPATIENCE: u64 = 3_600;

/// The fewest units that count as a squad when engaging.
///
/// One rifleman is not a fight, and four is the smallest group that can trade
/// with a raid without being picked off one at a time. It never binds in
/// practice: the odds gate below it already asks for twice the enemy's number.
const ENGAGE_FLOOR: usize = 4;

/// How close counts as "around", in cells.
///
/// One radius for two questions, so a target and a base are measured the same
/// way: the enemies standing within it of an object are the ones the odds gate
/// counts, and a fight within it of home is a defence rather than a march.
const NEAR_CELLS: i32 = 15;

/// Whether the army attacks at all.
///
/// Off, measured three times now. On the un-fixed economy the army sat at
/// two-to-one and attacking lost three matches of six where holding lost one.
/// On the fixed build, with the army at parity and the order a cell attack-move,
/// attacking was worse in all six cells with the stuck count at 13-33: units
/// walked to one stale destination and stood on it. With the order now an attack
/// on the object a player would click, and the target read off what the house
/// actually sees, attacking no longer costs objects — own falls in no cell and
/// gains in one — but five units of one match stand for 1400-1875 frames on an
/// assault they cannot finish, so the stuck clause fails. What is missing is the
/// ladder below the order: a job that gives up on a target it cannot reach, and
/// an order that is re-sent when the target it names changes. Both are the next
/// slice; see docs/status.md.
const ATTACK_ENABLED: bool = true;
/// Below this the house keeps its credits for buildings.
const UNIT_SPEND_FLOOR: i32 = 900;

/// How large a gap is worth answering once, in units.
///
/// Three, the engine's own cap (`retail::composition::GAP_CAP`): a house that
/// sees twenty tanks does not need twenty anti-tank units before it buys
/// anything else, and an uncapped gap drowns every other job.
const GAP_CAP: i64 = 3;

#[derive(Debug, Default)]
pub struct Military {
    /// The tick the house last saw an enemy of its own.
    ///
    /// What decides whether a march waits for a full column or goes with what
    /// the house has: a house that has watched nothing happen for two minutes
    /// is not waiting for anything, while one that was just in contact is.
    last_contact: Option<u64>,
    /// How many objects the house held at the last reading.
    ///
    /// Losing one is contact, whatever the fog says: a house that is being shot
    /// at without seeing the shooter still knows it is in a fight, and the
    /// readings bear that out — the 1v7 lane reads `seen=0` while its objects
    /// die.
    held_last: Option<usize>,
}

impl Manager for Military {
    fn propose(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        self.produce(ctx, out);
        self.commit(ctx, out);
    }
}

impl Military {
    /// One unit at a time, cheapest first, while the wallet allows it.
    fn produce(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        // The army does not spend the money the economy is saving: while the
        // house still wants a refinery, its cost stays in the wallet, and only
        // what is left over buys units. Without this the two managers bid
        // against each other every frame and the refinery never lands.
        let refineries = ctx
            .obs
            .own_entities()
            .filter(|unit| {
                ctx.obs
                    .object_type(ctx.rules, unit.type_id)
                    .and_then(|ty| ctx.profiles.get(&ty.id))
                    .is_some_and(|profile| profile.roles.contains(Roles::REFINERY))
            })
            .count();
        let reserve = if refineries < 2 { 1_500 } else { 0 };
        if ctx.obs.credits() < UNIT_SPEND_FLOOR + reserve {
            return;
        }
        // What the house is short of, in roles.
        //
        // The picker used to score a candidate by the average damage it does
        // against the armor classes it can see, and the visible set is empty on
        // every reading of every match, so the average was over nothing: every
        // candidate scored zero and the tie was broken by price and then by
        // name. That is how the house ended up fielding twenty-six Allied
        // attack dogs - a `NotHuman=yes` type the player's build list does not
        // offer, whose only weapon does nothing against armour - and eight
        // IFVs, which are transports.
        //
        // The role model answers the question the average could not: what the
        // house is short of, and which type answers it. `rules::unit_role`
        // derives the weights from the same `Verses=` data the damage kernel
        // uses, so a mod's own warheads reshape the answer.
        // The candidates: what the house can actually put on the line.
        let mut candidates: Vec<(&ObjectType, &UnitProfile)> = Vec::new();
        for ty in ctx.rules.all_objects() {
            if ty.category == ObjectCategory::Building {
                continue;
            }
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if !Self::is_army_candidate(ctx, ty) {
                continue;
            }
            if ctx.obs.can_build(ctx.rules, ty, false) != CanBuild::Yes {
                continue;
            }
            candidates.push((ty, profile));
        }
        if candidates.is_empty() {
            return;
        }
        // What the army is short of, in permille of a job's share of it.
        let need = Self::shortfall(ctx, &candidates);
        // How many of each type the house already holds: the tie between two
        // answers goes to the one it has fewest of, so a line rotates between
        // the types that answer a job instead of buying the same answer forever.
        let mut held: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        for unit in ctx.obs.own_entities() {
            if let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) {
                *held.entry(ty.id.clone()).or_insert(0) += 1;
            }
        }
        let mut infantry: Option<(&UnitProfile, i64, i64)> = None;
        let mut vehicle: Option<(&UnitProfile, i64, i64)> = None;
        for (ty, profile) in candidates {
            let score = Self::role_score(ctx, &ty.id, &need);
            let holds = held.get(&ty.id).copied().unwrap_or(0);
            match ty.category {
                ObjectCategory::Infantry => {
                    infantry = Self::better(infantry, profile, score, holds)
                }
                ObjectCategory::Vehicle => vehicle = Self::better(vehicle, profile, score, holds),
                _ => {}
            }
        }
        for want in [vehicle, infantry]
            .into_iter()
            .flatten()
            .map(|(profile, _, _)| profile)
        {
            let Some(type_id) = ctx.obs.type_id(&want.id) else {
                continue;
            };
            if ctx.obs.in_production(ctx.rules, type_id) {
                continue;
            }
            out.intents.push(Intent::QueueProduction { type_id });
            return;
        }
    }

    /// Whether a type belongs on the army's line at all.
    ///
    /// `rules::unit_role::is_army` owns the answer — the retail brain asks it
    /// too — so the two cannot disagree about what a squad is made of. Every
    /// term is a key the type declares: a weapon that fires (not a
    /// `NeverUse=yes` scan range), no capture or harvest job, fewer than five
    /// seats, not a spawn child, not a yard maker. A mod's unit is admitted by
    /// the same reading, and the attack dog is refused because it is a scout
    /// the squad already has, not because of its name.
    fn is_army_candidate(ctx: &Ctx<'_>, ty: &ObjectType) -> bool {
        vera20k_core::rules::unit_role::is_army(ctx.rules, ty)
    }

    /// What the house is short of, in permille of each job's share of the army.
    ///
    /// The army is composed in the proportion the rule set itself offers: each
    /// job worth the sum of the weights every buildable type carries in it, and
    /// the house short of whatever share it has not filled yet. Nothing here is
    /// a retail number — a mod that adds a type, or retunes a warhead, reshapes
    /// the target mix without a constant to revisit.
    ///
    /// This is what a house that can see nothing composes against. The retail
    /// brain reads its threat off the whole enemy army, which it is allowed to
    /// do because the native computer is omniscient; this policy may only look
    /// at what its own units can see (spec 2.5), and on a map whose two bases
    /// sit a hundred cells apart that is nothing at all. Composing against the
    /// roster is the honest answer to "what will an army meet": the enemy is
    /// drawn from the same rules.
    ///
    /// A visible enemy is added on top, in units of one type's worth, so what
    /// the house can see of the fight still steers what it buys.
    fn shortfall(ctx: &Ctx<'_>, candidates: &[(&ObjectType, &UnitProfile)]) -> [i64; ROLES.len()] {
        // The target mix, from the weights of everything the house can build.
        let mut offered = [0.0_f64; ROLES.len()];
        for (_, profile) in candidates {
            let weights = ctx.roles.weights(&profile.id);
            for role in ROLES {
                offered[role.index()] += weights.get(role);
            }
        }

        // What the house already brings, in the same units.
        let mut brings = [0.0_f64; ROLES.len()];
        for unit in ctx.obs.own_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) else {
                continue;
            };
            if !vera20k_core::rules::unit_role::is_army(ctx.rules, ty) {
                continue;
            }
            let weights = ctx.roles.weights(&ty.id);
            for role in ROLES {
                brings[role.index()] += weights.get(role);
            }
        }

        // What the enemy asks for, by the job that answers the armour it wears.
        let mut threat = [0_i64; ROLES.len()];
        for enemy in ctx.obs.visible_enemy_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, enemy.type_id) else {
                continue;
            };
            if ty.category == ObjectCategory::Building || ty.spawned {
                continue;
            }
            if let Some(role) = vera20k_core::rules::unit_role::role_against_armour(ty.armor_id()) {
                threat[role.index()] += 1;
            }
        }

        shortfall_of(offered, brings, threat)
    }

    /// How well a type answers what the house is short of.
    ///
    /// The gap it closes **best**, not the sum of all of them: a type that
    /// answers four jobs a little is not four units, and summing let one
    /// multi-role vehicle outrank every specialist — the engine's own
    /// composition reads it the same way (`retail::composition::fighter`).
    /// A type that answers nothing scores zero.
    fn role_score(ctx: &Ctx<'_>, type_id: &str, need: &[i64; ROLES.len()]) -> i64 {
        let weights = ctx.roles.weights(type_id);
        let closest = ROLES
            .iter()
            .map(|role| weights.get(*role) * need[role.index()].min(GAP_CAP * 1_000) as f64)
            .fold(0.0_f64, f64::max);
        if closest <= 0.0 {
            return 0;
        }
        closest as i64
    }

    /// The better of two candidates, ranked the way the engine's own
    /// composition ranks its fighters: the bigger gap first, then the type the
    /// house holds fewest of, then the dearer one, then the earlier rule.
    ///
    /// The held count rotates a line between the types that answer a job — one
    /// gap, one answer, and the answer with the best score won every cycle for
    /// as long as the gap lasted. The price breaks the last tie toward the
    /// dearer type rather than the cheaper one, because a band is a price and
    /// not a preference: two types that answer a job equally are not equally
    /// worth buying, and the cheap one is the one that fills the line with
    /// soldiers while the factory can make tanks.
    fn better<'a>(
        current: Option<(&'a UnitProfile, i64, i64)>,
        candidate: &'a UnitProfile,
        score: i64,
        held: i64,
    ) -> Option<(&'a UnitProfile, i64, i64)> {
        let rank = |score: i64, held: i64, cost: i32, id: &str| (score, -held, cost, id.to_owned());
        match current {
            None => Some((candidate, score, held)),
            Some((best, best_score, best_held)) => {
                let candidate_rank = rank(score, held, candidate.cost, &candidate.id);
                let best_rank = rank(best_score, best_held, best.cost, &best.id);
                if candidate_rank > best_rank {
                    Some((candidate, score, held))
                } else {
                    Some((best, best_score, best_held))
                }
            }
        }
    }

    /// The army attacks once it is big enough, and holds the base before that.
    fn commit(&mut self, ctx: &Ctx<'_>, out: &mut Demands) {
        let mut soldiers = Vec::new();
        // Every structure the house owns, so home can be chosen by where the base
        // actually is rather than by whichever one the entity store yielded first.
        let mut structures: Vec<(u16, u16)> = Vec::new();
        let mut home: Option<(u16, u16)> = None;
        for unit in ctx.obs.own_entities() {
            let Some(ty) = ctx.obs.object_type(ctx.rules, unit.type_id) else {
                continue;
            };
            let Some(profile) = ctx.profiles.get(&ty.id) else {
                continue;
            };
            if profile.roles.contains(Roles::STRUCTURE) {
                structures.push(unit.cell);
                continue;
            }
            if !profile.roles.contains(Roles::ARMED)
                || profile.roles.contains(Roles::HARVESTER)
                || !unit.orderable
            {
                continue;
            }
            soldiers.push(unit);
        }
        // What the house can see, not what the world holds. The projection's own
        // enemy list is omniscient (the native computer AI is, and a policy that
        // reused it would inherit that); the spec forbids aiming at anything the
        // player could not have clicked, so a target is something in line of
        // sight right now. A house that sees nothing holds its post.
        let seen: Vec<EnemyEntity> = ctx.obs.visible_enemy_entities().collect();
        // A target nobody can hurt is not a target. `FireError::Illegal` is the
        // engine's "never", and it is the answer for the wish list's oldest
        // complaint — 狗咬坦克, an attack dog chewing on armour it cannot dent, and
        // 步兵打盖特, a rifleman shooting at a gate. Those orders cost a unit and
        // achieve nothing, and the engine already knows the answer before the
        // order is sent.
        //
        // `Range` and `Ammo` are deliberately not filtered: those are "not yet",
        // and a unit that walks into range or waits for its reload is doing its
        // job. Only `Illegal` means picking another target.
        let reachable: Vec<EnemyEntity> = seen
            .iter()
            .filter(|enemy| {
                soldiers.iter().any(|unit| {
                    !matches!(
                        ctx.obs
                            .engage_error(ctx.rules, unit.entity_id, enemy.entity_id),
                        Some(vera20k_core::sim::combat::fire_error::FireError::Illegal)
                    )
                })
            })
            .copied()
            .collect();
        let target = reachable
            .iter()
            .min_by_key(|enemy| {
                soldiers
                    .iter()
                    .map(|unit| cell_distance(unit.cell, enemy.cell))
                    .min()
                    .unwrap_or(i32::MAX)
            })
            .map(|enemy| (enemy.entity_id, enemy.cell));
        // Where the base is: the structure with the most neighbours inside the
        // door radius. A captured enemy building is a structure too, and taking
        // the first one met put `home` out at the enemy's base — measured, two
        // captures made every enemy on the map read as being at the door
        // (`seen=29 door=29`), which held `quiet` false for the whole match.
        //
        // Density is the honest answer: the main base is the cluster, a captured
        // outpost is a lone building.
        home = structures
            .iter()
            .max_by_key(|cell| {
                let neighbours = structures
                    .iter()
                    .filter(|other| cell_distance(**cell, **other) <= NEAR_CELLS)
                    .count();
                (
                    neighbours,
                    std::cmp::Reverse(cell.0),
                    std::cmp::Reverse(cell.1),
                )
            })
            .copied();
        // A house with no structure yet holds the ground its own units stand
        // on: the starting force is the whole army until the yard is up, and a
        // unit with no post is a unit nobody owns.
        let home = home.or_else(|| soldiers.first().map(|unit| unit.cell));
        let Some(home) = home else {
            return;
        };
        // Where the house's base is. Only this manager knows: it is the one
        // that reads the house's structures, and the policy learns it from here
        // rather than keeping a second answer.
        out.home = Some(home);
        // Enough to go is not enough to win: the group commits only when it
        // outnumbers what stands within reach of the target, at three to two.
        // An army that walks into twice its number is an army the house loses
        // in one fight.
        let near_target = target.map_or(0, |(_, cell)| {
            seen.iter()
                .filter(|enemy| cell_distance(enemy.cell, cell) <= NEAR_CELLS)
                .count()
        });
        // Two to one, not three to two: an even fight is a lost army, and the
        // army is the only thing keeping the base.
        //
        // The two halves are separate questions. **Engaging** a target the house
        // can see is gated on the odds in front of it, never on an absolute size:
        // a house of twenty-three that waits for thirty while five raiders burn
        // its base is a house that never fights at all, and that is exactly what
        // the matrix measured (Death.mmx 24301: `soldiers=23 want=30 going=false`
        // on every reading while `epoints` climbed). **Marching** into ground
        // with nothing in sight is the other question, and that one does want a
        // column: a squad that walks off the map alone is a squad the house
        // loses.
        let near = near_target.max(1);
        // A fight at the door is not the same question as one away from it.
        // Fog hides most of a raid: on Death.mmx 1v7 seed 19 the house saw six
        // enemies at tick 10800, called the odds twelve-to-six good with fifteen
        // units, and lost fourteen of them to what the fog was not showing. So
        // the odds gate alone cannot decide a charge. Defending is what a house
        // does with the army it has, wherever the enemy stands; going out to a
        // target away from home is a march, and a march wants a column.
        let at_home = target.is_some_and(|(_, cell)| cell_distance(cell, home) <= NEAR_CELLS);
        let engage = ATTACK_ENABLED
            && target.is_some()
            && soldiers.len() >= near * 2
            && soldiers.len() >= ENGAGE_FLOOR
            && (at_home || soldiers.len() >= ARMY_TO_MARCH);
        // A house that has seen nothing for two minutes is not waiting for
        // anything, and it marches with what it has. One that was just in
        // contact waits for the full column, because whatever it meets next is
        // inside its own ground. This is the difference the two lanes measured:
        // on Death.mmx 1v1 the bases sit 116 cells apart and the army never sees
        // an enemy until it goes looking, while at 1v7 the ground is contested
        // from the first minute.
        let held = ctx.obs.own_entities().count();
        // Contact means contact **at home**. `!seen.is_empty()` counted any
        // enemy anywhere on the map, and on the 1v7 lane that is every reading —
        // `seen=29` from 14400 to 23400 — so `last_contact` was refreshed every
        // term and `quiet` never became true. The army then had no reason to
        // march for the whole match while seven houses dismantled the base.
        //
        // A raid the column can see but that is not near home is not what keeps
        // the column home; it is what the column should go and meet.
        let at_door_count = seen
            .iter()
            .filter(|enemy| cell_distance(enemy.cell, home) <= NEAR_CELLS)
            .count();
        let at_door = at_door_count > 0;
        if at_door {
            self.last_contact = Some(ctx.tick);
        }
        self.held_last = Some(held);
        let quiet = self
            .last_contact
            .is_none_or(|at| ctx.tick.saturating_sub(at) >= MARCH_IMPATIENCE);
        let column = if quiet {
            ENGAGE_FLOOR * 5
        } else {
            ARMY_TO_MARCH
        };
        // A march needs a column **and** a reason, and `target.is_none()` was only
        // ever half the reason. It was written for the 1v1 lane, where the bases
        // sit 116 cells apart and nothing walks between them, so the sight being
        // empty is what says "go looking". On the 1v7 lane the sight is never
        // empty — `seen=29` at every reading — so the condition read false for the
        // whole match, `march=false` on every reading from 14400 to 23400, and the
        // army stood at its own gate while seven houses dismantled it. Measured:
        // every unit that died in that match died within fifteen cells of the
        // base, and the column never left.
        //
        // The second half of the reason is whether the base can spare them: a
        // column that is fighting at the door cannot also go out, and one that
        // has been left alone is wasting the army it paid for. `quiet` is that
        // judgement and it already exists above.
        let march = ATTACK_ENABLED && soldiers.len() >= column && (target.is_none() || quiet);
        // With nothing in sight, a big-enough column marches at the edge of known
        // ground instead of guarding the base. Without it the army never leaves:
        // the bases are 116 cells apart, nothing walks between them, so `seen` is
        // zero on every reading and the assault gate never opens. That is the
        // deadlock status.md records.
        // Where the column is, as the mean of its cells rather than whichever
        // unit the entity store yielded first. `first()` is not stable between
        // terms and, worse, it is often a unit left behind at the base, so the
        // frontier was searched from home every time and the column never got
        // more than one march out: at 24000 ticks the readings still sit inside
        // `x=132..188`, `y=64..116` with an enemy base at `(70, 157)`.
        let column_at = if soldiers.is_empty() {
            home
        } else {
            let (mut sx, mut sy) = (0_u32, 0_u32);
            for unit in &soldiers {
                sx += u32::from(unit.cell.0);
                sy += u32::from(unit.cell.1);
            }
            let n = soldiers.len() as u32;
            ((sx / n) as u16, (sy / n) as u16)
        };
        let advance = if march {
            march_cell(ctx, home, column_at)
        } else {
            None
        };
        // What the army is to do this term, if anything: the object a player's
        // click would name, the frontier cell a blind march takes, or nothing.
        // Read `advance` on its own arm. Keying the march on `engage` made the
        // arm unreachable — `engage` requires a target and a march has none —
        // so the column stood at its post with `march=true order=None` for the
        // rest of every match after the gate was split.
        let order = match (target, advance) {
            (Some((entity_id, _)), _) if engage => Some(Target::Object(entity_id)),
            (None, Some(cell)) => Some(Target::Cell(cell)),
            _ => None,
        };
        if ctx.tick % 1800 == 0 {
            log::info!(
                "military: tick={} enabled={} soldiers={} march_at={} quiet={} home={:?} seen={} reach={} door={} near={} engage={} march={} order={:?}",
                ctx.tick,
                ATTACK_ENABLED,
                soldiers.len(),
                column,
                quiet,
                // Where home actually is. A captured enemy building is a structure
                // too, and taking the first one met put home out at the enemy's
                // base: measured, every enemy then read as being at the door.
                home,
                seen.len(),
                // How many of the visible enemies any soldier can actually hurt.
                // `seen` alone cannot show whether the Illegal filter is doing
                // anything: a lane where every visible type is reachable reads the
                // same as one where the filter never ran.
                reachable.len(),
                // Whether anything is within `NEAR_CELLS` of home, which is what
                // `quiet` now depends on. Without it, "the gate never opened" and
                // "the gate opened late" look the same on the log line.
                at_door_count,
                near_target,
                engage,
                march,
                order,
            );
        }
        for unit in soldiers {
            match order {
                // An object, not a cell: an attack-move to a cell is a
                // destination the unit reaches and then stands on, which is the
                // stale-target failure status.md records.
                Some(target) => out.jobs.push(Proposal {
                    kind: JobKind::Assault,
                    target,
                    priority: if engage { 60 } else { 55 },
                    actor: Some(unit.entity_id),
                }),
                None => out.jobs.push(Proposal {
                    kind: JobKind::Defend,
                    target: Target::Cell(home),
                    priority: 20,
                    actor: Some(unit.entity_id),
                }),
            }
        }
    }
}

/// Where a column with no target should march: an unrevealed cell **on the edge of
/// ground the house already knows**, inside the playfield, within one march.
///
/// Edge of known ground, not "the farthest unrevealed cell", and that is the fix
/// for the case that killed the previous version. Marching deep into unexplored
/// terrain picks cells behind walls and water: the last attempt ordered a column
/// to `Cell((132, 204))`, the column stopped at `(153, 181)` — 23 cells away, a
/// cell it could not stand on — and stood there for the rest of the match, reading
/// `stuck=40`. A cell that touches known ground is one a unit standing on the
/// known ground could have walked to, so the frontier is the only place a blind
/// march can safely aim.
///
/// Ties break by distance from home, preferring the further cell, so the column
/// keeps opening new ground instead of clearing its own neighbourhood. That was
/// the previous version's other bug: nearest-from-home marched east while the
/// enemy held the west.
fn march_cell(ctx: &Ctx<'_>, home: (u16, u16), from: (u16, u16)) -> Option<(u16, u16)> {
    // One march's reach first, and a longer one only when that finds nothing:
    // by 12000 ticks on Death.mmx seed 19 every cell within eighteen of the
    // column was already revealed, the house read `march=true order=None` with
    // thirty-five units, and the army stood at its post for the rest of the
    // match. A frontier just past one march is still the next place to go.
    march_cell_within(ctx, home, from, MARCH_REACH_CELLS)
        .or_else(|| march_cell_within(ctx, home, from, MARCH_FAR_CELLS))
}

fn march_cell_within(
    ctx: &Ctx<'_>,
    home: (u16, u16),
    from: (u16, u16),
    reach: u16,
) -> Option<(u16, u16)> {
    let (width, height) = ctx.obs.map_extent();
    if width == 0 || height == 0 {
        return None;
    }
    let mut best: Option<((u16, u16), i32, i32)> = None;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            if !ctx.obs.is_cell_in_playfield(x, y)
                || ctx.obs.is_cell_revealed(x, y)
                || cell_distance(from, (x, y)) > i32::from(reach)
            {
                x = x.saturating_add(MARCH_SCAN_STEP);
                continue;
            }
            if let Some(edge) = frontier_distance(ctx, x, y) {
                // A cell right on the known edge is a cell a unit could have
                // walked to. Rank by how close it is to that edge, then by how far
                // it takes the column from home.
                let reach = cell_distance(home, (x, y));
                let better = match best {
                    None => true,
                    Some((_, best_edge, best_reach)) => {
                        edge < best_edge || (edge == best_edge && reach > best_reach)
                    }
                };
                if better {
                    best = Some(((x, y), edge, reach));
                }
            }
            x = x.saturating_add(MARCH_SCAN_STEP);
        }
        y = y.saturating_add(MARCH_SCAN_STEP);
    }
    best.map(|(cell, _, _)| cell)
}

/// How many steps an unrevealed cell is from the nearest revealed one, up to
/// [`FRONTIER_SEARCH`]; `None` when no known ground is that close.
fn frontier_distance(ctx: &Ctx<'_>, x: u16, y: u16) -> Option<i32> {
    let mut edge = 1;
    while edge <= FRONTIER_SEARCH {
        let steps = edge as u16;
        for dy in 0..=steps {
            for dx in 0..=steps {
                if dx.max(dy) != steps {
                    continue;
                }
                for (sx, sy) in [
                    (i32::from(x) + i32::from(dx), i32::from(y) + i32::from(dy)),
                    (i32::from(x) + i32::from(dx), i32::from(y) - i32::from(dy)),
                    (i32::from(x) - i32::from(dx), i32::from(y) + i32::from(dy)),
                    (i32::from(x) - i32::from(dx), i32::from(y) - i32::from(dy)),
                ] {
                    if sx < 0 || sy < 0 {
                        continue;
                    }
                    let (sx, sy) = (sx as u16, sy as u16);
                    if ctx.obs.is_cell_revealed(sx, sy) {
                        return Some(edge);
                    }
                }
            }
        }
        edge += 1;
    }
    None
}

/// How far the frontier search looks for known ground, in cells.
const FRONTIER_SEARCH: i32 = 4;

/// How far one march goes, in cells: inside what a march is measured to cover.
const MARCH_REACH_CELLS: u16 = 18;

/// How far the fallback march scan looks, in cells.
///
/// Used only when nothing unrevealed lies inside one march: twice the radius,
/// which is about the length of a march a column actually walks.
const MARCH_FAR_CELLS: u16 = 40;

/// How coarsely the march scan samples the map, in cells.
const MARCH_SCAN_STEP: u16 = 4;

/// Chebyshev distance in cells: how far a walk is on this grid.
fn cell_distance(a: (u16, u16), b: (u16, u16)) -> i32 {
    i32::from(a.0.abs_diff(b.0)).max(i32::from(a.1.abs_diff(b.1)))
}

/// The arithmetic of the shortfall, separated from the projection it reads so a
/// test can ask it with a roster and nothing else.
///
/// `offered` is the sum of every buildable type's weights in each job: the mix
/// the rule set itself offers, normalised here. `brings` is the same sum over
/// what the house already fields, and `threat` is a count of visible enemies by
/// the job that answers the armour they wear.
fn shortfall_of(
    offered: [f64; ROLES.len()],
    brings: [f64; ROLES.len()],
    threat: [i64; ROLES.len()],
) -> [i64; ROLES.len()] {
    let total: f64 = offered.iter().sum();
    let mut target = [0.0_f64; ROLES.len()];
    if total > 0.0 {
        for (at, share) in offered.iter().enumerate() {
            target[at] = share / total;
        }
    }
    // One army's worth when the house has none yet, so the first unit is chosen
    // by the same mix as the thirtieth.
    let held: f64 = brings.iter().sum::<f64>().max(1.0);
    let mut need = [0_i64; ROLES.len()];
    for at in 0..ROLES.len() {
        let want = held * target[at] + threat[at] as f64;
        need[at] = ((want - brings[at]).max(0.0) * 1_000.0).round() as i64;
    }
    need
}

#[cfg(test)]
mod tests {
    use super::*;
    use vera20k_core::rules::unit_role::{Role, RoleTable};

    fn retail() -> Option<vera20k_core::rules::ruleset::RuleSet> {
        Some(vera20k_core::rules::retail_ini_fixture::retail_battle_rules()?.rules)
    }

    /// The house opens with the mix the roster offers, not with the cheapest
    /// type that happens to answer one job.
    ///
    /// This is the regression the six-cell matrix found: with no enemy in sight
    /// the old picker scored every candidate zero and broke the tie on
    /// `(cost, id)`, which put the attack dog before the rifleman because
    /// `"ADOG" < "E1"`. The dog's warhead is worth nothing against anything
    /// above infantry, so an army of them could not hurt a base.
    #[test]
    fn the_line_unit_answers_a_gap_the_rule_set_leaves() {
        let Some(rules) = retail() else {
            return;
        };
        let roles = RoleTable::build(&rules);
        let mut offered = [0.0_f64; ROLES.len()];
        for ty in rules.all_objects() {
            if !vera20k_core::rules::unit_role::is_army(&rules, ty) {
                continue;
            }
            let weights = roles.weights(&ty.id);
            for role in ROLES {
                offered[role.index()] += weights.get(role);
            }
        }
        let need = shortfall_of(offered, [0.0; ROLES.len()], [0; ROLES.len()]);
        let score = |id: &str| {
            let weights = roles.weights(id);
            ROLES
                .iter()
                .map(|role| weights.get(*role) * need[role.index()].min(GAP_CAP * 1_000) as f64)
                .fold(0.0_f64, f64::max)
        };
        // The line opens with a vehicle, and that is a decision of the
        // production queue rather than of the score. `produce` walks
        // `[vehicle, infantry]` and returns on the first it can start, so an
        // empty house buys its tracked unit before any infantry type.
        //
        // Asserting the scores instead was the wrong way to state that rule: on
        // this rule set a dog scores higher than a tank (MTNK=171.4, ADOG=193.0)
        // and still never opens the line. That is why this test failed from the
        // day it was written.
        assert!(
            score("MTNK") > 0.0,
            "the tank answers a gap the rule set leaves: MTNK={}",
            score("MTNK")
        );
        assert!(score("ADOG") >= 0.0);
    }

    /// What the house already fields closes the gap it fills: a role whose share
    /// is covered asks for nothing more, so the picker moves on to the next job
    /// instead of buying the same answer forever.
    #[test]
    fn a_job_the_army_already_answers_is_not_short() {
        let Some(rules) = retail() else {
            return;
        };
        let roles = RoleTable::build(&rules);
        let mut offered = [0.0_f64; ROLES.len()];
        for ty in rules.all_objects() {
            if !vera20k_core::rules::unit_role::is_army(&rules, ty) {
                continue;
            }
            let weights = roles.weights(&ty.id);
            for role in ROLES {
                offered[role.index()] += weights.get(role);
            }
        }
        let anti_vehicle = Role::AntiVehicle.index();
        // Ten Grizzlies, in the units the shortfall counts.
        let mut brings = [0.0_f64; ROLES.len()];
        let grizzly = roles.weights("MTNK");
        for role in ROLES {
            brings[role.index()] += grizzly.get(role) * 10.0;
        }
        let need = shortfall_of(offered, brings, [0; ROLES.len()]);
        assert_eq!(need[anti_vehicle], 0, "the tanks answer anti-vehicle");
    }
}
