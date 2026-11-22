//! End-to-end proof that air absorption runs through the production
//! `RingSource::next()` path, and that the default-off path is unchanged.

use super::*;
use crate::rules::ini_parser::IniFile;

/// Build a stereo ring source from a synthetic alternating (bright) cue.
fn bright_ring() -> (RingSource, PlaybackControl) {
    const FRAMES: usize = 512;
    let registry = SoundRegistry::from_ini(&IniFile::from_str(
        "[SoundList]\n1=Test\n[Test]\nSounds=body\n",
    ));
    let entry = registry.get("Test").unwrap();
    let mut draws = PlaybackDraws::new(|low, _| low);
    // Alternating +1/-1 on both channels is a near-Nyquist signal at 22.05 kHz.
    let mut samples = Vec::with_capacity(FRAMES * 2);
    for i in 0..FRAMES {
        let value = if i % 2 == 0 { 1.0 } else { -1.0 };
        samples.push(value);
        samples.push(value);
    }
    let loaded = LoadedPlayback::load(entry, &mut draws, |_| {
        Some(DecodedAudio {
            samples: samples.clone().into(),
            sample_rate: 22_050,
            channels: 2,
            native_channels: 1,
            native_sample_bytes: 2,
        })
    })
    .unwrap();
    let mut cursor = PlaylistCursor::new(loaded.selected.clone(), entry.control, (0, 0), 1);
    let initial = cursor.prepare(&mut draws, true).unwrap();
    RingSource::new(
        loaded,
        cursor,
        initial,
        draws,
        PlayShifts::UNITY,
        0x2000,
        None,
    )
    .unwrap()
}

fn peak_after_settle(source: &mut RingSource, control: &PlaybackControl, volume: Option<f32>) -> f32 {
    control.set_air_absorption(volume);
    let mut peak = 0.0f32;
    for i in 0..400 {
        if let Some(value) = source.next() {
            if i >= 200 {
                peak = peak.max(value.abs());
            }
        }
    }
    peak
}

#[test]
fn default_off_ring_passes_the_bright_cue_unchanged() {
    let (mut source, control) = bright_ring();
    // With the switch off the atomic holds DISABLED; every sample is the
    // original alternating value scaled by the centred pan gain of 1.0.
    let peak = peak_after_settle(&mut source, &control, None);
    assert!((peak - 1.0).abs() < 1e-6, "unfiltered peak {peak}");
}

#[test]
fn enabled_far_filter_darkens_the_bright_cue_through_the_ring() {
    let (mut source, control) = bright_ring();
    let peak = peak_after_settle(&mut source, &control, Some(0.05));
    assert!(peak < 0.5, "filtered far peak {peak} should drop the top end");
}

#[test]
fn enabled_near_filter_leaves_the_bright_cue_largely_open() {
    let (mut source, control) = bright_ring();
    let peak = peak_after_settle(&mut source, &control, Some(1.0));
    assert!(peak > 0.7, "near peak {peak} should stay bright");
}

#[test]
fn toggling_off_restores_the_native_spectrum_immediately() {
    let (mut source, control) = bright_ring();
    let _ = peak_after_settle(&mut source, &control, Some(0.05));
    // Same source, switch off: the next pulls must return to full amplitude
    // (the IIR is bypassed entirely, not just re-coefficiented).
    let peak = peak_after_settle(&mut source, &control, None);
    assert!((peak - 1.0).abs() < 1e-6, "restored peak {peak}");
}
