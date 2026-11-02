# Where the policy stands

Measured 2026-11-02 on the v2 branch (commit 36d67b4fd), retail YR, hard
computer. One process per map and seed, side by side:

    python -m tools.autoplay_check --brain v2 --seeds 0x5EED,19,10 \
        --maps Death.mmx,Barrel.mmx --ticks 24000

## What runs

A match driven by this crate: the maker deploys, the house raises a power plant
to open the tree, a refinery, a barracks and a war factory, places each through
the native site search, keeps one harvester per refinery, queues the cheapest
armed unit its factories can build, and holds the army at its post. Every unit
that can take an order holds one, every order goes through the funnel the
player's clicks use, and every decision is read off the unit profile rather than
a type name.

## Numbers

1v1 hard, Death.mmx, 12000 ticks, three seeds (own vs enemy objects at the end):

| Seed | Own | Enemy | Unowned | Stuck |
|---|---|---|---|---|
| 0x5EED | 35 | 47 | 0 | 0 |
| 19 | 35 | 40 | 0 | 0 |
| 10 | 35 | 45 | 0 | 0 |

24000 ticks, both maps, three seeds: four of six survive the forty-minute mark.

| Map | Seed | Result |
|---|---|---|
| Death.mmx | 0x5EED | survives (25 vs 64) |
| Death.mmx | 19 | survives (33 vs 56) |
| Death.mmx | 10 | lost at 17680 |
| Barrel.mmx | 0x5EED | survives (27 vs 58) |
| Barrel.mmx | 19 | survives (25 vs 79) |
| Barrel.mmx | 10 | lost at 20435 |

1v7 hard, Death.mmx, 12000 ticks, three seeds: two of three survive. The retail
policy on 0x5EED lost at 11361.

## The recorded baseline (2026-11-02, commit 3f9ba3a4a, this machine)

The table above is a reading; this is a file on disk that a later run can be
diffed against, both maps and all three seeds in one pass:

    RA2_DIR=<retail> python -m tools.autoplay_check --brain v2 \
        --seeds 0x5EED,19,10 --maps Death.mmx,Barrel.mmx --ticks 12000 \
        --skip-tests --history target/autoplay-results/baseline.jsonl

| Map | Seed | Own | Enemy | Unowned | Stuck |
|---|---|---|---|---|---|
| Death.mmx | 0x5EED | 35 | 47 | 0 | 0 |
| Death.mmx | 19 | 35 | 40 | 0 | 0 |
| Death.mmx | 10 | 35 | 45 | 0 | 0 |
| Barrel.mmx | 0x5EED | 32 | 38 | 0 | 0 |
| Barrel.mmx | 19 | 34 | 45 | 0 | 0 |
| Barrel.mmx | 10 | 35 | 40 | 0 | 0 |

A second run of the same command read exactly the same in every cell
(`--compare-only`: 6 same, 0 better, 0 worse), so a diff is evidence about the
policy and not about the machine.

    RA2_DIR=<retail> python -m tools.autoplay_check --brain v2 \
        --seeds 0x5EED,19,10 --maps Death.mmx,Barrel.mmx --ticks 12000 \
        --skip-build --skip-tests --history target/autoplay-results/rerun.jsonl
    python -m tools.autoplay_check --compare-only \
        --baseline target/autoplay-results/baseline.jsonl \
        --history target/autoplay-results/rerun.jsonl

## The wallet is the ceiling, and the miners are why

`--start-credits 30000` on the same two maps and seed 0x5EED, 12000 ticks:

| Map | Own (10000 start) | Own (30000 start) | Enemy (30000 start) |
|---|---|---|---|
| Death.mmx | 35 | 85 | 49 |
| Barrel.mmx | 32 | 85 | 51 |

The army is not what stops the house at 35 objects; the wallet is. Five of the
six cells above end at 12 credits with the wallet unchanged for the last 3600
frames. Two readings under that:

- With 30000 in the bank the house spends down to 300, the economy reports
  `nothing to build ... refineries=3 defenses=4`, and the military manager stops
  too (`UNIT_SPEND_FLOOR` 900). Nothing is queued for the last four minutes: the
  build order has no target and the unit buyer has no budget.
- The harvesters do not unload. Across the seven miners of the 30000-credit
  Death run, the engine's state log reads `ReturnToRefinery → Dock` 527 times and
  `SearchOre → Harvest` with `cargo=0` 14 times — an empty hold arriving at ore,
  which is what a finished unload looks like. The retail brain on the same map
  and seed arrives at ore empty 96 times against about 93 dock entries. Ours
  enters Dock and comes back out full.

The state log is the engine's (`src/sim/miner`), so the reading is engine-side
even when the policy is what changed. The hypothesis that fits both native rules
the code cites — `new_order_pending` (`refinery_dock.rs:420`, `0x0073E539`) ends
an unload when a NavCom exists and the queued mission is neither none nor
Harvest, and `per_cell_release_dock_contact` (`0x0073ACD7`) drops the contact
unless the mission is Unload or Enter is queued — is that the policy re-sends
`Goal::Harvest` (a MegaMission *with a destination*) every term the miner's
mission is not Harvest, which is exactly during Enter and Unload. That is a
hypothesis, not yet a proven cause; the reading that decides it is the ratio
above, which the next change has to move.

## The harvest job is not re-sent into the engine's own dock cycle

The policy used to order a harvester again every term its mission was not
`Harvest` — which is exactly while the engine is docking it (`Enter` for the
refinery contact, `Unload` for the dump). That order is a MegaMission, and a
MegaMission is `Queue_Mission(mission, 1)`: it replaced the queued `Enter` that
`per_cell_release_dock_contact` (`0x0073ACD7`) needs to keep the radioed
refinery, so the truck dropped its dock and cycled back to the ore field full.
`Policy::living_out` now counts `Harvest`, `Enter` and `Unload` as the same job
being lived out, and a harvester is only ordered again when it is idle.

Same command and same baseline as above, after the change:

| Map | Seed | Own | Enemy | Unowned | Stuck |
|---|---|---|---|---|---|
| Death.mmx | 0x5EED | 46 (was 35) | 47 | 0 | 0 |
| Death.mmx | 19 | 51 (was 35) | 40 | 0 | 0 |
| Death.mmx | 10 | 50 (was 35) | 45 | 0 | 1 |
| Barrel.mmx | 0x5EED | 46 (was 32) | 38 | 0 | 1 |
| Barrel.mmx | 19 | 46 (was 34) | 39 | 0 | 1 |
| Barrel.mmx | 10 | 50 (was 35) | 40 | 0 | 0 |

Own is up in all six cells and down in none, no seed that survived is lost, and
unowned stays zero. The engine readings move with it: a miner arrives at ore
with an empty hold 26-35 times per match instead of 10-16, Dock entries fall from
166-172 to 26-32 (one dock per unload, as in the retail runs), and the wallet no
longer freezes — the last readings are 390-1164 credits instead of 12, and no
cell spends the last 3600 frames standing still.

The one clause that does not hold is `stuck`, which reads 1 in three cells where
it read 0. The reading is now per unit (`policy v2: stuck at ...`), and every
stuck unit is a CMIN holding `job=Harvest`, still for 90-210 frames. Between two
readings the counter resets (id=2911 reads `still=90` at 10800 and `still=210` at
11400), so those units do move; they pause for a few seconds at a time, which is
what harvesting in place and waiting at a one-dock refinery look like from a
projection that carries only the cell. Recorded as a residual: whether a pause is
a fill or a dock queue is not visible to the policy yet.

## The forty-minute reading, and the attack that is still off

The same build over 24000 ticks, three seeds, both maps, against the numbers the
handoff recorded on the un-fixed build:

| Map | Seed | Own | Enemy | Handoff's own (before) |
|---|---|---|---|---|
| Death.mmx | 0x5EED | 91 | 65 | 25 (survived) |
| Death.mmx | 19 | 51 | 57 | 33 (survived) |
| Death.mmx | 10 | 61 | 57 | lost at 17680 |
| Barrel.mmx | 0x5EED | 34 | 61 | 27 (survived) |
| Barrel.mmx | 19 | 102 | 65 | 25 (survived) |
| Barrel.mmx | 10 | 70 | 49 | lost at 20435 |

Six of six survive the forty-minute mark; four of six did before, and both seeds
that used to be lost now survive. Nothing here wins — every cell is a timeout,
which is the most a house that never attacks can do.

`ATTACK_ENABLED = true` for one run, everything else identical:

| Map | Seed | Own | Enemy | Stuck |
|---|---|---|---|---|
| Death.mmx | 0x5EED | 46 → 45 | 46 → 36 | 0 → 33 |
| Death.mmx | 19 | 51 → 50 | 45 → 44 | 0 → 33 |
| Death.mmx | 10 | 50 → 34 | 45 → 46 | 1 → 2 |
| Barrel.mmx | 0x5EED | 46 → 44 | 49 → 41 | 1 → 13 |
| Barrel.mmx | 19 | 46 → 46 | 39 → 40 | 1 → 29 |
| Barrel.mmx | 10 | 50 → 45 | 41 → 29 | 0 → 29 |

All six cells are worse, own falls in four of them, and the stuck count goes from
0-1 to 13-33. The per-unit reading added with the dock fix says why: 25-33 units
of the army stand around one stale target cell — `job=Assault
target=Cell((96,143))`, `still=500-1400` frames — while holding the job. The
enemy count does fall (49 → 41, 46 → 36, 45 → 44), so the army trades when it
arrives and then stops. Attacking stays off until an assault job can end on
arrival, pick a new target, judge local superiority and retreat.

One thing the probe needed first: an assault order is an attack-move with a
destination, and the engine lands that on `MissionType::Move` with the attack-move
latch set (`mega_mission.rs::foot_assign_target_command`, the `0x004DF141`
destination arm), not on `Attack`. The predicate now accepts both, and with the
flag off again the matrix reads identical in all six cells — verified, so the fix
is inert until attacking comes back.

## The army aims at what the house can see, and still does not attack

The projection could already answer "where is every enemy in the world"
(`enemy_entities`), and its own doc said so: the native computer AI is omniscient
and a policy reusing its decisions inherits that. The spec forbids it (2.5, "no
real-time enemy information outside vision"), which matters most at 1v7, where
omniscience is every base on the map at once. `Observation::visible_enemy_entities`
narrows that list to the cells the house's fog marks visible right now
(`FogState::is_cell_visible`, the L2 layer); a test spawns one rifleman beside the
house's own and one across the map and asserts that only the first is a target.
The army targets from it.

An assault is now an attack on the enemy *object* a player's click would name,
not an attack-move to where it stood, and the job ends when its object is no
longer visible. With attacking still off, the matrix reads identical in all six
cells, so the change is inert until the flag returns.

It also has to be re-aimed before that: the probe with the flag on reads 0 cells
with own down and one up (own 46-51 against the same 39-49, so the army no longer
loses objects), but five units of Barrel.mmx seed 0x5EED stand for 1400-1875
frames on `job=Assault target=Object(2579)` — an assault on something the house
can see and they cannot finish. The probe before this one, with a cell as the
target, lost objects in four cells and froze 25-33 units. So the next rung is a
job with a ladder below it: give up on a target this unit cannot reach, do not
propose it to that unit again, and re-send the order when the target it names
changes.

## Reverted: the reserve, again

The handoff's item 1 asks for the fixed reserves to become the real cost of the
cheapest harvester. Tried: the economy's `+200` margin and the unit buyer's floor
both read the cheapest buildable harvester off the profile table. It is worse in
all six cells — own 46-51 becomes 37-41 — because a house that keeps a truck's
cost in the bank stops buying units, and units are what the object count is. The
constant stays, and with it the open question of what the reserve is for now that
income works.

## The assault ladder, and what it did and did not fix

Three rungs were missing below the order, and `design.md` section 4 already names
them (no waiting on another pass; every rung terminates):

- a job whose object is no longer visible ends;
- a unit that has stood in one cell for 450 frames on an assault ends it and the
  target is not proposed to that unit again for 1800 frames (`stalled`, `cooled`);
- a unit that nothing proposed for gets the default job — guard the nearest
  structure, read off the profile — so `Unowned == 0` holds by construction.

With attacking on, the stuck count on the probe fell from 13-33 to 0-2 and
`unowned` from 0→14 in one cell to 0 in all six. Own falls in no cell.

What is left is the rung below those: an assault on a target the unit can see and
reach but cannot finish. Those units read `job=Assault target=Object(2579)` with
`still=1800` at the end of the run, and the abandonment that should fire at 450
frames reads `stalled=0` — because those units are re-assigned and re-ordered in
the same term they are counted, so the reading the ladder consults (`stood`) has
just been reset. The next reading has to come from the target's health, not from
the unit's cell: an assault that has not reduced its target in N frames is
`Ineffective`, which is the design's fourth outcome and the one still missing.

Two order-identity bugs turned up while measuring, and the matrix caught both as
a fall in own rather than a test:

- comparing the target for *every* watched job. A harvest job's target is the
  cell the truck stood on and a defend post is where the first soldier is before
  the yard exists, so both read as a new order every term: the dock regression
  again, own 46 → 34. Only an assault is compared by target.
- skipping the `issued` bookkeeping when the mission already showed the job, so a
  unit with no remembered order was re-ordered every term.

With attacking off the matrix reads identical in all six cells, so all of it is
inert until the flag returns.

## The fourth outcome, and the stuck clock none of it could be read with

`Ineffective` — the design's fourth ending — is now implemented: an assault whose
target's health has not moved for 450 frames ends, the pair is cooled down, and
`ineffective` counts it. Two defects had to be fixed before the number would
move, and both were found by reading it:

- the first version recorded the target's health unconditionally, which moved the
  timestamp every term, so the window could never elapse: `ineffective=0` in a
  match where units stood 1800 frames. The clock now remembers the reading that
  started the current run and resets only when the target loses a hit point;
- the stuck clock was keyed on the cell alone, so a unit switching from holding a
  post to attacking kept the cell it had held and read as "standing still on its
  assault" in the same term it was given one — 38 units at once. It is cleared
  when a unit's job changes, and a unit whose assault took health off its target
  this term is excluded: the spec's H2 counts a unit stuck when it holds a
  *movement* job and does not move, not a unit standing in range and firing.

With attacking on, the cell that read 38 reads 0, two cells read +1 stuck, own
falls in no cell and gains 3 in one. With the flag off the matrix is identical in
all six cells.

## The hard maps, on this build

| Map | Own | Enemy | Stuck | Reading |
|---|---|---|---|---|
| xcc_maps/xeb1.map | 30 | 36 | 1 | holds its side; no wall opened (walls=0) |
| xcc_maps/xtsunami.map | 35 | 43 | 0 | never leaves the starting island |

Before the assignment fix the island run read 28 stuck: the army was being sent
at a target it could not reach and froze there. It now holds instead, so the
number is zero and the failure has moved to where it belongs — the policy has no
transport job at all. Boarding itself is proven (the Enter mission boards an
IFV in a test), so this is policy work, not an engine gap.

## Known gaps

- The army does not attack, and attacking is measurably worse even at parity:
  six of six cells fall with the flag on, and the army stops at a stale target
  cell (see the section above). Re-targeting on arrival, local superiority,
  retreat and reinforcement are the next slice; the order path itself is sound
  now (attack-move lands on `Move`, and the predicate accepts it).
- The income stall is fixed at its source (the harvest cycle above); the reserves
  are not. One cell still ends with a refinery in production at 5 credits
  (Death.mmx, seed 10), and five of the six end with `nothing to build` while
  holding 300-661 credits: two refineries, three or four defences, and nothing
  else the wallet can afford. The unit buyer's `UNIT_SPEND_FLOOR` (900) plus its
  1500 reserve while the house wants refineries is the binding limit there.
- A refinery queued with only its cost plus the fixed 200 in the wallet takes the
  rest of the income while it is unfinished. Three attempted fixes made the
  matrix worse and were reverted; the reserves are still a constant.
- `stuck` no longer distinguishes a harvester that pauses from one that is
  frozen; see the residual in the section above. A pause is not visible in the
  projection, so the next reading has to come from the engine's miner state.
- The shipyard counts as a war factory. Splitting the role improved Death.mmx
  and lost every Barrel.mmx seed, so the split was reverted.
- Nothing for walls, transports, garrisons, superweapons, scouting, crates, or
  defence placement.

## How a change is judged

A change is kept when the matrix improves: own count up at a fixed tick, no seed
lost that survived before, unowned zero, stuck not up. One change per
measurement. Five of the experiments in this session improved one reading and
lost seeds on another map, and were reverted; the assignment bug above is why
one of them looked like a win.

The measurement is a run of `tools/autoplay_check.py` against the baseline
recorded above, which prints a cell as `worse` when own falls, a seed that
survived is lost, unowned rises or stuck rises (`--baseline` marks the cell, and
`--compare-only` re-reads two files without running anything).
