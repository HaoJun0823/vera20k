# The work queue

Reorganised on 2026-11-02 after the v2 policy reached a measurable state. One
line per item, with the reading that accepts it. The detailed chronicle is in
status.md; the design is in design.md; the acceptance criteria are in spec.md.

## Where the work stands

Measured, retail YR, hard computer, six-cell matrix (Death.mmx and Barrel.mmx,
seeds 0x5EED / 19 / 10, 12000 ticks), the flag off:

| Reading | Value |
|---|---|
| own (Death / Barrel) | 46 / 51 / 50 and 46 / 46 / 50 |
| enemy | 45 / 45 / 46 and 41 / 39 / 49 |
| unowned | 0 in every cell |
| stuck | 0 in every cell (since the cargo-aware counter) |
| lost | 0:0 in every cell: neither side loses an object |
| 24000 ticks | six of six survive |
| 1v7, 12000 ticks | two of three survive |

The policy builds an economy, keeps the harvesters working, and holds the army
at the base. It does not attack, and nothing fights: the loss counter is zero
everywhere, which is why an object count cannot tell a strong policy from a
hiding one.

**Since then: the army campaigns.** It goes out, fights, and a unit that has
finished its fight far from home walks back before it is given another job.
With the flag on, the lane kills five enemy objects on Death.mmx seed 19 and
two on seed 24301, and the base no longer falls (Barrel.mmx 19 own 37 -> 45).

**And the judge reads strength, not a count.** The house's fighting strength in
hit points is the reading now, the unit picker buys the unit with the most
damage per credit against the armor it can see, and two trucks on a refinery
put the second refinery and the first defences on the map. The house's strength
over the six cells: 6657 -> 7092 on average, four cells up by six to
thirty-four percent, two cells down by six and seventeen percent (the same seed
on both maps).

## M1: the build you can play

The target for the next build, and what "done" means for it:

1. **A playable build.** fast profile, client, VERA20K_BRAIN=v2 selects the
   new policy, the retail policy stays the default.
2. **A visible opening.** The house deploys its maker, raises a plant, a
   refinery, a barracks and a war factory, places them, and the harvesters
   work. The unit count should reach the high forties by 12000 ticks, not the
   thirties.
3. **Every unit explainable.** The log names each unit, its job and how long
   it has stood still. A unit that stands still has a line that says why.
4. **The three switches real.** Production off stops building, forces off stops
   the army, abilities off stops the superweapons. The board gates the
   commands; the HUD line names what is on.
5. **The army does not throw the base away.** The attack flag stays off until
   the column can come back (item B1).

Acceptance for M1: the client runs, the opening is visible in one match, and
the log answers "what is every unit doing" without reading the code.

## The queue

### A. Visibility (cheap, and every later item depends on it)

| Item | Work | Accepts when |
|---|---|---|
| A1 | Per-unit decision log: id, type, job, target, standing frames | done (this branch) |
| A2 | The compare tool prints both commits and marks a mixed comparison | the diff names the code it compared |
| A3 | The matrix runs two lanes: the flag off (baseline) and the flag on (experiment) | the army branch has a live reading every run |
| A4 | A page for the player: keys, switches, log fields, what to look for | playing-v2.md exists |

### B. The army that goes and comes back

| Item | Work | Accepts when |
|---|---|---|
| B1 | A column that returns: a retreat condition and a way home | done: own 37 -> 45 on Barrel.mmx 19, five cells the same, none worse |
| B2 | A give-up rule for an object target the column cannot hurt | partly: the object ladder exists; the cell ladder is answered by holding the ground |
| B3 | Re-enable the attack flag | done: the flag is on, and the lane kills |
| B4 | A frontier cell the column can actually walk to | answered the other way: the column holds the ground it took, and a reachability test changed nothing |
| B5 | Why the second truck costs two cells on one seed | the timing accident is not explained; it is the same seed on both maps |

Measured so far, all reverted: a garrison of twelve (the base survives, the
kills go to zero), a proportional garrison (worse), skipping a frontier cell
the column already failed (one cell worse), a recall on a visible raid (the
base still falls, the army oscillates), and engaging raiders inside the base
(one cell down eleven objects). The trade is measured: the army that kills is
the whole army. B1 is not a smaller garrison; it is a column that comes back.

### C. The strategy above the army

| Item | Work | Accepts when |
|---|---|---|
| C1 | A phase ladder: opening, expansion, pressure, decision | the phase comes from the state, not a constant |
| C2 | Composition by role: front line, ranged, anti-air | two or more roles in every army; the faction spread narrows |
| C3 | Priority between objectives (survival, economy, technology, expansion, harassment) | one table, no constants in two places |

### D. The house that grows

| Item | Work | Accepts when |
|---|---|---|
| D1 | A second refinery that lands | the wallet stops freezing at 12 credits |
| D2 | Walls and defence placement | xeb1 opens a wall; the base has a ring |
| D3 | Breaches and transports | xtsunami lands units on another island |
| D4 | Scouting | something sees the enemy before the army commits |

### E. The abilities

| Item | Work | Accepts when |
|---|---|---|
| E1 | Superweapon use: the target is worth it | the launch count is non-zero and the target is a base |
| E2 | Infiltration: factory, refinery, laboratory, superweapon | each use is counted |
| E3 | Harvester micro: high-value ore, no raiding | the harvester's ore value per minute rises |

## The rule that decides

One change per measurement. Six cells, or three at 1v7. Keep the change when
own does not fall, no surviving seed is lost, unowned stays zero and stuck
does not rise. A reading that only says "own is higher" is not a win: with
lost at 0:0 everywhere, own counts the economy, not the fighting.
