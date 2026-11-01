//! Managers: the only things allowed to ask for work.
//!
//! A manager reads the projection and proposes jobs. It never issues a command,
//! never touches another manager's state, and never picks the unit — the
//! assignment does that. One file per manager, registered in crate::registry.

pub mod abilities;
pub mod build;
pub mod economy;
pub mod military;

use crate::job::Proposal;

/// What a manager may do in one term.
pub trait Manager {
    /// Propose what this manager wants done. Read-only over the projection.
    fn propose(&mut self, out: &mut Vec<Proposal>);
}
