# Handoff

Where the v2 policy stands at the end of the session that took it from a dog
army to a composed one. The chronicle is status.md; this is what to read first
and what to do next.

## The state

Branch `feature/autoplay-v2`, thirteen commits from `autoplay: the army is
composed from the role model` to `autoplay: two reverted experiments, and what
they point at`. Every one is measured; every reverted one is recorded with its
numbers.

- **1v1, six cells, 12000 ticks** — six of six better than the state the session
  started from (`eval-v2-fixed.jsonl`): own +37 total, army +5867 total, no cell
  worse, `stuck` down where it moved at all.
- **1v1, six cells, 24000 ticks** — six of six survive. own averages 54, army
  6787, 63 lost for 35 killed. `Barrel.mmx` seed 19 is the weak cell
  (`own 39, army 4045, lost 19:7`).
- **1v7, three seeds, 12000 ticks** — three of three survive, own 43/46/42,
  army 6439/6200/5193, no loss on two seeds and 11:0 on one. 24000 ticks is
  still lost (12418-15952): seven brutal houses hold 324-369 objects.

## What changed and why it matters

1. **`rules::unit_role` was scoring `NeverUse=yes` weapons.** The `VirtualScanner`
   every dog carries as `Secondary=` gave the attack dog 0.24 anti-vehicle and
   0.30 anti-air weight off a warhead it can never fire. Fixed at the owner, so
   the retail brain gets the right answer too.
2. **`rules::unit_role::is_army` is the one owner of "is this army"**, and the
   retail census asks it too. `Roles::AI_ONLY` is gone: `NotHuman=yes` does not
   mean unbuildable — every dog is `NotHuman` with `Prerequisite=Barracks` — so
   a mod unit marked that way would have been dropped.
3. **The autoplay profile no longer sums every weapon slot.** It reads the slots
   native fires and never a `NeverUse` entry, and `Armed` comes from the engine's
   `Capabilities`.
4. **The picker composes against the mix the rule set offers**, not against a
   threat vector that is empty on every reading.
5. **The army fights the raid it can see and marches only with a column**, where
   the column size is a fact about contact rather than a constant.

## What to do next, in order

1. **`Barrel.mmx` seed 19 at 24000 ticks.** `own 39, army 4045, lost 19:7` — the
   weakest cell on either lane. Read the log for what it is losing to first.
2. **The army never finds the enemy.** `seen=0` on every reading for the whole
   24000-tick match, and the column wanders its own side of the map (tick 23400:
   `order=Some(Cell((136, 64)))` against an enemy base at `(70, 157)`). Ranking
   the frontier by distance instead of proximity is three cells better and three
   worse; the target is re-chosen every term from the column's first unit, so the
   column oscillates between opposite edges of the explored region. That is
   queue.md D4, and it is the largest single lever left.
3. **Artillery against a fortress.** The 1v1 enemy builds almost nothing but
   defences. `HOWI` is buildable from the first war factory and the picker never
   buys it, because the only source of artillery demand is the roster mix. The
   first attempt (a demand for every armed enemy building in sight) is inert
   because the army never gets close enough to see one — it becomes live once
   item 2 is answered.
4. **Income.** Three trucks for two bays, wallet at 300-900 credits, and every
   reserve rule tried against it has lost to the army (status.md: three times).
   A cheaper truck, a closer ore field, or ore worth more per trip — not another
   saving rule.

## The rule that decides

Six cells, or three at 1v7. Keep a change when own does not fall, no surviving
seed is lost, unowned stays zero and stuck does not rise — and read `army` and
`lost` beside `own`, because a change that raises the object count while killing
nothing is a change that made the house hide. Every measurement in status.md
names the file it was compared against.
