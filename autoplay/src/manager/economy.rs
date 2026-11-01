//! Economy: income, harvesters, refineries, the build order.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 4.

use super::Manager;
use crate::job::Proposal;

#[derive(Debug, Default)]
pub struct Economy;

impl Manager for Economy {
    fn propose(&mut self, _out: &mut Vec<Proposal>) {}
}
