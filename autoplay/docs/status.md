# Where the policy stands

Measured 2026-11-02 on the v2 branch (commit 7bdbb42e9), retail YR, hard
computer. One process per map and seed, run side by side:

    python -m tools.autoplay_check --brain v2 --seeds 0x5EED,19,10 \
        --maps Death.mmx,Barrel.mmx --ticks 24000

## What runs

A match driven by this crate: the maker deploys, the house raises a power plant
to open the tree, a refinery, a barracks and a war factory, places each through
the native site search, keeps one harvester per refinery, queues the cheapest
armed unit its factories can build, and holds the army at home until it is twice
its starting size. Every unit that can take an order holds one, every order goes
through the funnel the player's clicks use, and every decision is read off the
unit profile rather than a type name.

## Numbers

1v1 hard, Death.mmx, 12000 ticks, three seeds (own vs enemy objects at the end):

| Seed | Own | Enemy | Unowned | Stuck |
|---|---|---|---|---|
| 0x5EED | 29 | 39 | 0 | 1 |
| 19 | 27 | 42 | 0 | 1 |
| 10 | 29 | 48 | 0 | 1 |

24000 ticks, both maps, three seeds: 3 of 6 survive the forty-minute mark.

| Map | Seed | Result |
|---|---|---|
| Death.mmx | 0x5EED | survives (25 vs 66) |
| Death.mmx | 19 | lost at 12140 |
| Death.mmx | 10 | survives (26 vs 55) |
| Barrel.mmx | 0x5EED | lost at 20494 |
| Barrel.mmx | 19 | lost at 20053 |
| Barrel.mmx | 10 | survives (26 vs 47) |

1v7 hard, Death.mmx, 12000 ticks, seed 0x5EED: survives, 12 own against 312.
The retail policy on the same seed lost at 11361.

## Known gaps

- The army has no judgement. It holds until 24 units, then attack-moves the
  whole group at the nearest enemy object. There is no local-superiority test,
  no retreat, no reinforcement, no focus fire.
- The economy stalls on some seeds. A refinery queued near the wallet floor can
  hold the building factory for minutes; the economy log line is the reading.
- One harvester loops: a truck alternating SearchOre and ReturnToRefinery
  without docking is the stuck=1 in every run.
- The shipyard counts as a war factory. Making it a separate role improved the
  short game on Death.mmx and lost every Barrel.mmx seed, so the split was
  reverted; the trade-off is unresolved.
- Nothing yet for walls, transports, garrisons, superweapons, scouting, crates,
  or defence placement.

## How a change is judged

A change is kept when the matrix improves: own count up at a fixed tick, no seed
lost that survived before, unowned still zero, stuck not up. One change per
measurement: the shipyard experiment is what happens otherwise.
