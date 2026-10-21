//! The agent interface and the scripted agent.
//!
//! An agent is called once per frame, between frames, and answers with the
//! commands the player would have produced by clicking. It owns whatever state
//! it needs, including what it is waiting for; the simulation does not.
//!
//! Determinism: the agent is part of the deterministic input stream. It may not
//! read a clock, a file or a socket while deciding, may not iterate an
//! unordered container, and may not let an `f32` reach a decision. Any
//! randomness must come from a generator it seeds and owns, never from a
//! simulation stream.

use crate::sim::autoplay::observation::AutoPlayView;
use crate::sim::command::Command;

/// A player substitute: it looks at [`AutoPlayView`] and answers with commands.
pub trait AutoPlayAgent {
    /// Called once per frame. Returned commands are queued for this house and
    /// take effect in the frame tail of the next frame.
    fn on_frame(&mut self, view: &AutoPlayView<'_>) -> Vec<Command>;
}

/// One command, released at one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptStep {
    /// The command is released on the first frame whose tick reaches this.
    pub frame: u64,
    pub command: Command,
}

/// An agent that plays a fixed script: a list of `(frame, Command)` pairs.
///
/// The script is a schedule, not a policy — it never reads the view. It exists
/// so the actuation path can be built and tested before any policy does, which
/// is the point of the hands-before-brain ordering in `docs/autoplay.md`.
#[derive(Debug, Clone, Default)]
pub struct ScriptedAgent {
    steps: Vec<ScriptStep>,
    next: usize,
}

impl ScriptedAgent {
    /// A script in any order; it is sorted by frame with a stable sort, so the
    /// release order of same-frame steps is the order they were given.
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        let mut steps = steps;
        steps.sort_by_key(|step| step.frame);
        Self { steps, next: 0 }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (u64, Command)>) -> Self {
        Self::new(
            pairs
                .into_iter()
                .map(|(frame, command)| ScriptStep { frame, command })
                .collect(),
        )
    }

    /// Steps not yet released.
    pub fn pending(&self) -> &[ScriptStep] {
        &self.steps[self.next..]
    }
}

impl AutoPlayAgent for ScriptedAgent {
    fn on_frame(&mut self, view: &AutoPlayView<'_>) -> Vec<Command> {
        let tick = view.tick();
        let mut released = Vec::new();
        while let Some(step) = self.steps.get(self.next) {
            if step.frame > tick {
                break;
            }
            released.push(step.command.clone());
            self.next += 1;
        }
        released
    }
}
