# Where the policy stands

Measured 2026-11-02 on the v2 branch (commit 2490dbeb2), retail YR, hard
computer. One process per map and seed, run side by side:

    python -m tools.autoplay_check --brain v2 --seeds 0x5EED,19,10 \
        --maps Death.mmx,Barrel.mmx --ticks 24000

## What runs

A match driven by this crate: the maker deploys, the house raises a power plant
to open the tree, a refinery, a barracks and a war factory, places each through
the native site search, keeps one harvester per refinery, queues the cheapest
armed unit its factories can build, and holds the army at home until it is twice
its starting size and outnumbers what stands near its target three to two. Every
unit that can take an order holds one, every order goes through the funnel the
player's clicks use, and every decision is read off the unit profile rather than
a type name.

## Numbers

1v1 hard, Death.mmx, 12000 ticks, three seeds (own vs enemy objects at the end):

| Seed | Own | Enemy | Unowned | Stuck |
|---|---|---|---|---|
| 0x5EED | 35 | 43 | 0 | 1 |
| 19 | 35 | 43 | 0 | 1 |
| 10 | 24 | 48 | 0 | 1 |

24000 ticks, both maps, three seeds: five of six survive the forty-minute mark.

| Map | Seed | Result |
|---|---|---|
| Death.mmx | 0x5EED | lost at 17086 |
| Death.mmx | 19 | survives (25 vs 66) |
| Death.mmx | 10 | survives (14 vs 56) |
| Barrel.mmx | 0x5EED | survives (22 vs 63) |
| Barrel.mmx | 19 | survives (20 vs 69) |
| Barrel.mmx | 10 | survives (29 vs 45) |

1v7 hard, Death.mmx, 12000 ticks, three seeds: two of three survive the
twenty-minute mark (0x5EED 16 own vs 299; 10 27 vs 294; 19 lost at 11103). The
retail policy on 0x5EED lost at 11361.

## Known gaps

- The army still only defends or attack-moves; there is no retreat, no
  reinforcement, no focus fire, no defence construction.
- The economy stalls on some seeds: a refinery queued near the wallet floor can
  leave the house with zero income and no way back. Three attempts to fix this
  made the matrix worse and were reverted.
- One harvester loops (stuck=1 on every run): a truck alternating SearchOre and
  ReturnToRefinery without docking.
- The shipyard counts as a war factory. Splitting the role improved Death.mmx
  and lost every Barrel.mmx seed, so the split was reverted.
- Nothing yet for walls, transports, garrisons, superweapons, scouting, or
  defence placement.

## How a change is judged

A change is kept when the matrix improves: own count up at a fixed tick, no seed
lost that survived before, unowned zero, stuck not up. One change per
measurement. Three of the last four experiments (shipyard role, refinery
reserve, truck-first ordering) each improved one reading and lost seeds on the
other map, and were reverted.
