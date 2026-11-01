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

## The hard maps, measured before the assignment fix

| Map | Own | Enemy | Stuck | Reading |
|---|---|---|---|---|
| xcc_maps/xeb1.map | 30 | 27 | 1 | holds its side; no wall opened (walls=0) |
| xcc_maps/xtsunami.map | 35 | 42 | 28 | the army is frozen: the target is on another island |

The island reading is the clearest failure the policy has: twenty-eight units
holding a move job and not moving. A reachability ladder was tried and reverted
(two minutes of "this target is unreachable" made the wall map worse), so this
is still open.

## Known gaps

- The army does not attack. Attacking is behind a flag that is off, because an
  army that attacks at two-to-one loses three matches of six. Local
  superiority, retreat and reinforcement are the next slice.
- The economy stalls on some seeds: a refinery queued near the wallet floor can
  leave the house with zero income. Three attempted fixes made the matrix worse
  and were reverted.
- One harvester loops now and then (stuck=1): a truck alternating SearchOre and
  ReturnToRefinery without docking.
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
