# Auto play: scripted command injection

Roadmap for driving the local player's house from an agent. The agent takes the place of
the player's hands, not of the game: it observes what the player would see and issues the
same commands the player's clicks produce, through the command path the game already uses.

Status: design, with the H0 actuation skeleton implemented and tested. See
[Current state](#current-state).

This document specifies the **actuation layer** ("the hands") only: observation, the agent
interface, the host, and the admission gate. It deliberately does **not** specify the
**policy layer** ("the brain") — what to build, when to attack, which enemy to engage.
Hands first. Every decision about policy is listed under
[Deferred: the brain](#deferred-the-brain) and is open.

## Goal and non-goals

Goals:

- An agent drives the local player's house by issuing `Command`s: build, place, move,
  attack, sell, deploy, fire a superweapon.
- Those commands travel the existing deterministic path — `CommandEnvelope` →
  `Simulation::queue_command` → the frame-tail dispatch — so replay, save/load and future
  lockstep are unaffected by the agent's existence.
- The agent is just another command producer. Nothing downstream changes.

Non-goals:

- **No policy.** This document does not choose what the agent decides, and does not port,
  wrap or reuse `sim::ai_*`. That is the brain, deferred.
- **No new gameplay AI.** The existing computer-house AI is untouched by this layer.
- **No direct state writes.** The agent may not touch `Simulation` mutably. Anything that
  writes gameplay state outside `queue_command` is a takeover, not auto play, and is out
  of scope. `sim::cheats::TrainerAction` and `apply_debug_tools` are explicitly excluded:
  they run before every frame phase, are refused in network mode, and close the current
  replay segment (`src/app/match_diagnostics.rs:47-53`).
- **No native parity requirement.** This is a new capability, not a port of gamemd
  behavior. No `gamemd`-derived evidence is needed for the hands.
- **No strict equivalence with human clicks.** The agent addresses entities by id; it does
  not reproduce `What_Action`, screen coordinates, or the sidebar's cameo decision tree.
- **No network transport for the agent.** An out-of-process agent is a later, optional
  stage; the in-process interface comes first so it can be tested headlessly.

## Why hands before brain

1. Every policy candidate needs somewhere to put its hands. Building the policy first
   means building it against an imagined interface and rewriting it later.
2. The hands are cheap and risk-free: new files only, no production behavior changed, no
   owner moved. The brain is not: reusing `sim::ai_*` means splitting decisions from
   their house-state writes, which touches a live owner and every one of its callers.
3. The hands are verifiable on their own — a scripted agent can drive a full opening
   build with no policy at all, and that alone exercises the whole deterministic path.

## What already exists

The three parts that are usually hard are done.

| Piece | Where | State |
|---|---|---|
| Action space | `Command`, `src/sim/command.rs:721` | Complete: build, place, move, attack, attack-move, guard, stop, deploy, sell, rally, superweapon, and more. Semantic, not pixel coordinates. |
| Authority | `Simulation::queue_command(s)`, `src/sim/world/command_schedule.rs:30` | Append-only ingress. Preserves owner, tick and insertion order; does not sort, retime or deduplicate. |
| Replay | `ReplayLog`, `src/sim/replay.rs:725` | Records the command stream plus a state hash every 15 frames (`PLAY_HASH_INTERVAL`, `replay.rs:695`). An agent needs no new recording: its commands are already commands. |
| Stable references | `GameEntity::stable_id()`, `src/sim/game_entity.rs:1270` | Entities keep their `u64` across frames, so an agent can hold a unit across frames without its own mapping. |

Building blocks the observation layer will read, all already ported:

| Query | Where |
|---|---|
| What a house may build | `can_build`, `src/sim/production/can_build.rs:80` (native `HouseClass::CanBuild @ 0x004F7870`) |
| Where to put a building | `find_base_building_site`, `src/sim/ai_base_site.rs:146` (native `FindBaseBuildingSite @ 0x005060B0`) — read-only `&Simulation`, usable as-is |
| Whether a cell is visible | `Fog::is_cell_visible`, `src/sim/vision/mod.rs:1571` |
| Which house the player is | `ScenarioSession::current_house`, `src/sim/scenario_session.rs:839` |

Timing the agent must be written against:

- `execute_tick` is the current tick when a command is issued
  (`src/app/input/commands.rs:1036`), and a command is due when
  `execute_tick <= tick + 1` (`take_due_commands`, `command_schedule.rs:154-166`).
- Ordinary commands execute in the frame tail, inside `run_late_region`
  (`apply_due_commands`, `command_schedule.rs:676`).
- So: **a command issued before frame N's drain is drained by frame N and takes
  effect in frame N's tail** — the host runs immediately before the drain, so
  the agent's answer reaches the frame it was given. An agent must still keep
  its own waiting state: nothing in the simulation remembers what it asked for.

## Current state

| Area | State |
|---|---|
| `src/sim/autoplay/observation.rs` | Implemented: `AutoPlayView` with credits, power output/drain/balance, own entities, `max_health`, ready buildings, ground walkability. Each field reads its authoritative owner. |
| `src/sim/autoplay/agent.rs` | Implemented: `AutoPlayAgent` and `ScriptedAgent`, a fixed `(frame, Command)` schedule. |
| `src/sim/autoplay/host.rs` | Implemented: `AutoPlayHost::step` — observation → agent → gate → `queue_commands`. The gate reuses `Simulation::entity_owned_by_id` and `Simulation::order_actor_admits` rather than restating them. |
| `src/sim/autoplay/autoplay_tests.rs` | Implemented: 10 tests over the envelope, the gate's three rejections, the per-frame cap, input lock, the release schedule and the observation. Four run on synthetic rules so they hold without `RA2_DIR`; six need it to spawn real object types. |
| `vera20k` hotkey category | Groundwork, delivered separately: `catalog::VERA_CATEGORY_KEY` is declared in the input layer and listed after every category the retail commands carry. |
| AutoPlay hotkey | Implemented: `HotkeyCommand::ToggleAutoPlay`, registered in the `vera20k` category with **no default key** — `KEYBOARDMD.INI` has no entry for it, so the player assigns one. Pressing it installs or removes `MatchState::auto_play`. |
| Frame-loop wiring | Implemented: `advance_one_simulation_frame` steps the host immediately before `take_due_commands`, when one is installed. A match with no host takes the unchanged path. |
| A scripted scenario test (MCV → build → place → unit) | Not started. The actuation path is unit-tested, not yet exercised end to end in `headless_scenario`. |
| Anything in `sim::ai_*` | Untouched. |

`ScriptedAgent` takes a typed `Vec<(u64, Command)>`, not JSON. `Command` is
serde-derived, but its `InternedId` fields are numeric, so a JSON script could
not be written by hand without a second, competing command spelling. A
name-resolving script format is therefore a brain-stage question, not a hand
one, and H0's "Done when" below is adjusted accordingly.

## Target ownership

| Responsibility | Owner |
|---|---|
| Read-only projection of what the player can see | `sim::autoplay::observation` (new) |
| Agent interface | `sim::autoplay::agent` (new) |
| Admission gate, per-frame cap, envelope construction | `sim::autoplay::host` (new) |
| Calling the host once per frame | the frame loop: `app/match_runtime/sim_tick`, and the headless runner |
| Everything else | unchanged. No existing owner moves. |

**Placement: `src/sim/autoplay/`, inside the core crate.** Three reasons:

- The observation layer must call `pub(crate)` queries (`can_build`,
  `find_base_building_site`); from `app/` it cannot reach them.
- `src/headless_scenario.rs` can then drive an agent with no window, which is where the
  tests live.
- It depends on `sim` only, so the `sim/` → `render|ui|audio|net` rule holds.

## Design

Sketches. Accessor names are indicative and get fixed against the owning module while
implementing; the point here is the boundary, not the spelling.

### Observation

A read-only projection built for a specific house. It is **not** `SimView`: that returns
`&Simulation` and is effectively full read access. It is **not** `telemetry_snapshot()`
either: that is a debug view with `TELEMETRY_MAX_SPOTS` truncation and `f32` display
coordinates, and reading it would lose precision and threaten determinism.

```rust
pub struct AutoPlayView<'a> { sim: &'a Simulation, house: InternedId }

impl AutoPlayView<'_> {
    fn credits(&self) -> i32;
    fn power_balance(&self) -> i32;
    fn own_entities(&self) -> impl Iterator<Item = EntityView>;
    fn visible_enemies(&self) -> impl Iterator<Item = EntityView>;
    fn buildable(&self) -> impl Iterator<Item = BuildableView>;
    fn pending_placement(&self) -> Option<PendingPlacement>;
    fn placement_site(&self, ty: &ObjectType) -> Option<(i16, i16)>;
    fn is_passable(&self, rx: u16, ry: u16) -> bool;
}
```

Rules:

- Integer or `SimFixed` only. No `f32` reaches a decision.
- Iteration order is fixed: storage order for entities, registration order for houses.
  No `HashMap` iteration, ever.
- `visible_enemies` filters through `Fog::is_cell_visible(house, ..)`. Whether the agent
  should be fog-limited at all is a **brain** question, not decided here; the field is
  provided with a switch so either answer works.

### Agent

```rust
pub trait AutoPlayAgent {
    fn on_frame(&mut self, view: &AutoPlayView<'_>) -> Vec<Command>;
}
```

Called once per frame, between frames. The agent owns its own state, including whatever
it is waiting for.

### Host

```rust
pub struct AutoPlayHost { agent: Box<dyn AutoPlayAgent>, max_per_frame: usize }

impl AutoPlayHost {
    pub fn step(&mut self, sim: &mut Simulation, rules: &RuleSet);
}
```

Per frame: build the view for the local house → call `on_frame` → gate each command →
wrap into `CommandEnvelope { owner: <local house>, execute_tick: sim.session().tick(), .. }`
→ `sim.queue_commands(..)`.

The gate is deliberately minimal. Strict human equivalence is a non-goal, so the gate
only prevents self-harm, it does not re-derive `What_Action`:

- the acting entity belongs to `owner` — needed, because MegaMission dispatch checks the
  actor's lifetime but **not** its owner (`src/sim/world/world_commands.rs:2465-2469`
  leaves that to the input layer, and the input layer is not involved here);
- the actor is alive and not in limbo;
- at most `max_per_frame` commands;
- `session.input_locked()` is respected.

## Determinism rules for agents

The agent is part of the deterministic input stream, so:

- no wall clock, no file or network reads during a decision;
- no iteration over unordered containers;
- no `f32` in a decision;
- any randomness comes from a seeded generator the agent owns, never from a simulation
  stream.

Replay stays correct regardless of what the agent does, because replay re-runs the
command stream, not the agent.

Residual: agent state is not serialized, so a save/load mid-match loses the agent's
memory of what it was waiting for. Accepted for now; noted for the brain stage.

## Implementation chains

One PR each. A stage is done when its "Done when" holds.

### Stage H0: scripted actuation (skeleton delivered)

Scope: `observation`, `agent` with a `ScriptedAgent` playing a fixed list of
`(frame, Command)` pairs, `host`, and tests.

Delivered: the three modules and their tests, the frame-loop wiring, and the AutoPlay
hotkey that installs the host.

Still to do for this stage, before it counts as done:

- **Where the script comes from.** The hotkey installs a `ScriptedAgent`, and nothing
  supplies it a script yet, so switching auto play on runs an empty agent. Options are a
  file the player drops next to the map, a scripted scenario, or the policy layer
  answering directly — all brain-adjacent, so this is the next decision.
- An end-to-end scripted scenario in `src/headless_scenario.rs`: deploy the MCV, queue a
  power plant, place it at a site from `placement_site`, queue a unit — reproducible
  through `ReplayRunner` with identical state hashes.

Done when: both hold.

### Stage H1: full observation surface

Scope: fill in `AutoPlayView` — credits, power, own entities, visible enemies, buildable
list, production queue, pending placement, placement site, passability. Each field with a
test asserting it reads the authoritative owner and not a derived cache.

Done when: every field has a test that fails if the source is swapped for a stale or
derived one, and the headless script from H0 still replays identically.

After H1 the hands are complete and the brain can be designed against a real, tested
surface.

## Decided: where the brain's decisions come from

**Decision: reuse `sim::ai_*`, but only as a source of decisions.** The agent translates
each decision into commands. It never writes house state, `is_human` stays true, and the
player keeps selection, the sidebar and the right to order anything at any time. AI
takeover is operation, not control.

One premise behind that decision needs recording precisely, because it is easy to
overstate:

> There is no single native instruction that hands a player's house to the AI.

What exists instead is four trigger actions — **3, 13, 30, and 74/75** — each writing one
independent AI activation byte on a `HouseClass`:

| Action | Writes | Enables |
|---|---|---|
| 3 | `+0x1EE` Production | AI factory production (`production/factory_ai.rs`) |
| 13 | `+0x1EF` AutocreateAllowed | AI autocreate teams |
| 30 | `+0x1F3` AutoBaseBuilding | AI base building / BasePlan (`mcv_deploy.rs`) |
| 74 / 75 | `+0x1F2` AITriggersActive | AITrigger evaluation and team creation |

They resolve their target house through the shared action selector and have **no human
gate**, so a map can target the player's own house. But nothing in the trigger runtime
ever writes `is_human` or `player_control`: the player's selection, sidebar and ordering
rights survive untouched. That is the property to preserve, and it is also why these four
bytes are the natural granularity for "AI does the operating, the player keeps the
choice".

Note the consequence for a human house: `ai_base_building` and the other AI entries gate
on `is_controlled_by_human`, so setting these latches on the player's house does **not**
start the AI today. Auto play is what gives them meaning for a human house.

## Deferred: the brain

Specified above: decisions come from `sim::ai_*`, delivered as commands. Still open:
2. **Fog honesty.** `sim::ai_*` sees the whole map. Reusing it for the player house makes
   the player omniscient. Fine for unattended play, wrong for anything competitive.
   Switch already provided in `AutoPlayView`.
3. **House-state memory.** `ai_production`, `ai_unit_choices` and the team timers are the
   computer AI's memory. One owner writes them: either the existing AI (computer houses)
   or the agent (player house), never both.
4. **RNG.** Some AI decision paths draw from a simulation stream. Reuse changes the draw
   sequence. Harmless for replay, but such a match must not be used for native parity.
5. **Team scripts.** `team_script_vm` assigns missions directly. Some actions have no
   player equivalent, so translation is lossy and needs an action-by-action ruling.
6. **Out-of-process agent.** Whether the brain lives outside the process (BWAPI-Client /
   PySC2 style), and if so over what transport.
7. **Where the host is called from** once a real policy exists, and how it is gated per
   match mode.

## Validation per stage

- Headless scenario runs (`src/headless_scenario.rs`), never retail assets in the lib
  suite.
- Replay reproduction and state-hash equality for every scripted scenario.
- The existing lib suite for the touched modules.
- No native comparison: this is not a port. If a later stage ports policy from gamemd,
  that stage states its own evidence level per the project contract.

## Open questions

- Does the host belong in the app frame loop only, or also inside the headless runner as
  a first-class mode? (H0 answers this in practice.)
- Should `max_per_frame` be native-derived or a plain configurable cap? Unknown native
  equivalent; propose a plain cap and record the residual.
- Does an agent-driven house need `is_human` left untouched? Assumed yes — the computer
  AI gates on `is_controlled_by_human` (`src/sim/house_state.rs:2255`) and must stay off
  for a player house, or two owners would drive one house.
