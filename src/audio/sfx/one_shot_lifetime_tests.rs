//! One-shot SFX lifetime across device sample rates.
//!
//! Production hands `RingSource` to rodio's mixer through
//! `Player::connect_new(mixer)` + `player.append(source)`. The mixer runs at
//! the *device* rate (48 kHz or 192 kHz depending on the default output
//! endpoint) and converts the source with `UniformSourceIterator`.
//!
//! `quarter_samples` pins the ring to `round_up_1024((bytes_per_second * 3 /
//! 100) * 8)` frames of the **source** rate, while `RingSource::sample_rate`
//! reports the **shifted playback** rate. Those two only agree when the device
//! rate matches the source rate; at 192 kHz rodio resamples by ~8.7x.
//!
//! These checks pin the observable contract: a one-shot cue must reach
//! exhaustion (rodio's `Player::empty()` becomes true) at every device rate,
//! not only at the rate the ring was sized for.

use super::*;
use crate::audio::sfx::{
    DecodedAudio, LoadedPlayback, LoadedSampleIndices, PlaybackDraws, PlaybackService,
    PlaylistCursor, RingSource, SoundArbiter,
    arbiter::{ArbiterAction, EntryFacts, EventId, PlayRequest},
};
use crate::rules::sound_ini::{SoundEntry, VOLUME_SCALE};
use rodio::Source;
use std::num::NonZero;

/// A short one-shot cue: 24 ms of mono-ish stereo at 22 050 Hz, the length of
/// a `CreditUp` tick and far shorter than one ring quarter.
fn one_shot_clip() -> DecodedAudio {
    let rate = 22_050u32;
    let frames = (rate * 24 / 1000) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let value = if frame % 2 == 0 { 0.5 } else { -0.5 };
        samples.push(value);
        samples.push(value);
    }
    DecodedAudio {
        samples,
        sample_rate: rate,
        channels: 2,
        native_frame_bytes: 2,
    }
}

/// Minimal `PlaybackService` so a real arbiter event can drive one playout.
struct OneShotPlayback {
    clip: DecodedAudio,
    loaded: bool,
    prepared: bool,
    source: Option<(RingSource, PlaybackControl)>,
}

impl PlaybackService for OneShotPlayback {
    fn channel_acquired(&mut self, _: EventId) -> i32 {
        0
    }

    fn load_samples(&mut self, _: EventId, _: bool) -> bool {
        self.loaded = true;
        true
    }

    fn prepare_playout(&mut self, _: EventId, plays_attack: bool) -> bool {
        self.prepared = true;
        let _ = plays_attack;
        true
    }

    fn start_playout(&mut self, _: EventId, pan: i32) -> bool {
        let clip = self.clip.clone();
        let mut clips = std::collections::BTreeMap::new();
        clips.insert(0usize, clip);
        let loaded = LoadedPlayback {
            selected: LoadedSampleIndices {
                attack: None,
                middle: vec![0],
                decay: None,
            },
            clips,
        };
        let entry = one_shot_entry();
        let mut cursor = PlaylistCursor::new(
            loaded.selected.clone(),
            entry.control,
            entry.delay_ms,
            entry.loop_count,
        );
        let draws = PlaybackDraws::new(|_, _| 0);
        let Some(initial) = cursor.prepare(&mut { draws.clone() }, false) else {
            return false;
        };
        let rate = loaded.clips[&initial].sample_rate;
        self.source = RingSource::new(loaded, cursor, initial, draws, rate, pan, None);
        self.source.is_some()
    }

    fn stop_playout(&mut self, _: EventId) {}
}

fn one_shot_entry() -> SoundEntry {
    SoundEntry {
        id: "TESTONE".to_owned(),
        sounds: vec!["TESTONE".to_owned()],
        priority: 2,
        limit: 5,
        control: 0,
        type_flags: 0,
        loop_count: 0,
        delay_ms: (0, 0),
        fshift: (0, 0),
        vshift: 0,
        attack: 0,
        decay: 0,
        volume: 100,
        volume_linear: VOLUME_SCALE,
        range: 10,
        min_volume: 0.2,
    }
}

fn one_shot_facts() -> EntryFacts {
    EntryFacts {
        priority: 2,
        limit: 5,
        control: 0,
        loop_count: 0,
        delay_ms: (0, 0),
        entry_volume_linear: VOLUME_SCALE,
        samples_ok: true,
    }
}

/// Build one started `RingSource` through the production arbiter phases.
fn started_one_shot() -> (RingSource, PlaybackControl) {
    let mut arbiter = SoundArbiter::new(0);
    let request = PlayRequest {
        key: "TESTONE".to_owned(),
        facts: one_shot_facts(),
        volume_linear: VOLUME_SCALE,
        pan: crate::audio::sfx::PAN_CENTRE,
    };
    arbiter.submit(&request, 0).expect("event admitted");
    let mut playback = OneShotPlayback {
        clip: one_shot_clip(),
        loaded: false,
        prepared: false,
        source: None,
    };
    let actions = arbiter.update_tick(100, &mut playback);
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, ArbiterAction::Start { .. })),
        "the one-shot must start: {actions:?}"
    );
    playback.source.expect("started source")
}

/// Pull from a real rodio mixer at `device_rate` until the mixer reports the
/// source exhausted, bounded so a source that never ends fails instead of
/// hanging. Returns whether it ended and how many device frames were needed.
fn consume_at(device_rate: u32) -> (bool, usize) {
    let (source, _control) = started_one_shot();
    let channels = NonZero::new(2u16).expect("stereo");
    let rate = NonZero::new(device_rate).expect("nonzero rate");
    let (controller, mut sink) = rodio::mixer::mixer(channels, rate);
    let player = rodio::Player::connect_new(&controller);
    player.append(source);

    // Three seconds of device audio is far past a 24 ms cue's real length.
    let limit = (device_rate as usize) * 3 * usize::from(channels.get());
    let mut pulled = 0usize;
    while pulled < limit {
        if sink.next().is_none() {
            break;
        }
        pulled += 1;
        if player.empty() {
            return (
                true,
                pulled / (usize::from(channels.get()) * device_rate as usize),
            );
        }
    }
    (false, pulled)
}

#[test]
fn one_shot_cue_reaches_exhaustion_at_48k() {
    let (ended, _seconds) = consume_at(48_000);
    assert!(ended, "a 24 ms one-shot must end on a 48 kHz mixer");
}

#[test]
fn one_shot_cue_reaches_exhaustion_at_192k() {
    let (ended, _seconds) = consume_at(192_000);
    assert!(
        ended,
        "a 24 ms one-shot must end on a 192 kHz mixer too; \
         the ring is sized from the source rate while rodio resamples to the \
         device rate"
    );
}

/// A consumer at any device rate must observe the cue end. This is the
/// property production depends on: `report_finished_outputs` retires an output
/// only once `Player::empty()` turns true, and an endless source would hold its
/// channel forever.
#[test]
fn one_shot_cue_ends_for_every_resampling_consumer() {
    let clip = one_shot_clip();
    for device_rate in [22_050u32, 48_000, 96_000, 192_000, 384_000] {
        let (source, _control) = started_one_shot();
        let channels = NonZero::new(2u16).expect("stereo");
        let mut uniform = rodio::source::UniformSourceIterator::new(
            source,
            channels,
            NonZero::new(device_rate).expect("nonzero rate"),
        );
        // Bounded so an endless source fails instead of hanging. Ten seconds of
        // device audio is far past a 24 ms cue at any resampling ratio.
        let ceiling = device_rate as usize * 10 * usize::from(channels.get());
        let delivered = uniform.by_ref().take(ceiling).count();
        assert!(
            delivered < ceiling,
            "consumer at {device_rate} Hz never saw the cue end \
             (pulled {delivered} samples, {} bytes of clip)",
            clip.samples.len()
        );
    }
}
