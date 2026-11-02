# Handoff: the auto-play rewrite

Written 2026-11-02 for an agent taking this over. Everything below is measured
on this branch; nothing here is a plan that has not been run.

## Where the work is

- Branch: feature/autoplay-v2 (fork HaoJun0823/vera20k, upstream
  YuriPlanet/vera20k). upstream/main was merged in (204 commits) and one
  merged call site fixed.
- The new policy lives in the workspace member autoplay/, outside the engine
  crate. vera20k-core still owns the engine; the policy reads a projection and
  answers through the same command funnel a player's click uses.
- The old policy (src/sim/strategy/retail/) is untouched and still the default.
  The new one is selected by name, never in place of it.

## Build and run

    # play build (client)
    python -m tools.cargo_run -- build --profile fast
    target/owned-worktrees/<id>/fast/vera20k.exe
    # the new policy in the client:
    set VERA20K_BRAIN=v2

    # headless match matrix: one process per map, faction and seed
    python -m tools.autoplay_check --brain v2 --seeds 0x5EED,19,10 --countries america,russia,yuri --maps Death.mmx,Barrel.mmx --ticks 24000

    # the policy crate's tests
    RA2_DIR=<retail> python -m tools.cargo_run -- test -p vera20k-autoplay --lib

RA2_DIR points at the retail install. The maps used by the matrix are
Death.mmx and Barrel.mmx (game root) and xcc_maps/xeb1.map and
xcc_maps/xtsunami.map.

## What the policy does today

One term is 15 frames. Managers propose, the assignment gives each unit at most
one job, and the outbox answers:

- deploys the maker, raises a power plant to open the tree, a refinery, a
  barracks and a war factory, places each through the native site search;
- keeps one harvester per refinery and buys the cheapest armed unit its
  factories can build;
- holds the army at its post (attacking is behind a flag that is off, below);
- every fact about a type comes from autoplay/src/profile/mod.rs, derived from
  RULESMD.INI through the production reader. No type name in the policy.

## Verified engine facts (do not re-derive)

| Question | Answer |
|---|---|
| Can a unit board a transport? | Yes: Goal::Enter (Command::MegaMission, mission Enter, destination = the transport). MoveToObject does not board. |
| Can the policy place a building? | Yes: control::placement::Placement::cell_for. |
| Can it ask whether a build is allowed? | Yes: Observation::can_build, which returns production::CanBuild (made public for this). |
| Can it see cargo? | Yes: Observation::passenger_count and inside_transport were added for it. |
| Is there a path-finding API? | Yes (sim/pathfinding, A*). It routes around static obstacles only; threats are the policy's job. |
| Does the client need a change to use the new policy? | One line: src/app/input/dispatch.rs reads VERA20K_BRAIN. |

## Measured baseline (2026-11-02, retail YR, hard computer)

Death.mmx 1v1, 12000 ticks, three seeds: own 35 / 35 / 35, enemy 47 / 40 / 45,
unowned 0, stuck 0. 24000 ticks over Death.mmx and Barrel.mmx: four of six
survive. 1v7 at 12000 ticks: two of three survive (the retail policy lost at
11361 on the same seed).

Faction sweep, Death.mmx 0x5EED, 12000 ticks, v2: america 35 own / 47 enemy,
russia 51 / 48, yuri 29 / 46. The 40-unit spread between factions is unexplained
and is the first thing worth chasing: the unit picker is "cheapest armed unit",
which quietly means "each faction's best-value unit", so every army ends up one
type.

Hard maps: xeb1.map holds its side and opens no wall; xtsunami.map never leaves
the starting island. Both are expected failures until the breach and transport
jobs exist.

## The measurement harness

tools/autoplay_check.py runs one process per map, faction and seed, all in
parallel, and appends one JSON line per match to
target/autoplay-results/history.jsonl: when, commit, brain, map, country, seed,
opponents, ticks, result, tick, own, enemy, unowned, stuck, yards, walls, start
credits / units / tech / roster, seconds. Two runs are compared by reading the
file.

Built, in the same session's later pass (see the end of this file): `--baseline
<file>` runs the matrix and diffs every cell against a recorded file, and
`--compare-only` re-reads two files without running anything. Without it, "did
this change break something" was answered by remembering a table.

## Method that worked, and the trap in it

- One change per measurement, over at least three seeds and two maps.
- A change is kept when the matrix improves: own up at a fixed tick, no seed lost
  that survived before, unowned still zero, stuck not up.
- Five experiments in this session improved one reading and lost seeds on the
  other map, and were reverted: shipyard role, refinery reserve, truck-first
  ordering, defence-before-refinery, unreachable-target ladder.

The trap: a change can look like a win because the behaviour it was supposed to
enable never ran. The assignment gave a unit every proposal in turn, so the
weakest proposal landed last and won: the army was told to attack and to hold at
the same time and always held. An "attack threshold" change then looked like a
big survival improvement because the army had never attacked. Fix the semantics
first, then measure behaviour.

## Open work, in the order it should be done

1. Economy reserves. The stall this item described is fixed (see the end of this
   file): the policy was re-ordering each harvester in the middle of its own
   dock. The reserve itself was then tried the way this item asks and reverted:
   with the economy's margin and the unit buyer's floor both read off the
   cheapest buildable harvester, own drops from 46-51 to 37-41 in all six cells,
   because a house that keeps a truck's cost in the bank stops buying units. What
   is still open is what the reserve is for now that income works; the constant
   stays until a reading says otherwise.
2. The army as units. Military holds no state: it recomputes who is a soldier
   every term. It needs squads (membership on a term), local superiority per
   squad, retreat, reinforcement and focus fire. Attacking is off until that
   exists (ATTACK_ENABLED in autoplay/src/manager/military.rs), and the reason is
   measured three times: at two-to-one on the un-fixed build, at parity with a
   cell target (worse in all six cells, stuck 13-33), and at parity with the
   object target and legal vision the policy has now (own down in no cell, up in
   one, but five units standing 1400-1875 frames on an assault they cannot
   finish). The order path is sound — an attack names the enemy object and the
   target is read from `visible_enemy_entities`, so the policy no longer aims at
   anything the player could not have clicked. The ladder below the order now
   exists (`stalled`/`cooled`/`ineffective` in the stats line, plus a default
   guard job so no unit is left unowned), and with the flag on the stuck count
   falls from 13-33 to 0 in the cell that showed it and +1 in two others, while
   own falls in no cell and gains in one. The `Ineffective` outcome reads the
   target's health (`Observation::health`, added for it), not the unit's cell,
   because re-assignment resets a cell timer. What is left before the flag can
   stay on is judgement: which target is worth the army, and when to break off.
   The machinery to end a job is now there; nothing yet chooses well.
3. Composition by role, not by price. Front line, ranged, anti-air quotas; this
   fixes the one-type armies and the faction spread at once. The spread is now
   measured properly: america 46, russia 70, yuri 45 at 12000 ticks on the same
   map and seed, after the Yuri refinery profile bug was fixed (see the end of
   this file). Russia's 24-object lead over the other two is the "cheapest armed
   unit" picker, untouched, and it is what this item is for.
   **Half of it is done and the half that matters was reverted.** `Roles::RANGED`
   exists (reach past six cells, from the weapon table), and it is inert: six of
   six cells identical. The quota that reads it — a third of the army ranged,
   the next unit filling the thin half — measured three cells worse and one
   better, so it was reverted under the keep rule. Quotas alone lower the object
   count, because longer-ranged units cost more and the count is what the matrix
   judges. Before retrying, find a reading that can see the range paying for
   itself: the matrix counts objects, and both houses are still holding at 12000
   ticks, so it cannot.
- **That reading now exists and it reads zero.** The harness records
  `own_lost:enemy_lost` off the census curve, and over 12000 ticks on Death.mmx
  neither side loses a single object — both curves rise monotonically. There is
  no fight for a range advantage to win, which is the real reason the quota
  failed. `enemy` in every table in this file is the enemy's economy, not its
  army.
- **Why there is no fight.** The attack gate now reports itself:
  `military: tick=9000 enabled=true soldiers=30 want=30 seen=0 near=0 going=true`.
  The army is big enough and willing; the house sees no enemy at all, on every
  reading of the match. `Observation::visible_enemy_entities` is a correct L2 read
  and the house simply never leaves its own sight radius.
- **Scouting was tried twice and reverted twice.** Nearest unrevealed cell: the
  scout walked one step past the base and stopped. Farthest unrevealed cell: six
  of six cells worse, `stuck` up everywhere, because the scouts were given a
  `Move` and could not make it. The order funnel shows no refusals, so the
  question is downstream of admission.
- Next reading, smallest first: take one unit and one `Goal::Move` to a cell
  thirty cells away, in a match, and find where the order stops. `sim::pathfinding`
  exists and the retail policy moves units around the same map, so this is a
  policy-side question — most likely the job being re-decided every term before
  the unit has gone anywhere, which is what the stuck counts suggest.
4. Breach and transport jobs, which unlock the two hard maps. Boarding itself is
   proven; the job is missing.
5. Per-unit decision reasons in the log (unit 1234: job=defend
   target=(80,90) reason=outnumbered), and a replay of one match, so a "the
   unit did nothing" report is actionable.

## The spec and the design

- autoplay/docs/spec.md: every wish as an observable criterion, with the
  historical failures as counters (H1-H18) and the priority ladder.
- autoplay/docs/design.md: why the layer is shaped this way.
- autoplay/docs/status.md: the measured state, the known gaps, and the reverted
  experiments.

The wish list itself is G:/Documents/autoplay-v2.txt (the user's file; it is not
in the repository).

## House rules that matter here

- The contract is AGENTS.md. Run cargo through python -m tools.cargo_run --;
  every cargo test uses --lib; format edited leaf files only.
- Retail behaviour is the authority: a change cites the native function and
  address next to the code, and the mechanism must already exist in the engine
  before the policy uses it.
- One owner per state. The policy owns its jobs; the engine owns the
  simulation. Do not add a second copy of a decision.
- Determinism: integer maths in decisions, stable iteration, no f32.

## The Yuri house had no refinery it could want

`Roles::REFINERY` came from `Refinery=` alone. `[YAREFN]` does not carry that key;
it is declared by `ResourceDestination=yes`. The Yuri refinery therefore read as
`harvester,deployer,armed,structure`, the build order's `!has(REFINERY)` test was
permanently true, and its `or(power)` fallback fired every term: sixteen bio
reactor placements, power 2400, no ore income, own 29 at 12000 ticks against
America's 46.

Gating the same key on "is a building" is what makes it correct rather than too
broad: `[SMIN]`, the slave miner, is the deployed refinery's vehicle form and
carries the key too, and it is a harvester. After the fix: yuri 45, america 46,
russia 70.

Read this before trusting any faction number recorded before it: everything in
this file above about factions was measured on a build where Yuri could not build
a refinery.

## Added after this was written (same day, later session)

The compare mode above is built: `tools/autoplay_check.py --baseline <file>`
diffs every cell of a run against a recorded file and prints `worse` when own
falls, a seed that survived is lost, unowned rises or stuck rises; it also warns
when the opening (credits/units/tech/roster) moved, because then the two readings
are not the same experiment. `--compare-only` re-reads two files with no build and
no matches. Tests are in `tools/tests/test_autoplay_check.py`.

A baseline is recorded at `target/autoplay-results/baseline.jsonl` (Death.mmx and
Barrel.mmx, seeds 0x5EED/19/10, 12000 ticks); a second run of the same command
read identical in all six cells, so a diff is now evidence about the policy.

Two things that change the shape of work item 1:

- The object ceiling at 12000 ticks is money, not combat. With
  `--start-credits 30000` the same two maps read own 85 (against 35 and 32 on a
  10000 start). Five of the six baseline cells sit at 12 credits with the wallet
  frozen for the last 3600 frames.
- The income is dead because the harvesters do not unload. In the 30000-credit
  Death run, seven miners log `ReturnToRefinery → Dock` 527 times and arrive at
  ore with an empty hold 14 times; the retail brain on the same map and seed does
  the reverse (96 empty arrivals against ~93 dock entries). The engine's own state
  log, so the numbers are not the policy's opinion.

Before changing the reserves, the harvest cycle is the reading to move: the
policy re-sends `Goal::Harvest` (a MegaMission with a destination) every term the
miner's mission is not Harvest, and during Enter/Unload it is not. Native rules
worth reading first: `new_order_pending` (`src/sim/miner/refinery_dock.rs:420`,
`0x0073E539`) and `per_cell_release_dock_contact` (`0x0073ACD7`). Whether that is
the cause is not yet proven; the ratio above is what decides it.

Then it was fixed: `Policy::living_out` counts `Harvest`, `Enter` and `Unload` as
the same job being lived out, so a harvester is ordered again only when it is
idle. The same baseline then reads own 46/51/50 on Death.mmx (was 35/35/35) and
46/46/50 on Barrel.mmx (was 32/34/35), no seed lost, unowned still zero; empty
arrivals at ore rise from 10-16 to 26-35 per match and Dock entries fall from
166-172 to 26-32, so one dock now means one unload. The `stuck` clause reads 1 in
three cells instead of 0 and status.md records why: every stuck unit is a
harvester pausing for a few seconds, and the counter resets between readings.
Follow-up worth its own commit: `stuck` should not count a harvester that is
filling its hold, which the projection cannot tell from a unit that is frozen.

The forty-minute matrix on that build: six of six cells survive, where four of
six did before, and both seeds that used to be lost (Death.mmx 10 at 17680,
Barrel.mmx 10 at 20435) now finish at 24000 with own 61 and 70. Every cell is
still a timeout, so the ladder's wins need the army slice rather than the
economy.

The attack flag was then probed once more, at parity rather than at two-to-one:
all six cells worse, own down in four, and the stuck count 13-33. The reading is
the army standing around one target cell with `job=Assault`, which is the
concrete starting point for item 2 above.

Since then the army aims legally and at an object. `Observation` gained
`is_cell_visible` and `visible_enemy_entities` (L2 only; the omniscient list stays
where it is, unused by the policy), the military proposes `Target::Object`, the
policy maps it to `Goal::Attack` — the click a player makes on an enemy — and a
job ends when its object is no longer visible. With the flag off the matrix is
identical in all six cells, so all of it is inert until attacking returns. With
the flag on, own falls in no cell and gains in one (46→49 on Barrel.mmx seed 19),
the enemy counts do not move, and the stuck clause fails on five units of
Barrel.mmx seed 0x5EED that stand 1400-1875 frames on `job=Assault
target=Object(2579)`: the army has nothing that gives up on a target it cannot
reach, and the policy does not re-send an order once a unit's mission reads
`Attack`, so a re-aimed job changes nothing for that unit.
