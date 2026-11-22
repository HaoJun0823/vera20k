//! Distance-dependent low-pass ("air absorption") for positional SFX.
//!
//! VERA-only enhancement, off by default. gamemd's `VocClass::CalcVolumeAndPan
//! @ 0x00750AC0` only produces a scalar volume and a pan; distance never
//! changes a sound's spectrum, so a distant gunshot stays as bright as a near
//! one and reads as "small" rather than "far". Real air attenuates high
//! frequencies faster than low ones, so rolling off the top end with distance
//! is the single strongest perceptual distance cue we can add without an HRTF.
//!
//! This module is deliberately device-free and dependency-free: it is a
//! one-pole (first-order) IIR low-pass whose coefficient is derived from the
//! sound's spatial volume and its sample rate. It is enabled only when the
//! player turns on the VERA option; with the option off, callers leave the
//! filter disabled and sample output is bit-identical to the native path.

/// Cutoff at full spatial volume (listener, closest audible point), in Hz.
/// 18 kHz is effectively "open" for the 22.05 kHz retail samples and for any
/// normal device rate, so near sounds pass unchanged.
const NEAR_CUTOFF_HZ: f32 = 18_000.0;
/// Cutoff at the audibility floor (0.05, the farthest audible point), in Hz.
/// Around 2 kHz keeps speech and impact body while removing the brightness
/// that betrays a "shrunk" near sample.
const FAR_CUTOFF_HZ: f32 = 2_000.0;
/// Spatial volume at which the far cutoff is reached. Matches the native
/// audibility cutoff `MIN_VOLUME_CUTOFF` in `super`.
const MIN_AUDIBLE_VOLUME: f32 = 0.05;
/// Keep the cutoff strictly below Nyquist; a one-pole at Nyquist is marginal.
const NYQUIST_MARGIN: f32 = 0.95;

/// Sentinel stored in the playback control's atomic when air absorption is
/// off. `u32::MAX` is a NaN bit pattern, which no valid `0..=1` spatial volume
/// can produce, so it is unambiguous.
pub(crate) const DISABLED: u32 = u32::MAX;

/// A one-pole low-pass coefficient for `cutoff_hz` at `sample_rate`.
///
/// Standard RBJ/`exp` form `alpha = 1 - exp(-2*pi*fc/fs)`, which stays in
/// `(0, 1)` for any finite positive cutoff. Rounded to `f32` because the
/// playback ring is `f32`.
fn coefficient(cutoff_hz: f32, sample_rate: u32) -> f32 {
    if sample_rate == 0 {
        return 0.0;
    }
    let omega = core::f32::consts::TAU * cutoff_hz / sample_rate as f32;
    1.0 - (-omega).exp()
}

/// Cutoff in Hz for a spatial volume in `0..=1`.
///
/// `volume` is the positional gain computed by `super::spatial_gain`: 1 at
/// the closest point, declining to 0 at `Range=`. The curve is linear in
/// volume but expressed over the audible band only (`MIN_AUDIBLE_VOLUME..1`),
/// so the roll-off tracks how far the sound actually is rather than the
/// absolute number the INI entry happens to carry.
pub(crate) fn cutoff_for_volume(volume: f32, sample_rate: u32) -> f32 {
    let audible = volume.clamp(MIN_AUDIBLE_VOLUME, 1.0);
    let fraction = (audible - MIN_AUDIBLE_VOLUME) / (1.0 - MIN_AUDIBLE_VOLUME);
    let hz = FAR_CUTOFF_HZ + (NEAR_CUTOFF_HZ - FAR_CUTOFF_HZ) * fraction;
    let nyquist = 0.5 * sample_rate as f32 * NYQUIST_MARGIN;
    hz.min(nyquist).max(1.0)
}

/// Per-channel one-pole low-pass state.
///
/// One instance belongs to one playback source; `state` holds the last
/// filtered sample of each channel so a stereo source keeps its two filters
/// independent (the left/right pan gain is applied after this stage by
/// `RingSource`). `alpha == 0` means the filter is off and `process` is a
/// pass-through, which is what keeps the default path bit-identical.
#[derive(Clone, Copy)]
pub(crate) struct AirAbsorption {
    alpha: f32,
    state: [f32; 2],
}

impl AirAbsorption {
    /// Build a disabled filter. Callers with the VERA option off never touch
    /// it, but the default is a pass-through for safety.
    pub(crate) fn off() -> Self {
        Self {
            alpha: 0.0,
            state: [0.0; 2],
        }
    }

    /// Enable the filter from a spatial volume and the source's sample rate.
    /// Reused sources call this on every gain refresh so a moving sound's
    /// brightness tracks its distance. Existing filter state is preserved.
    pub(crate) fn set_volume(&mut self, volume: f32, sample_rate: u32) {
        self.alpha = coefficient(cutoff_for_volume(volume, sample_rate), sample_rate);
    }

    /// Whether filtering is active.
    pub(crate) fn enabled(self) -> bool {
        self.alpha > 0.0
    }

    /// Filter one interleaved sample on `channel` (0 = left, 1 = right).
    /// With the filter off this returns `sample` unchanged.
    #[inline]
    pub(crate) fn process(&mut self, sample: f32, channel: usize) -> f32 {
        if self.alpha == 0.0 {
            return sample;
        }
        let slot = channel.min(1);
        let filtered = self.state[slot] + self.alpha * (sample - self.state[slot]);
        self.state[slot] = filtered;
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_filter_is_an_exact_pass_through() {
        let mut filter = AirAbsorption::off();
        for (i, value) in [0.0, 1.0, -1.0, 0.37, f32::MAX].into_iter().enumerate() {
            assert_eq!(filter.process(value, i % 2), value);
        }
        assert!(!filter.enabled());
    }

    #[test]
    fn cutoff_falls_monotonically_with_distance_stays_below_nyquist() {
        let rate = 22_050;
        let near = cutoff_for_volume(1.0, rate);
        let mid = cutoff_for_volume(0.5, rate);
        let far = cutoff_for_volume(0.05, rate);
        assert!(near > mid && mid > far, "{near} {mid} {far}");
        // At 22.05 kHz the near cutoff is Nyquist-clamped (~10.5 kHz); at a
        // high-rate device it reaches the full 18 kHz open band.
        assert!((near - 0.5 * rate as f32 * NYQUIST_MARGIN).abs() < 0.001);
        assert!((cutoff_for_volume(1.0, 96_000) - NEAR_CUTOFF_HZ).abs() < 0.001);
        assert!((far - FAR_CUTOFF_HZ).abs() < 0.001);
        // Clamped below Nyquist even at a low device rate.
        assert!(cutoff_for_volume(1.0, 4_000) < 2_000.0);
        // Values below the audibility floor are pinned to the far cutoff.
        assert_eq!(cutoff_for_volume(0.0, rate), far);
    }

    #[test]
    fn a_distant_filter_attenuates_high_frequency_but_keeps_dc() {
        let rate = 22_050;
        let mut far = AirAbsorption::off();
        far.set_volume(0.05, rate);
        assert!(far.enabled());
        // A constant signal is passed (after a one-sample settle) regardless
        // of cutoff: this is a low-pass, not a volume change.
        let mut dc = far;
        let mut last = 0.0;
        for _ in 0..64 {
            last = dc.process(0.5, 0);
        }
        assert!((last - 0.5).abs() < 1e-3, "DC {last}");

        // A full-scale alternating (near-Nyquist) signal is strongly reduced.
        let mut ac = far;
        let mut peak = 0.0f32;
        for i in 0..64 {
            let input = if i % 2 == 0 { 1.0 } else { -1.0 };
            let out = ac.process(input, 0);
            if i >= 32 {
                peak = peak.max(out.abs());
            }
        }
        assert!(peak < 0.5, "far high-frequency peak {peak}");
    }

    #[test]
    fn a_near_filter_keeps_more_high_frequency_than_a_far_one() {
        let rate = 44_100;
        let alternating_peak = |volume: f32| {
            let mut filter = AirAbsorption::off();
            filter.set_volume(volume, rate);
            let mut peak = 0.0f32;
            for i in 0..128 {
                let input = if i % 2 == 0 { 1.0 } else { -1.0 };
                let out = filter.process(input, 0);
                if i >= 64 {
                    peak = peak.max(out.abs());
                }
            }
            peak
        };
        let near_peak = alternating_peak(1.0);
        let far_peak = alternating_peak(0.05);
        assert!(
            near_peak > far_peak,
            "near {near_peak} should pass more HF than far {far_peak}"
        );
    }

    #[test]
    fn left_and_right_channels_keep_independent_state() {
        let mut filter = AirAbsorption::off();
        filter.set_volume(0.05, 22_050);
        // Drive only the left channel with a step; the right must stay at 0.
        for _ in 0..32 {
            let _ = filter.process(1.0, 0);
            let right = filter.process(0.0, 1);
            assert_eq!(right, 0.0);
        }
        assert!(filter.state[0] > 0.0);
        assert_eq!(filter.state[1], 0.0);
    }
}
