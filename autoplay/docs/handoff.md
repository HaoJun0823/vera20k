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

1. **`Barrel.mmx` seed 19 at 24000 ticks** — resolved. The centroid anchor (the
   column marches from where the column is, not from `soldiers.first()`) took it
   from `own 39, army 4045, lost 19:7` to `own 80, army 9804, lost 2:0`. It was an
   attrition the house could not pay for, and marching further let it meet the
   enemy in the field instead of at its own door.
2. **The army now finds the enemy, but does not march to its base.** The centroid
   anchor fixed the wandering: with `soldiers.first()` the frontier was searched
   from home every term, but the mean of the column's cells lets it walk outward
   and it now engages enemies in the field (Barrel.mmx seed 19 reads `seen=9` at
   tick 21600). Marching *directly* at the enemy base was tried and rejected — it
   sends the column into the fortress (tesla coils, walls, no siege of its own)
   and three Death.mmx cells lost half their army. So the army meets the enemy in
   the field, which is the favourable ground; pushing all the way to the base
   needs a siege unit first (item 3).
3. **Artillery against a fortress.** The 1v1 enemy builds almost nothing but
   defences. `HOWI` is buildable from the first war factory and the picker never
   buys it, because the only source of artillery demand is the roster mix. The
   attempt (a demand for every armed enemy building in sight) is inert twice over:
   the army still never reaches the enemy base buildings (it meets units in the
   field, not the fortress), so `seen` never contains a building. It becomes live
   only once item 2's base march works — which needs a siege unit so the column
   survives the trip.
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
