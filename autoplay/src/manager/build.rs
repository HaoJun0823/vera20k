//! Build: placement, walls, repairs.
//!
//! A stub so the registry names a manager that exists; the work lands one phase
//! at a time. See autoplay/docs/spec.md.

use super::{Ctx, Demands, Manager};

#[derive(Debug, Default)]
pub struct Build;

impl Manager for Build {
    fn propose(&mut self, _ctx: &Ctx<'_>, _out: &mut Demands) {}
}
