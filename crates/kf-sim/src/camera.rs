//! Camera from integer unit totals (DATA_SPECIFICATION.md §2.3 items 2–4).
//! Motion is applied one event at a time in `seq` order, never pre-summed:
//! directional gain and the pitch clamp are path-dependent. The radians are
//! always [`LinearSensitivity::yaw`]/[`pitch`](LinearSensitivity::pitch) of
//! the totals, the same functions the validator recomputes.

use crate::geom::{self, V3};
use core::f64::consts::FRAC_PI_2;
use kf_core::{LinearSensitivity, Nanos, ValueScale};
use kf_dirgain::{Engine, Profile};

/// Above this the pitch limit in units no longer has an exact `f64`, so
/// the clamp could disagree with the recomputed radians.
const MAX_PITCH_LIMIT: i64 = 1 << 52;

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    sens: LinearSensitivity,
    scale: ValueScale,
    pitch_limit: i64,
    gain: Option<Engine>,
    yaw_units: i64,
    pitch_units: i64,
    yaw: f64,
    pitch: f64,
}

/// Why a sensitivity cannot drive the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraError {
    /// An angle per unit or sensitivity that is not finite and > 0, a
    /// product (angle per unit × sensitivity) that is not normal or exceeds
    /// `f64::MAX / 2⁶⁴` (so every `i64` unit total stays finite), or gain
    /// on a source with `value_scale` ≠ 1.
    Sensitivity,
    /// The pitch limit would exceed 2⁵² units.
    PitchUnitTooSmall,
}

impl Camera {
    /// `gain`: directional gain (ODD-compatible, `kf-dirgain`, saturation
    /// policy `carry_capped`), only for sources with `units = counts` and
    /// `value_scale = 1` (the caller checks the units). The engine's sensor
    /// DPI stays 0 (unknown): the host never knows the mouse's DPI stage,
    /// so the profile's `ONLY_AT_BASE_DPI` flag has no effect here.
    pub fn new(
        sens: LinearSensitivity,
        scale: ValueScale,
        gain: Option<Profile>,
    ) -> Result<Camera, CameraError> {
        let ok = |v: f64| v.is_finite() && v > 0.0;
        let product = |v: f64| v.is_normal() && v <= f64::MAX / 18_446_744_073_709_551_616.0;
        if !(ok(sens.yaw_rad_per_unit)
            && ok(sens.pitch_rad_per_unit)
            && ok(sens.sens_x)
            && ok(sens.sens_y)
            && product(sens.yaw_rad_per_unit * sens.sens_x)
            && product(sens.pitch_rad_per_unit * sens.sens_y))
            || (gain.is_some() && scale != ValueScale::X1)
        {
            return Err(CameraError::Sensitivity);
        }
        let pitch_limit = pitch_limit_units(&sens, scale)?;
        let gain = gain.map(|p| {
            let mut e = Engine::new(p.base_dpi);
            e.set_profile(&p);
            e
        });
        let mut c = Camera {
            sens,
            scale,
            pitch_limit,
            gain,
            yaw_units: 0,
            pitch_units: 0,
            yaw: 0.0,
            pitch: 0.0,
        };
        c.recompute();
        Ok(c)
    }

    /// Applies one motion record (`dx`, `dy` in source units × value_scale,
    /// device frame: +y is toward the user). With gain, the engine's
    /// millisecond clock is `src_ts_ns` when the record has one, else the
    /// receipt `t_ns` (`approx_gain_timing`). A record whose values fit
    /// `i16` (every real HID report, −32768 included) is one engine call,
    /// exactly as the firmware sees it; a wider one is fed as successive
    /// pieces of at most ±32767 per axis (same sign as the remainder, so no
    /// overflow; at most 65 539 pieces), so no count is lost.
    pub fn apply_motion(&mut self, dx: i32, dy: i32, t_ns: Nanos, src_ts_ns: Option<Nanos>) {
        let (dx, dy) = match &mut self.gain {
            None => (i64::from(dx), i64::from(dy)),
            Some(e) => {
                let now_ms = src_ts_ns.unwrap_or(t_ns).0.div_euclid(1_000_000) as u32;
                if let (Ok(x), Ok(y)) = (i16::try_from(dx), i16::try_from(dy)) {
                    let (gx, gy) = e.process_motion(x, y, now_ms);
                    (i64::from(gx), i64::from(gy))
                } else {
                    let (mut rx, mut ry, mut ox, mut oy) = (dx, dy, 0i64, 0i64);
                    while rx != 0 || ry != 0 {
                        let (px, py) = (rx.clamp(-32767, 32767), ry.clamp(-32767, 32767));
                        let (gx, gy) = e.process_motion(px as i16, py as i16, now_ms);
                        ox += i64::from(gx);
                        oy += i64::from(gy);
                        rx -= px;
                        ry -= py;
                    }
                    (ox, oy)
                }
            }
        };
        self.yaw_units = self.yaw_units.saturating_add(dx);
        self.pitch_units = self
            .pitch_units
            .saturating_add(dy)
            .clamp(-self.pitch_limit, self.pitch_limit);
        self.recompute();
    }

    /// A new driving source is a new device stream: the gain engine's
    /// remainders and direction estimate restart (`reset_remainders` clears
    /// both). Unit totals are kept.
    pub fn reset_gain_state(&mut self) {
        if let Some(e) = &mut self.gain {
            e.reset_remainders();
        }
    }

    fn recompute(&mut self) {
        self.yaw = self.sens.yaw(self.yaw_units, self.scale).0;
        self.pitch = self.sens.pitch(self.pitch_units, self.scale).0;
    }

    pub fn yaw_units(&self) -> i64 {
        self.yaw_units
    }

    pub fn pitch_units(&self) -> i64 {
        self.pitch_units
    }

    /// ψ, positive = turned right.
    pub fn yaw_rad(&self) -> f64 {
        self.yaw
    }

    /// θ, positive = looking up, within [−π/2, π/2].
    pub fn pitch_rad(&self) -> f64 {
        self.pitch
    }

    /// The largest |pitch_units| the clamp allows.
    pub fn pitch_limit_units(&self) -> i64 {
        self.pitch_limit
    }

    pub fn forward(&self) -> V3 {
        geom::forward(self.yaw, self.pitch)
    }

    pub fn gain(&self) -> Option<&Engine> {
        self.gain.as_ref()
    }
}

/// The largest U ≥ 0 with |pitch(U)| ≤ π/2, decided by the same
/// [`LinearSensitivity::pitch`] that produces the recorded radians, so the
/// clamp and the recomputation never disagree. Starts from
/// `⌊(π/2 × scale) / (pitch_rad_per_unit × sens_y)⌋` and steps by one
/// until the predicate flips: the predicate is monotone in U (exact integers
/// below 2⁵³, monotone rounding) and only rounding separates the guess from
/// the limit, so it takes a few steps.
fn pitch_limit_units(sens: &LinearSensitivity, scale: ValueScale) -> Result<i64, CameraError> {
    let within = |u: i64| sens.pitch(u, scale).0.abs() <= FRAC_PI_2;
    let guess = (FRAC_PI_2 * f64::from(scale.get())) / (sens.pitch_rad_per_unit * sens.sens_y);
    if guess >= MAX_PITCH_LIMIT as f64 {
        return Err(CameraError::PitchUnitTooSmall);
    }
    let mut u = guess as i64;
    while u > 0 && !within(u) {
        u -= 1;
    }
    while within(u + 1) {
        u += 1;
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sens(per_unit: f64, sens_x: f64, sens_y: f64, invert_y: bool) -> LinearSensitivity {
        LinearSensitivity {
            yaw_rad_per_unit: per_unit,
            pitch_rad_per_unit: per_unit,
            sens_x,
            sens_y,
            invert_y,
        }
    }

    const SOURCE: f64 = 0.022 * (core::f64::consts::PI / 180.0);

    #[test]
    fn totals_and_radians() {
        let s = sens(SOURCE, 1.5, 1.5, false);
        let mut c = Camera::new(s, ValueScale::X1, None).unwrap();
        for (dx, dy) in [(5, -3), (-2, 7), (100, 0), (0, -4)] {
            c.apply_motion(dx, dy, Nanos(0), None);
        }
        assert_eq!((c.yaw_units(), c.pitch_units()), (103, 0));
        assert_eq!(
            c.yaw_rad().to_bits(),
            s.yaw(103, ValueScale::X1).0.to_bits()
        );
        assert_eq!(c.pitch_rad().to_bits(), 0f64.to_bits()); // +0.0, not −0.0
        // Device +y (toward the user) looks down.
        c.apply_motion(0, 10, Nanos(0), None);
        assert!(c.pitch_rad() < 0.0);
    }

    #[test]
    fn pitch_clamp_is_exact() {
        for scale in [ValueScale::X1, ValueScale::X256, ValueScale::X1024] {
            for (per, sy, inv) in [
                (SOURCE, 1.0, false),
                (SOURCE, 0.37, true),
                (1e-3, 3.0, false),
            ] {
                let s = sens(per, 1.0, sy, inv);
                let mut c = Camera::new(s, scale, None).unwrap();
                let u = c.pitch_limit_units();
                assert!(s.pitch(u, scale).0.abs() <= FRAC_PI_2);
                assert!(s.pitch(u + 1, scale).0.abs() > FRAC_PI_2);
                assert_eq!(s.pitch(-u, scale).0, -s.pitch(u, scale).0);
                // Driving far past the limit both ways stops exactly at ±U.
                c.apply_motion(0, i32::MAX, Nanos(0), None);
                c.apply_motion(0, i32::MAX, Nanos(0), None);
                assert_eq!(c.pitch_units(), u);
                for _ in 0..3 {
                    c.apply_motion(0, i32::MIN, Nanos(0), None);
                }
                assert_eq!(c.pitch_units(), -u);
                assert!(c.pitch_rad().abs() <= FRAC_PI_2);
            }
        }
    }

    #[test]
    fn clamp_is_path_dependent() {
        // Up past the limit then down: the excess is lost, unlike a sum.
        let s = sens(1e-3, 1.0, 1.0, false);
        let mut c = Camera::new(s, ValueScale::X1, None).unwrap();
        let u = c.pitch_limit_units();
        c.apply_motion(0, -(u as i32) - 500, Nanos(0), None);
        c.apply_motion(0, 500, Nanos(0), None);
        assert_eq!(c.pitch_units(), -u + 500);
    }

    #[test]
    fn rejects_bad_sensitivity() {
        let bad = [
            sens(0.0, 1.0, 1.0, false),
            sens(SOURCE, f64::NAN, 1.0, false),
            sens(SOURCE, 1.0, -1.0, false),
            sens(SOURCE, 1.0, f64::INFINITY, false),
            // Factors fine, products not: overflow and subnormal.
            sens(1e300, 1e300, 1.0, false),
            sens(1e-200, 1e-200, 1.0, false),
        ];
        for s in bad {
            assert_eq!(
                Camera::new(s, ValueScale::X1, None),
                Err(CameraError::Sensitivity)
            );
        }
        assert_eq!(
            Camera::new(sens(1e-300, 1.0, 1.0, false), ValueScale::X1, None),
            Err(CameraError::PitchUnitTooSmall)
        );
        // Gain needs value_scale 1.
        let g = Some(Profile::new_default(800));
        assert_eq!(
            Camera::new(sens(SOURCE, 1.0, 1.0, false), ValueScale::X256, g),
            Err(CameraError::Sensitivity)
        );
    }

    #[test]
    fn gain_matches_the_engine_per_event() {
        let mut p = Profile::new_default(800);
        p.flags |= kf_dirgain::flags::ENABLED;
        p.multipliers_q8_8 = [256, 300, 384, 200, 512, 256, 160, 330];
        p.sanitize_and_sign();
        let s = sens(1e-6, 1.0, 1.0, false); // pitch limit ≈ 1.57e6 units: no clamping
        let mut c = Camera::new(s, ValueScale::X1, Some(p)).unwrap();
        let mut e = Engine::new(800);
        e.set_profile(&p);
        let (mut yx, mut yy) = (0i64, 0i64);
        let events = [
            (3, 1, 1_000_000),
            (-7, 2, 2_000_000),
            (40, -9, 90_000_000),
            (1, 1, 91_000_000),
            // i16 extremes are one engine call, as in the firmware.
            (-32768, 32767, 92_000_000),
            (5, -32768, 93_000_000),
        ];
        for (dx, dy, ts) in events {
            c.apply_motion(dx, dy, Nanos(5), Some(Nanos(ts)));
            let (gx, gy) = e.process_motion(dx as i16, dy as i16, (ts / 1_000_000) as u32);
            yx += i64::from(gx);
            yy += i64::from(gy);
        }
        assert_eq!((c.yaw_units(), c.pitch_units()), (yx, yy));
        assert_eq!(c.gain(), Some(&e));
        // Without a source stamp the receipt time drives the clock.
        let mut c2 = Camera::new(s, ValueScale::X1, Some(p)).unwrap();
        c2.apply_motion(3, 1, Nanos(1_000_000), None);
        assert_eq!(c2.gain().unwrap().last_motion_ms, 1);
    }

    /// A record wider than i16 under a real (non-identity) profile is the
    /// same as feeding the engine its ±32767 pieces one by one.
    #[test]
    fn wide_deltas_with_real_gain() {
        let mut p = Profile::new_default(800);
        p.flags |= kf_dirgain::flags::ENABLED;
        p.multipliers_q8_8 = [512, 300, 384, 200, 512, 256, 160, 330];
        p.sanitize_and_sign();
        let s = sens(1e-9, 1.0, 1.0, false);
        let mut c = Camera::new(s, ValueScale::X1, Some(p)).unwrap();
        let mut e = Engine::new(800);
        e.set_profile(&p);
        let (mut yx, mut yy) = (0i64, 0i64);
        for (dx, dy) in [(100_000, -70_000), (-40_000, 5), (3, 2)] {
            c.apply_motion(dx, dy, Nanos(7_000_000), None);
            let (mut rx, mut ry) = (dx, dy);
            while rx != 0 || ry != 0 {
                let (px, py) = (rx.clamp(-32767, 32767), ry.clamp(-32767, 32767));
                let (gx, gy) = e.process_motion(px as i16, py as i16, 7);
                yx += i64::from(gx);
                yy += i64::from(gy);
                rx -= px;
                ry -= py;
            }
        }
        assert_eq!((c.yaw_units(), c.pitch_units()), (yx, yy));
        assert_eq!(c.gain(), Some(&e));
        assert_ne!(
            c.yaw_units(),
            100_000 - 40_000 + 3,
            "gain changed the totals"
        );
    }

    #[test]
    fn wide_deltas_split_without_loss() {
        // Identity profile (gain configured but 1.00x): totals are exact.
        let s = sens(1e-9, 1.0, 1.0, false); // pitch limit ≈ 1.57e9 units
        let mut c = Camera::new(s, ValueScale::X1, Some(Profile::new_default(800))).unwrap();
        c.apply_motion(100_000, -70_000, Nanos(0), None);
        c.apply_motion(i32::MIN, 3, Nanos(0), None);
        c.apply_motion(i32::MAX, -1_000_000_000, Nanos(0), None);
        c.apply_motion(i32::MAX, 1_000_000_000, Nanos(0), None);
        let yaw = 100_000 + i64::from(i32::MIN) + 2 * i64::from(i32::MAX);
        assert_eq!((c.yaw_units(), c.pitch_units()), (yaw, -70_000 + 3));
    }
}
