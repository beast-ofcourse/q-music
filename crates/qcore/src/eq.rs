//! Ten-band equaliser with a preamp and a soft limiter.
//!
//! One peaking biquad per band per channel, recalculated only when the user moves
//! a slider. State lives with the processor, so engaging the EQ mid-track does not
//! produce a click beyond the first sample of the new filter chain.

use crate::model::EqSettings;

/// ISO third-octave centres used by every hardware graphic equaliser.
pub const EQ_FREQUENCIES: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

const BAND_Q: f32 = 1.41;
const MAX_GAIN_DB: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    /// RBJ cookbook peaking filter.
    fn peaking(sample_rate: f32, frequency: f32, q: f32, gain_db: f32) -> Self {
        let nyquist_limit = sample_rate * 0.45;
        let freq = frequency.min(nyquist_limit).max(20.0);
        if gain_db.abs() < 0.01 {
            return Self::identity();
        }

        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let sin_w0 = w0.sin();
        let cos_w0 = w0.cos();
        let alpha = sin_w0 / (2.0 * q);

        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

pub struct Equalizer {
    sample_rate: f32,
    bands: [[Biquad; 2]; EQ_FREQUENCIES.len()],
    preamp: f32,
    enabled: bool,
}

impl Equalizer {
    pub fn new(sample_rate: u32) -> Self {
        let mut equalizer = Self {
            sample_rate: sample_rate as f32,
            bands: [[Biquad::identity(); 2]; EQ_FREQUENCIES.len()],
            preamp: 1.0,
            enabled: false,
        };
        equalizer.configure(&EqSettings::flat());
        equalizer
    }

    /// Applies a persisted setting. Only the filters whose gain changed are rebuilt.
    pub fn configure(&mut self, settings: &EqSettings) {
        let settings = settings.clone().normalized();
        self.enabled = settings.enabled;
        self.preamp = 10f32.powf(settings.preamp_db / 20.0);

        for (index, frequency) in EQ_FREQUENCIES.iter().enumerate() {
            let gain = settings.gains[index];
            for channel in 0..2 {
                self.bands[index][channel] =
                    Biquad::peaking(self.sample_rate, *frequency, BAND_Q, gain);
            }
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if (self.sample_rate - sample_rate as f32).abs() > f32::EPSILON {
            self.sample_rate = sample_rate as f32;
            let muted = EqSettings { enabled: self.enabled, gains: vec![0.0; 10], preamp_db: 0.0 };
            self.configure(&muted);
        }
    }

    /// Clears filter memory at a seek or track change.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band[0].reset();
            band[1].reset();
        }
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.enabled {
            return (soft_limit(left), soft_limit(right));
        }

        let mut l = left * self.preamp;
        let mut r = right * self.preamp;
        for band in &mut self.bands {
            l = band[0].process(l);
            r = band[1].process(r);
        }
        (soft_limit(l), soft_limit(r))
    }

    /// Peak gain applied by the current setting, used to warn about clipping.
    pub fn max_gain_db(&self) -> f32 {
        self.bands
            .iter()
            .map(|band| 20.0 * band[0].b0.abs().max(1.0).log10())
            .fold(0.0, f32::max)
            .min(MAX_GAIN_DB)
    }
}

/// Gentle saturation above 0.8 instead of a hard square edge.
#[inline]
fn soft_limit(sample: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = sample.abs();
    if magnitude <= KNEE {
        return sample;
    }
    let over = magnitude - KNEE;
    let compressed = KNEE + over / (1.0 + over * 3.0) * (1.0 - KNEE) * 4.0 * 0.25;
    compressed.min(1.0).copysign(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(gain_db: f32, enabled: bool) -> EqSettings {
        EqSettings {
            enabled,
            gains: vec![gain_db; EQ_FREQUENCIES.len()],
            preamp_db: 0.0,
        }
    }

    #[test]
    fn flat_disabled_settings_pass_signal_through() {
        let mut eq = Equalizer::new(48_000);
        eq.configure(&EqSettings::flat());
        let (l, r) = eq.process(0.25, -0.25);
        assert!((l - 0.25).abs() < 1e-6);
        assert!((r + 0.25).abs() < 1e-6);
    }

    #[test]
    fn boost_raises_low_frequency_energy() {
        let mut eq = Equalizer::new(48_000);
        eq.configure(&settings(9.0, true));
        let mut peak = 0.0f32;
        // 1 kHz sits on a band centre; feed it and measure the settled amplitude.
        for index in 0..4_800 {
            let phase = index as f32 / 48_000.0 * 1_000.0 * std::f32::consts::TAU;
            let (out, _) = eq.process(phase.sin() * 0.1, 0.0);
            peak = peak.max(out.abs());
        }
        assert!(peak > 0.2, "boosted output stayed at {peak}");
        assert!(peak <= 1.0, "output clipped hard at {peak}");
    }

    #[test]
    fn reset_clears_filter_memory() {
        let mut eq = Equalizer::new(44_100);
        eq.configure(&settings(6.0, true));
        for _ in 0..128 {
            eq.process(0.9, 0.9);
        }
        eq.reset();
        let (l, _) = eq.process(0.0, 0.0);
        assert_eq!(l, 0.0);
    }

    #[test]
    fn settings_are_normalised_to_ten_bands() {
        let repaired = EqSettings { enabled: true, gains: vec![40.0, -40.0], preamp_db: 99.0 }
            .normalized();
        assert_eq!(repaired.gains.len(), EQ_FREQUENCIES.len());
        assert_eq!(repaired.gains[0], 12.0);
        assert_eq!(repaired.gains[1], -12.0);
        assert_eq!(repaired.preamp_db, 12.0);
    }
}
