# The auto play layers

VERA20k's agent stack has three layers with one-way dependencies. The point is not
tidiness: it is that a new behaviour, a new decision or a new *kind* of decision can be
added without editing the modules around it, and that a learned controller can replace
exactly one of the three.

Player-facing behaviour — the hotkeys, the two switches, the actuation and timing rules
— is in [autoplay.md](autoplay.md). This page is the internal contract.

```
app / hotkeys ──▶ sim::autoplay            facade: install, step, switches, HUD name
                       │
                       ▼
                  sim::strategy            L3  decide     (Brain, RetailBrain)
                       │
                       ▼
                  sim::control             L2  store, schedule, deliver
                       │
                       ▼
                  sim::behavior            L1  execute
                       │
                       ▼
   mission / combat / aircraft / movement / production   the native owners
```

## Responsibilities

| Layer | Answers | Must not |
|---|---|---|
| **L1 `sim::behavior`** | What does *this object* do about this order? Is the order issuable yet? Where is the nearest threat? What does a managed routine do this phase? | Name L2 or L3. Decide anything for the house. Write simulation state. |
| **L2 `sim::control`** | Who decides this frame, what is admitted, what the budget is, where a managed phase lives, what a decision may see. | Name L3. Decide gameplay. Keep a second copy of a native rule. |
| **L3 `sim::strategy`** | What to build, raise, send and take. | Write state, name L1, or issue a `Command` directly. |

`architecture_guards.rs::sim_ai_layers_depend_one_way` checks the arrows over
production source: `sim/behavior/` may not name `sim::control` or `sim::strategy`,
`sim/control/` may not name `sim::strategy`, and `sim/strategy/` may not name
`sim::behavior`. The goal vocabulary is re-exported from `sim::control` precisely so
L3 never needs to.

## Module inventory

| Module | Layer | Job | State |
|---|---|---|---|
| `behavior::goal` | L1 | `Goal` vocabulary; one variant per action, mapped to the `Command` the same click produces | none |
| `behavior::order` | L1 | the single funnel: existence, `mission_ready_to_commence`, `Queued`/`NotReady`/`Rejected` | none |
| `behavior::view` | L1 | `ObjectView` (own range, reload, mission, health, ammo limit) and `ThreatView` (radius query) | borrowed |
| `behavior::managed` | L1 | `MicroRoutine` contract, `Phase`, `MicroStep`, `FallbackReason`, cell helpers | none — pure |
| `behavior::managed::{kite,spy_approach,air_split,retreat}` | L1 | one micro behaviour each | none |
| `control::board` | L2 | frame order, gate, domain switches, envelope construction | owned, per house |
| `control::brain` | L2 | `Brain` and `Outbox`: the port a decision source implements | none |
| `control::budget` | L2 | per-frame command allowance and the micro rotation cursor | owned |
| `control::micro` | L2 | managed table: kind, params, phase, refusal count; lifecycle and GC | owned, **not** simulation state |
| `control::intent` | L2 | `Order` and `Intent`; intents applied through their native owners | none |
| `control::placement` | L2 | `find_base_building_site` first, bounded outward sweep as the fallback | none |
| `control::observation` | L2 | the read-only projection a decision sees | borrowed |
| `control::external` | L2 | reserved: `PolicySource`, versioned `ObservationBatch`, `ActionSlot` | none |
| `strategy::brain` | L3 | re-export of the port, with the direction documented | none |
| `strategy::retail` | L3 | the retail brain: plan, building choice, unit choice, teams, army orders, yard deployment | owned |
| `autoplay` | facade | `AutoPlayHost`, `AutoPlayDomains`, `ScriptedPolicy`, the hotkey-facing surface | host |

## Data ownership and lifecycle

- **One issuer per object.** A brain's order naming an object that is under a managed
  routine is dropped before admission; the routine is that object's only issuer.
- **The managed table is not simulation state.** It never enters a snapshot, a
  checksum or a replay. Its routines are pure functions of `(object, threats, phase)`
  and its output is commands, so a replay reproduces it from the command stream. A test
  steps two tables with the same input and compares both the steps and the phases.
- **Lifecycle is L2's.** An entry is dropped when the object dies, leaves the map, is
  captured, boards a transport or is mind-controlled, and when a team script takes it
  over — a team's members already have an issuer (`TeamClass::Recruit` clears their
  target). The table is garbage-collected once per frame before stepping.
- **Brain memory is the brain's.** The plan, the generator, the outstanding orders and
  the give-up counters live in `RetailBrain`, rebuilt on install, not serialized.
- **The projection is a borrow.** Nothing is copied or retained; a decision that needs a
  value across frames keeps the value.

## Direct and managed modes

**Direct** is the default: the brain decides, the control layer translates the order and
the native mission machine runs it to the end. No per-object memory exists.

**Managed** attaches an object to a routine. Each frame the routine returns a goal, a
wait, `Done` or `Fallback`; the board issues the goal through the same funnel and counts
refusals. After `Managed::MAX_REFUSALS` consecutive not-ready answers the object is
handed back to the brain and the hand-back is logged — a routine that cannot act must
not hold an object silently.

**Every routine in this build is non-parity.** Kiting, waiting out a patrol, splitting a
flight's fire and breaking off a hurt unit are things a player does by hand;
`gamemd`'s computer houses do not, and no golden or native comparison covers them. They
are enabled only for a house the retail brain drives, never for a scenario's own AI
house. Each module says so in its header. Where a routine's input is narrower than the
game's — `SpyApproach` reads proximity rather than vision and disguise, `Retreat` has no
per-shot ammunition counter to read — the module records that as a residual instead of
inventing the missing rule.

## External control seam

`control::external` fixes what an out-of-process controller would speak. It implements
no transport: no socket, no shared memory, no `net` dependency, no scheduling.

- `ObservationBatch` — one fixed-length integer tensor per frame: `globals`
  (`GLOBAL_SLOTS` values) and `entities` (`rows × ENTITY_SLOTS`, row-major). Rows follow
  stable entity id, so row `r` names the same object on both sides.
- `ObservationSchema::current()` — the layout, with a `version`. A version change is
  breaking: a controller pinned to an older version must be rejected, not adapted.
- `ActionSlot { row, goal, target_row, cell }` — batched actions, resolved against the
  batch they answered, so one call can speak for many objects.
- Determinism — a batch is built from the same integer state the in-process brain reads,
  in the same order, so replaying a command stream reproduces the batches that produced
  it. Batches are built only when a source is installed.

## Evidence

| Claim | Level |
|---|---|
| The plan, the choosers, the placement search, the team block and the production intents | native behaviour, ported and pinned by the retail tests and the `sim::ai_*` oracles |
| The gate, the budget, the envelope and the frame order | Rust regression tested (`autoplay_tests.rs`) |
| Managed routines | **not parity** — Rust regression tests only |
| The external seam | type-level and determinism tests only; nothing is transported |

## Recorded gaps

Native mechanisms this layer does **not** drive yet. Each needs its owner split into a
read-only decision and an apply half before a brain may call it; none is stubbed.

- `AI_Manage_Build_Queue` (`0x004FDD10`) and its credit-cutoff mode: selling, splicing
  and replanning under economic pressure.
- `AI_ChooseNextProduction` (`0x00506EF0`): base defenses. It is a mutation that also
  draws the Scenario RNG, so it cannot be called read-only today.
- The superweapon decision, the enemy-house search, the fire-sale and all-to-hunt
  reactions, and the parachute/air-strike house powers.
- Base expansion beyond the first yard.
- Fog honesty: the driven house sees the whole map, like the computer AI it borrows from.
