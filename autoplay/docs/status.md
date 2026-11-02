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

- The army does not attack. Attacking is behind a flag that is off, because an
  army that attacks at two-to-one loses three matches of six. Local
  superiority, retreat and reinforcement are the next slice.
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
