//! The presentation contract: which command writes which cameo.
//!
//! This is a pure table test — no simulation, no strip, no rules. That is the
//! point: the contract is small enough to check by reading, and a command added
//! later with a visible half has to be decided here rather than discovered from
//! a stream that shows nothing.

use crate::app::sidebar_projection::{CameoMirror, cameo_mirror};
use crate::sim::command::Command;
use crate::sim::intern::InternedId;
use crate::sim::production::ProductionCategory;

fn id(index: u32) -> InternedId {
    InternedId::from_index(index)
}

#[test]
fn a_produce_order_arms_the_cameo_that_builds_it() {
    assert_eq!(
        cameo_mirror(&Command::QueueProduction { type_id: id(7) }),
        Some(CameoMirror::Produce(id(7))),
        "a queued type writes the clock the player's press would write"
    );
}

#[test]
fn a_placement_arms_the_cameo_of_the_building_being_sited() {
    assert_eq!(
        cameo_mirror(&Command::PlaceReadyBuilding {
            type_id: id(9),
            rx: 3,
            ry: 4
        }),
        Some(CameoMirror::Place(id(9))),
        "a placed building shows its request while the site goes down"
    );
}

#[test]
fn a_mobile_placement_has_no_cameo_of_its_own() {
    assert_eq!(
        cameo_mirror(&Command::PlaceProducedMobile {
            category: ProductionCategory::Vehicle
        }),
        None,
        "a mobile placement names a category, not a type"
    );
}

#[test]
fn an_order_that_shows_nothing_writes_no_cameo() {
    let orders = [
        Command::Move {
            entity_id: 1,
            target_rx: 2,
            target_ry: 3,
        },
        Command::Guard { entity_id: 1, target: None },
        Command::Stop { entity_id: 1 },
        Command::SellBuilding { entity_id: 1 },
        Command::DeployMcv { entity_id: 1 },
    ];
    for order in orders {
        assert_eq!(
            cameo_mirror(&order),
            None,
            "a player's click would not write a cameo for {order:?}"
        );
    }
}