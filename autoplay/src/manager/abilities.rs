//! Abilities: transports, garrisons, IFV gunner, superweapons.
//!
//! Not implemented yet. See docs/autoplay-v2.md, step 6.

use super::{Ctx, Demands, Manager};

#[derive(Debug, Default)]
pub struct Abilities;

impl Manager for Abilities {
    fn propose(&mut self, _ctx: &Ctx<'_>, _out: &mut Demands) {}
}
