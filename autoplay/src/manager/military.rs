//! Military: teams, attack, defence.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 5.

use super::Manager;
use crate::job::Proposal;

#[derive(Debug, Default)]
pub struct Military;

impl Manager for Military {
    fn propose(&mut self, _out: &mut Vec<Proposal>) {}
}
