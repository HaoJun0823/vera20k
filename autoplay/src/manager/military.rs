//! Military: teams, attack, defence.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 5.

use super::{Ctx, Demands, Manager};

#[derive(Debug, Default)]
pub struct Military;

impl Manager for Military {
    fn propose(&mut self, _ctx: &Ctx<'_>, _out: &mut Demands) {}
}
