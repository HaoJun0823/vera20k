# Handoff: the auto-play rewrite

Written 2026-11-02 for an agent taking this over. Everything below is measured
on this branch; nothing here is a plan that has not been run.

**Read this file, then `autoplay/docs/status.md`.** status.md holds the
measurements in the order they were taken, including the ones that failed; this
file holds the state and the queue. The single most important thing on either page
is under "Start here" below, because the last four rounds were spent discovering
it and the next agent should not spend four more.

## Start here

The deadlock is closed and a sharper question replaced it. Read this section, then
`autoplay/docs/status.md`, then the numbers in `target/autoplay-results/front2.jsonl`.

**What was wrong.** `commit` only proposes an assault when `visible_enemy_entities`
has something in it; `seen` is zero on every reading of every match because the two
bases sit (169, 91) and (70, 157) apart — 116 cells with nothing walking between
them. So the army never moved, the enemy count never fell, and the army held. That
is measured, and four rounds went into finding it.

**What fixed it.** With no enemy in sight, a big-enough column marches at a cell on
the **edge of known ground** (the frontier), inside the playfield, within one march.
Three readings had to be right first: `Policy::job_achieved` (arrived is not stuck),
the frontier rather than the farthest unexplored cell (deep cells sit behind walls
and cannot be walked to), and `ACHIEVED_CELLS = 3` (a column arrives spread out).
All three are committed.

**What the matrix said.** Six cells, 12000 ticks, with the flag on:

| Map | Seed | own | lost | stuck |
|---|---|---|---|---|
| Death.mmx | 24301 | 46 → 46 | **0:8** | 0 |
| Death.mmx | 10 | 50 → 51 | 0:0 | 1 → 2 |
| Death.mmx | 19 | 51 → 51 | 0:0 | 0 → 1 |
| Barrel.mmx | 10 | 50 → 50 | 0:0 | 0 → 1 |
| Barrel.mmx | 24301 | 46 → 46 | 0:0 | 0 |
| Barrel.mmx | 19 | 46 → 37 | **7:0** | 0 → 1 |

Four worse, one better, one same — so `ATTACK_ENABLED` is **false** at HEAD and the
baseline is untouched (verified: six of six identical to `inert.jsonl`). Flipping
the flag to true reproduces `front2.jsonl`.

**The question that is open now.** In Barrel.mmx seed 19 the column left and the
base was destroyed: `soldiers` fell 32 → 22 between tick 9000 and 10800, `lost`
`7:0`. Everywhere else own holds or rises, and in Death.mmx 24301 the column
destroyed eight enemy objects. So an army that leaves wins fights and loses its
base. **What stays home is the next decision**, and `lost` is now a reading that can
see it. The three `stuck`-only cells do not count against the march: both counted
units in them are harvesters shuttling ore, not soldiers.

Do not re-derive any of this. Six attempts at the army were reverted before this
one; status.md has each with the number that rejected it.

## Where the work is

- Branch: feature/autoplay-v2 (fork HaoJun0823/vera20k, upstream
  YuriPlanet/vera20k). upstream/main was merged in (204 commits) and one
  merged call site fixed.
- HEAD at handoff: `f93386790`, working tree clean, everything local (nothing
  pushed; publication was never authorized).
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
- holds the army at its post. `ATTACK_ENABLED` is false at HEAD; the march, the
  frontier search and the arrival reading are all committed behind it and measured
  in `front2.jsonl`;
- every fact about a type comes from autoplay/src/profile/mod.rs, derived from
  RULESMD.INI through the production reader. No type name in the policy.

Nothing scouting-shaped is in the tree. Six scout attempts were measured and
reverted before the army march replaced the idea; status.md has each with its
numbers.

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

The recorded baseline is `target/autoplay-results/inert.jsonl` (commit `b08fa928`,
12000 ticks, america, `--baseline` compares against it):

| Map | Seed | own | enemy | unowned | stuck |
|---|---|---|---|---|---|
| Death.mmx | 10 | 50 | 45 | 0 | 1 |
| Death.mmx | 19 | 51 | 45 | 0 | 0 |
| Death.mmx | 24301 | 46 | 46 | 0 | 0 |
| Barrel.mmx | 10 | 50 | 41 | 0 | 0 |
| Barrel.mmx | 19 | 46 | 39 | 0 | 1 |
| Barrel.mmx | 24301 | 46 | 49 | 0 | 1 |

Every cell reads `result=timeout at tick 12000`: both houses are still standing,
which is the survival the keep rule judges. **Neither side loses an object in
12000 ticks** — the `lost` column is `0:0` on every cell, which is the fact the
whole army queue rests on.

24000 ticks over the same maps: six of six survive (before the harvester
re-ordering fix it was four of six). 1v7 at 12000 ticks: two of three survive (the
retail policy lost at 11361 on the same seed).

Faction sweep, Death.mmx 0x5EED, 12000 ticks, v2: **america 46, russia 70, yuri
45**. The 24-object spread is the "cheapest armed unit" picker, which quietly means
"each faction's best-value unit", so every army ends up one type — that is queue
item 3.

Hard maps: xeb1.map holds its side and opens no wall; xtsunami.map never leaves
the starting island. Both are expected failures until the breach and transport
jobs exist.

## What has been proven about movement (the last four rounds, condensed)

Every one of these has a test in `autoplay/src/`, and none of them needs re-deriving:

| Question | Answer | Test |
|---|---|---|
| Does one `Goal::Move` reach the engine? | Yes, 20 cells, through `Funnel::prepare` | `move_tests.rs` |
| Through the real board with budget and `order_stands`? | Yes, 20 cells, `rejected=0` | `board_move_tests.rs` |
| Does a unit on a guard post still take a move? | Yes | `guard_move_tests.rs` |
| How far does one march actually get? | About 30 of 160 cells, then stops | `guard_move_tests.rs` |
| Does the fog report an enemy in line of sight? | Yes, within a few frames | `fog_tests.rs` |
| Is the fog grid the playfield? | No — the grid is the whole map buffer | `vision_tests` |
| When is a unit stuck, and when is it finished? | Finished within 3 cells of its named cell | `policy::tests::a_unit_standing_on_the_cell_it_was_sent_to_is_not_stuck` |
| Can a blind march reach a deep unexplored cell? | No — walls and water; aim at the frontier | `front2.jsonl`, status.md |

The two that matter for the queue: **a march covers about thirty cells**, so plan
trip lengths accordingly; and **`seen=0` in a match is honest** — nothing has come
within sight, because nothing walks.

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

**0. Decide what stays home.** This is the immediate next step. The march exists,
is measured and is correct; it is behind `ATTACK_ENABLED = false` because the army
leaving loses the base in one of six cells. `target/autoplay-results/front2.jsonl`
is the run with the flag on, and the six cells are tabulated at the top of this
file.

The measured trade: in Barrel.mmx seed 19 `soldiers` fell 32 → 22 between tick 9000
and 10800 with `lost 7:0` while the column was away; in Death.mmx 24301 the same
mechanism destroyed eight enemy objects and read `same`, not worse.

Candidate rules, cheapest first — try one, measure, keep or revert:

   - **leave a garrison**: when the gate opens, hold back a fraction (a third?) of
     the soldiers on `Defend` at `home` and march only the rest. `commit` already
     builds the soldier list per term, so this is a split of that list, not new
     state;
   - **a commit radius**: march only when the column would stay within N cells of
     the base, so a fight is always near enough to fall back from. The deadlock's
     map has the enemy 116 cells away, so this may simply disable the march — check
     it against `front2` before believing it;
   - **reinforce while away**: keep `ARMY_TO_ATTACK` at the gate but let the army
     keep buying while it marches. It already does; confirm on a reading.

The judging readings are `own` (must not fall below the baseline), `lost` (must
stay non-zero somewhere, or nothing is being decided), and `stuck` (the two
harvester units in the `stuck`-only cells are not the army — do not chase them).

**Do not** re-tune the march itself first: the frontier, `ACHIEVED_CELLS = 3` and
`job_achieved` were each adjusted against measured cases and are now reading
correctly. The remaining defect is strategic, not geometric.

The items below are the older queue, kept for the reasoning. Much of the long
bullet chain under item 3 is a chronicle of rounds that status.md already records;
read it there rather than here.

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
- **The order path is now proven sound and the policy is not.**
  `autoplay/src/move_tests.rs` moves a tank twenty cells with one `Goal::Move`
  through the same funnel a player's click uses. In a match the same goal does not
  move a tank at all: three scout attempts, the last with a reachable destination
  (`Cell((16,16))`) and a remembered target, left tanks at `(178,100)` from tick
  600 to the end, stuck. The `Move` went out once. So what differs is the `Outbox`
  path or the state the unit is in — not the goal, and not the pathfinder.
  The next experiment should be one unit, one move, driven by a `Brain` that does
  nothing else, and watch where it stops; scissors on the `Outbox` drain are the
  most likely suspect.
- **Resolved, and the answer is the opposite of the guess.** `order_stands` and
  the `ControlBoard` are both innocent: `board_move_tests.rs` drives the real
  board, budget and gate included, and a tank arrives twenty cells away with
  `rejected` at zero. Asked directly, `Policy::think` emits 240 orders over three
  hundred frames and **every one is `GuardAt`** — the tree has no code that asks a
  unit to travel, because all the scouting attempts were reverted. The policy is
  not failing to move units; it is not asking.
- **Then the asking was tried a fourth time and the shape of the problem changed.**
  `Observation::is_cell_in_playfield` now exists, because the fog grid covers the
  whole map buffer while walkable ground is smaller: two earlier targets
  (`Cell((0,0))`, `Cell((16,16))`) were cells no pathfinder reaches. With a legal
  target (`Cell((16,128))`) the fourth scout was still worse in six of six cells,
  own falling 46→25 in one, `stuck` up everywhere — taking front-line soldiers off
  the base costs more than the map information is worth.
- **And a march is not unlimited.** `guard_move_tests.rs`: a 160-cell march on a
  200-cell arena starts, keeps walking, and stops after about thirty cells with
  130 to go. So the next scout is not a tank sent across the map; it is a cheap
  unit sent a short way, repeatedly, and the army stays home. Two facts make that
  design testable now: the playfield query, and the measured march length.
- **Five scouts now, and the fifth is the one that matters.** Picking the unit by
  sight per credit off the profile — a dog has `sight=9, speed=8, cost=200` against
  `E1`'s `5/4/200` — and sending it 24 cells at a time to a playfield-legal cell
  reached the enemy: `lost` went `0:0 → 5:0`, the first destroyed objects any
  scout attempt produced. It was still six of six cells worse and was reverted.
- **The reading that does not fit, and the next question.** In that same match,
  every one of seventeen readings shows `seen=0` — units walked into enemy fire and
  the projection reports no enemy in line of sight. `visible_enemy_entities` is
  tested and works in the arena fixture, so the two candidates are: the fog is not
  refreshed for the house in a real match, or the units are killed by something the
  visibility layer does not count (a defence shot, or fire from outside the unit's
  own sight). Settling that is worth more than another scouting rewrite — every
  decision that picks a target reads `seen`, so if it is always zero in a match,
  none of the army work can be judged.
- **Settled, and it is a deadlock.** `fog_tests.rs` puts two riflemen two cells
  apart in the arena and the house's fog reports both the cell and the enemy on it
  within a few frames: the fog and the accessor are sound. What the matches show is
  distance — the bases are at (169, 91) and (70, 157), a hundred and sixteen cells
  apart, and nothing walks between them. So the loop is: the army holds until
  something is in sight, nothing is ever in sight because nothing walks, so the
  army holds. `ATTACK_ENABLED = true` confirms it rather than fixing it: `going`
  turns true at tick 9000 and `seen` stays zero, with own and enemy identical to
  the attacking-off run.
  The next change is therefore to the waiting army, not to targeting: with no enemy
  in sight, the assault should be a march towards ground the house has not seen
  rather than a guard order on the base. The judging reading is `seen` becoming
  non-zero at all, on any term of any match.
- **Tried, and it works.** Marching a big-enough army at the unrevealed cell
  *farthest from home* (the nearest-first version walked the wrong way: east to
  `Cell((184,100))` while the enemy held the west) produced the first real
  engagement in this whole effort: over 12000 ticks on Death.mmx seed 0x5EED the
  column walked about sixty cells, from (169,91) to (152,151), and **destroyed
  seven enemy objects** (`lost 0:0 → 0:7`, enemy 46 → 36) at a cost of two of its
  own. It was reverted anyway — four of six cells worse — but two of those four are
  `stuck=40`, which is the ladder complaining, not the army failing.
  **The next attempt should keep the march and finish the ladder around it.** What
  a column does when it arrives and finds nothing is unwritten: it is re-decided
  every term, so it neither holds the ground it took nor marches on from it. Add
  that — arrive, keep the cell, send the next march from there — and the same
  design should read clean.
- **That was tried and it is the wrong next step.** Remembering the objective
  across terms stopped the advance (enemy 46 → 46, `lost` 0:0): a destination
  chosen once and kept is a column that stands where it was sent. Both versions
  reverted.
- **What three attempts agree on: `stuck` is measuring the wrong thing.** It counts
  a unit that holds a movement job and does not change cell for six seconds, and an
  army column holding ground it took is exactly that. The spec's H2 means a unit
  that *should* be moving and is not; a column standing on its objective is not
  that failure. Three march attempts have now been rejected on this reading alone,
  two of them at `stuck=40` while the same run destroyed seven enemy objects.
  **Fix the stuck definition first**, so a job whose end condition is met — assault
  on the cell it stands on, defend at the post it stands at — ends and is
  re-decided instead of counting against the policy forever. Until then the matrix
  cannot tell a working army from a frozen one, and no army change can be judged.
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

## Picking it up in ten minutes

    cd E:\Projects\vera20k-autoplay
    set RA2_DIR=I:\SteamLibrary\steamapps\common\Command & Conquer Red Alert II

    # 1. the policy's own tests (28 at handoff, all passing)
    python -m tools.cargo_run -- test -p vera20k-autoplay --lib

    # 2. the six-cell matrix, diffed against the recorded baseline
    python -m tools.autoplay_check --brain v2 --seeds 0x5EED,19,10 ^
        --maps Death.mmx,Barrel.mmx --ticks 12000 ^
        --history target/autoplay-results/<name>.jsonl ^
        --baseline target/autoplay-results/inert.jsonl

    # 3. read the matrix back later without re-running it
    python -m tools.autoplay_check --compare-only ^
        target/autoplay-results/<name>.jsonl target/autoplay-results/inert.jsonl

The matrix takes a few minutes. Read `own`, `lost` and `stuck` per cell; the keep
rule is in "Method that worked". `own` alone is not the judge — a change that
raises own while nothing fights is a change that made the house better at hiding.

One caution learned the hard way: `--skip-build` reuses the last binary, so a run
with it measures the *previous* code. Only use it when the tree has not changed
since the last build.

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
