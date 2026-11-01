//! Build: placement, walls, repairs.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 4.

use super::Manager;
use crate::job::Proposal;

#[derive(Debug, Default)]
pub struct Build;

impl Manager for Build {
    fn propose(&mut self, _out: &mut Vec<Proposal>) {}
}
