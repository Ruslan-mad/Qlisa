//! Authored source-time gain automation for Audio and Video cues.
//!
//! Authored points use file milliseconds and linear amplitude. The RT form is
//! a bounded `Copy` value so it can cross the audio command ring without a
//! heap allocation or an RT-thread destructor.

use serde::{Deserialize, Serialize};

pub const MAX_GAIN_ENVELOPE_POINTS: usize = 128;
pub const MAX_GAIN_ENVELOPE_GAIN: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GainEnvelopePoint {
    pub time_ms: u32,
    pub gain: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GainEnvelope {
    pub points: Vec<GainEnvelopePoint>,
}

/// Fixed-size representation used by voices and the audio command ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainEnvelopeRt {
    points: [GainEnvelopePoint; MAX_GAIN_ENVELOPE_POINTS],
    count: u8,
}

impl Default for GainEnvelopeRt {
    fn default() -> Self {
        Self::unity()
    }
}

impl GainEnvelopeRt {
    pub const fn unity() -> Self {
        Self {
            points: [GainEnvelopePoint { time_ms: 0, gain: 1.0 }; MAX_GAIN_ENVELOPE_POINTS],
            count: 0,
        }
    }

    pub fn from_envelope(envelope: &GainEnvelope) -> Result<Self, String> {
        envelope.validate()?;
        let mut result = Self::unity();
        result.count = envelope.points.len() as u8;
        result.points[..envelope.points.len()].copy_from_slice(&envelope.points);
        Ok(result)
    }

    pub fn gain_at_ms(&self, time_ms: f64) -> f32 {
        let count = self.count as usize;
        if count == 0 {
            return 1.0;
        }
        if count == 1 || time_ms <= self.points[0].time_ms as f64 {
            return self.points[0].gain;
        }
        if time_ms >= self.points[count - 1].time_ms as f64 {
            return self.points[count - 1].gain;
        }

        let mut low = 1;
        let mut high = count - 1;
        while low < high {
            let mid = low + (high - low) / 2;
            if (self.points[mid].time_ms as f64) < time_ms {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let right = low;
        let left = right - 1;
        let a = self.points[left];
        let b = self.points[right];
        let span = (b.time_ms - a.time_ms) as f64;
        let t = ((time_ms - a.time_ms as f64) / span).clamp(0.0, 1.0) as f32;
        a.gain + (b.gain - a.gain) * t
    }

    pub fn gain_at_frame(&self, frame: f64, sample_rate: u32) -> f32 {
        self.gain_at_ms(frame.max(0.0) * 1000.0 / sample_rate.max(1) as f64)
    }

    pub fn point_count(&self) -> usize {
        self.count as usize
    }
}

impl GainEnvelope {
    pub fn validate(&self) -> Result<(), String> {
        if self.points.len() > MAX_GAIN_ENVELOPE_POINTS {
            return Err(format!("gain envelope supports at most {MAX_GAIN_ENVELOPE_POINTS} points"));
        }
        for (index, point) in self.points.iter().enumerate() {
            if !point.gain.is_finite() || !(0.0..=MAX_GAIN_ENVELOPE_GAIN).contains(&point.gain) {
                return Err(format!("gain envelope point {index} must have gain between 0 and 2"));
            }
            if index > 0 && self.points[index - 1].time_ms >= point.time_ms {
                return Err("gain envelope point times must be strictly increasing".to_string());
            }
        }
        Ok(())
    }

    pub fn to_rt(&self) -> Result<GainEnvelopeRt, String> {
        GainEnvelopeRt::from_envelope(self)
    }
}

/// RT-only state. A replacement snapshots the current audible multiplier and
/// crossfades to the new source-time curve, avoiding a hard gain step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainEnvelopeState {
    envelope: GainEnvelopeRt,
    transition_from: f32,
    transition_elapsed: u32,
    transition_total: u32,
}

impl Default for GainEnvelopeState {
    fn default() -> Self {
        Self {
            envelope: GainEnvelopeRt::unity(),
            transition_from: 1.0,
            transition_elapsed: 0,
            transition_total: 0,
        }
    }
}

impl GainEnvelopeState {
    pub fn with_envelope(envelope: GainEnvelopeRt) -> Self {
        Self { envelope, ..Self::default() }
    }

    pub fn envelope(&self) -> GainEnvelopeRt {
        self.envelope
    }

    fn audible_at_frame(&self, frame: f64, sample_rate: u32) -> f32 {
        let target = self.envelope.gain_at_frame(frame, sample_rate);
        if self.transition_total == 0 {
            return target;
        }
        let t = (self.transition_elapsed as f32 / self.transition_total as f32).clamp(0.0, 1.0);
        self.transition_from + (target - self.transition_from) * t
    }

    pub fn replace(&mut self, envelope: GainEnvelopeRt, frame: f64, sample_rate: u32, transition_samples: u32) {
        self.transition_from = self.audible_at_frame(frame, sample_rate);
        self.envelope = envelope;
        self.transition_elapsed = 0;
        self.transition_total = transition_samples;
        if transition_samples == 0 {
            self.transition_from = envelope.gain_at_frame(frame, sample_rate);
        }
    }

    /// Evaluate and advance by one output sample. No allocation or locking.
    pub fn next_gain(&mut self, frame: f64, sample_rate: u32) -> f32 {
        let target = self.envelope.gain_at_frame(frame, sample_rate);
        if self.transition_total == 0 {
            return target;
        }
        self.transition_elapsed = self.transition_elapsed.saturating_add(1);
        let t = (self.transition_elapsed as f32 / self.transition_total as f32).clamp(0.0, 1.0);
        let gain = self.transition_from + (target - self.transition_from) * t;
        if self.transition_elapsed >= self.transition_total {
            self.transition_total = 0;
            self.transition_elapsed = 0;
        }
        gain
    }
}

#[cfg(test)]
mod tests {
    use super::{GainEnvelope, GainEnvelopePoint, GainEnvelopeRt, GainEnvelopeState};

    #[test]
    fn empty_envelope_is_unity_and_points_interpolate_linearly_in_amplitude() {
        let empty = GainEnvelopeRt::default();
        assert_eq!(empty.gain_at_ms(0.0), 1.0);
        let envelope = GainEnvelope { points: vec![
            GainEnvelopePoint { time_ms: 100, gain: 1.0 },
            GainEnvelopePoint { time_ms: 300, gain: 0.0 },
        ] };
        let rt = envelope.to_rt().unwrap();
        assert_eq!(rt.gain_at_ms(0.0), 1.0);
        assert_eq!(rt.gain_at_ms(200.0), 0.5);
        assert_eq!(rt.gain_at_ms(400.0), 0.0);
    }

    #[test]
    fn authored_envelope_serializes_as_the_direct_point_array() {
        let envelope = GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 25, gain: 0.4 }] };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1, "serialize directly as a point array");
        assert_eq!(json[0]["time_ms"], 25);
        assert!((json[0]["gain"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert_eq!(serde_json::from_value::<GainEnvelope>(json).unwrap(), envelope);
        assert_eq!(GainEnvelope::default().to_rt().unwrap().gain_at_ms(100.0), 1.0);
    }

    #[test]
    fn rejects_invalid_points_and_over_capacity_envelopes() {
        assert!(GainEnvelope { points: vec![
            GainEnvelopePoint { time_ms: 4, gain: 0.5 },
            GainEnvelopePoint { time_ms: 4, gain: 0.8 },
        ] }.validate().is_err());
        assert!(GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 1, gain: f32::NAN }] }.validate().is_err());
        assert!(GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 1, gain: 2.01 }] }.validate().is_err());
        assert!(GainEnvelope { points: (0..129).map(|i| GainEnvelopePoint { time_ms: i, gain: 1.0 }).collect() }.validate().is_err());
    }

    #[test]
    fn accepts_two_x_boost_and_preserves_it_in_rt_and_serialized_form() {
        let envelope = GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 0, gain: 2.0 }] };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json[0]["gain"], 2.0);
        let restored: GainEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(restored, envelope);
        assert_eq!(restored.to_rt().unwrap().gain_at_ms(0.0), 2.0);
    }

    #[test]
    fn replacement_crossfades_without_a_hard_step() {
        let old = GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 0, gain: 1.0 }] }.to_rt().unwrap();
        let new = GainEnvelope { points: vec![GainEnvelopePoint { time_ms: 0, gain: 0.0 }] }.to_rt().unwrap();
        let mut state = GainEnvelopeState::with_envelope(old);
        assert_eq!(state.next_gain(0.0, 48_000), 1.0);
        state.replace(new, 0.0, 48_000, 240);
        let first = state.next_gain(0.0, 48_000);
        assert!(first < 1.0 && first > 0.99, "first replacement sample must be continuous, got {first}");
        let mut last = first;
        for _ in 1..240 { last = state.next_gain(0.0, 48_000); }
        assert!(last.abs() < 1e-6, "transition reaches replacement curve, got {last}");
    }
}
