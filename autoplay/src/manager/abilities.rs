//! Abilities: transports, garrisons, IFV gunner, superweapons.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 6.

use super::Manager;
use crate::job::Proposal;

#[derive(Debug, Default)]
pub struct Abilities;

impl Manager for Abilities {
    fn propose(&mut self, _out: &mut Vec<Proposal>) {}
}
