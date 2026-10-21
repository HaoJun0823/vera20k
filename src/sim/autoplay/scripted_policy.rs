//! The scripted policy: a fixed list of commands released by frame number.
//!
//! It is a schedule, not a plan — it never reads the view. It exists so the
//! actuation path can be built and tested against something, and it is the
//! simplest thing that can occupy the policy seam.

use crate::sim::autoplay::observation::AutoPlayView;
use crate::sim::autoplay::policy::AutoPlayPolicy;
use crate::sim::command::Command;

/// One command, released at one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptStep {
    /// The command is released on the first frame whose tick reaches this.
    pub frame: u64,
    pub command: Command,
}

/// A policy that plays a fixed script.
#[derive(Debug, Clone, Default)]
pub struct ScriptedPolicy {
    steps: Vec<ScriptStep>,
    next: usize,
}

impl ScriptedPolicy {
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

impl AutoPlayPolicy for ScriptedPolicy {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn decide(&mut self, view: &AutoPlayView<'_>) -> Vec<Command> {
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
