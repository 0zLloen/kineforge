//! Runtime engine: octant scale, travel-window direction estimator,
//! remainder carry and per-report processing (`.c:268-664`).

use crate::{Profile, Q16_16_ONE, dir, flags};

/// Runtime engine state (`ddpi_engine_t`, `.h:125-140`). Every field is
/// public and named as in C so a state can be compared field by field with
/// the reference engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Engine {
    /// Active profile; [`Engine::set_profile`] sanitises it.
    pub profile: Profile,
    /// Fractional X accumulator, Q16.16.
    pub rem_x_q16_16: i32,
    /// Fractional Y accumulator, Q16.16.
    pub rem_y_q16_16: i32,
    /// Travel-window direction estimate, X. Despite the C name it holds
    /// counts × 256 (Q8), bounded to |d| <= 16383; the name is kept for
    /// field-by-field comparison with C.
    pub dir_x_q16_16: i32,
    /// Travel-window direction estimate, Y (counts × 256, see `dir_x_q16_16`).
    pub dir_y_q16_16: i32,
    /// Most recent octant index (0..=7).
    pub last_octant: u8,
    /// Most recent effective scale, Q16.16.
    pub last_scale_q16_16: u32,
    /// Timestamp of the last non-zero motion seen by [`Engine::process_motion`].
    pub last_motion_ms: u32,
    /// Current sensor DPI reported by the adapter (0 = unknown).
    pub sensor_dpi: u16,
    /// 1 once the adapter reported its clock ([`Engine::set_clock_ms`]).
    pub clock_known: u8,
    /// Adapter's millisecond clock as last reported.
    pub clock_ms: u32,
}

impl Engine {
    /// Neutral engine with a disabled 1.00x profile.
    /// Mirrors `ddpi_engine_init` (`.c:268-283`).
    pub fn new(base_dpi: u16) -> Self {
        Engine {
            profile: Profile::new_default(base_dpi),
            rem_x_q16_16: 0,
            rem_y_q16_16: 0,
            dir_x_q16_16: 0,
            dir_y_q16_16: 0,
            last_octant: 0,
            last_scale_q16_16: Q16_16_ONE,
            last_motion_ms: 0,
            sensor_dpi: 0,
            clock_known: 0,
            clock_ms: 0,
        }
    }

    /// Clears the remainders and the direction estimate.
    /// Mirrors `ddpi_engine_reset_remainders` (`.c:285-293`).
    pub fn reset_remainders(&mut self) {
        self.rem_x_q16_16 = 0;
        self.rem_y_q16_16 = 0;
        self.dir_x_q16_16 = 0;
        self.dir_y_q16_16 = 0;
    }

    /// Sets the DPI the sensor runs at (0 = unknown); a change clears the
    /// remainders and direction. Mirrors `ddpi_engine_set_sensor_dpi`
    /// (`.c:295-301`).
    pub fn set_sensor_dpi(&mut self, sensor_dpi: u16) {
        if self.sensor_dpi == sensor_dpi {
            return;
        }
        self.reset_remainders();
        self.sensor_dpi = sensor_dpi;
    }

    /// Records the adapter's millisecond clock for GET_CAPABILITIES.
    /// Mirrors `ddpi_engine_set_clock_ms` (`.c:303-309`).
    pub fn set_clock_ms(&mut self, now_ms: u32) {
        self.clock_known = 1;
        self.clock_ms = now_ms;
    }

    /// Curve off: identity profile, or bound to another DPI stage than the
    /// current one. Mirrors `ddpi_curve_off` (`.c:311-317`).
    fn curve_off(&self) -> bool {
        let p = &self.profile;
        p.is_identity()
            || (p.flags & flags::ONLY_AT_BASE_DPI != 0
                && self.sensor_dpi != 0
                && self.sensor_dpi != p.base_dpi)
    }

    /// Clears the direction estimate after the sensor was idle.
    /// Mirrors `ddpi_engine_notify_idle` (`.c:319-325`).
    pub fn notify_idle(&mut self) {
        self.dir_x_q16_16 = 0;
        self.dir_y_q16_16 = 0;
    }

    /// Loads a copy of `profile`, sanitised and re-signed; state is reset
    /// only if the result is an identity profile. Mirrors
    /// `ddpi_engine_set_profile` (`.c:327-336`).
    pub fn set_profile(&mut self, profile: &Profile) {
        self.profile = *profile;
        self.profile.sanitize_and_sign();
        if self.profile.is_identity() {
            self.reset_remainders();
        }
    }

    /// Updates the travel-window direction estimate `d` with one sample and
    /// returns it as the `i16` direction for [`compute_scale_q16_16`].
    /// `α = 2L / (2L + |dx| + |dy|)`, the first-order approximation of
    /// `exp(-s / 2L)` (the C comment's "second-order Padé" is wrong; audit
    /// §3.5). Mirrors `update_direction_estimator` (`.c:513-572`).
    fn update_direction_estimator(&mut self, in_dx: i16, in_dy: i16) -> (i16, i16) {
        let l = self.profile.window_counts();
        if l == 0 {
            return (in_dx, in_dy);
        }

        let step_l1 = u32::from(in_dx.unsigned_abs()) + u32::from(in_dy.unsigned_abs());
        if step_l1 == 0 {
            return (0, 0);
        }

        let mut next_x = i64::from(in_dx) * 256;
        let mut next_y = i64::from(in_dy) * 256;

        if self.dir_x_q16_16 != 0 || self.dir_y_q16_16 != 0 {
            let two_l = u32::from(l) * 2;
            let alpha_q16 = (two_l << 16) / (two_l + step_l1);
            // `>>` on negative i64 floors, like the C arithmetic shift.
            next_x += (i64::from(self.dir_x_q16_16) * i64::from(alpha_q16)) >> 16;
            next_y += (i64::from(self.dir_y_q16_16) * i64::from(alpha_q16)) >> 16;
        }

        // Keep |d| <= 16383 so the i16 estimate never wraps; halving both
        // axes keeps the angle. Like the C, the magnitudes come from the
        // value truncated to i32.
        let mut mag_x = (next_x as i32).unsigned_abs();
        let mut mag_y = (next_y as i32).unsigned_abs();
        while mag_x > 16383 || mag_y > 16383 {
            next_x >>= 1;
            next_y >>= 1;
            mag_x >>= 1;
            mag_y >>= 1;
        }
        self.dir_x_q16_16 = next_x as i32;
        self.dir_y_q16_16 = next_y as i32;

        (self.dir_x_q16_16 as i16, self.dir_y_q16_16 as i16)
    }

    /// Scales one sensor sample and returns the HID deltas, with direction
    /// estimation, sub-count remainders and ±32767 saturation carry.
    /// Mirrors `ddpi_process_delta` (`.c:574-647`).
    pub fn process_delta(&mut self, in_dx: i16, in_dy: i16) -> (i16, i16) {
        // Exact 1.00x bypass; state (remainders, direction) is kept.
        if self.curve_off() {
            self.last_scale_q16_16 = Q16_16_ONE;
            return (in_dx.max(-32767), in_dy.max(-32767));
        }

        if in_dx == 0 && in_dy == 0 {
            return (0, 0);
        }

        let reset_rem = self.profile.flags & flags::RESET_REMAINDER_ON_REVERSE != 0;

        if self.profile.flags & flags::MODEL_LINEAR_2X2 != 0 {
            let gain = u32::from(self.profile.global_gain_q8_8);
            let g = |idx| (u32::from(self.profile.sector_multiplier(idx)) * gain) as i32;
            let gx_q16 = g(dir::E);
            let gy_q16 = g(dir::N);
            let gne_q16 = g(dir::NE);
            let gnw_q16 = g(dir::NW);
            // Wrapping only matters for an unsanitised profile written into
            // the pub field (M·G >= 2^31), where the C overflows (UB).
            let shear_q16 = gne_q16.wrapping_sub(gnw_q16) / 2;

            let scaled_x =
                i64::from(in_dx) * i64::from(gx_q16) - i64::from(in_dy) * i64::from(shear_q16);
            let scaled_y =
                i64::from(in_dy) * i64::from(gy_q16) - i64::from(in_dx) * i64::from(shear_q16);

            self.last_scale_q16_16 = (gx_q16.wrapping_add(gy_q16) / 2) as u32;
            let out_dx =
                emit_from_q16_with_remainder(scaled_x, in_dx, &mut self.rem_x_q16_16, reset_rem);
            let out_dy =
                emit_from_q16_with_remainder(scaled_y, in_dy, &mut self.rem_y_q16_16, reset_rem);
            return (out_dx, out_dy);
        }

        let (est_dx, est_dy) = self.update_direction_estimator(in_dx, in_dy);
        let (scale_q16_16, octant) = compute_scale_q16_16(&self.profile, est_dx, est_dy);
        self.last_octant = octant;
        self.last_scale_q16_16 = scale_q16_16;

        let scaled_x = i64::from(in_dx) * i64::from(scale_q16_16);
        let scaled_y = i64::from(in_dy) * i64::from(scale_q16_16);
        let out_dx =
            emit_from_q16_with_remainder(scaled_x, in_dx, &mut self.rem_x_q16_16, reset_rem);
        let out_dy =
            emit_from_q16_with_remainder(scaled_y, in_dy, &mut self.rem_y_q16_16, reset_rem);
        (out_dx, out_dy)
    }

    /// [`Engine::process_delta`] plus the idle-gap reset: if no motion was
    /// seen for [`Profile::idle_ms`] before `now_ms` (any wrapping
    /// millisecond clock) the direction estimate restarts. Mirrors
    /// `ddpi_process_motion` (`.c:649-664`).
    pub fn process_motion(&mut self, in_dx: i16, in_dy: i16, now_ms: u32) -> (i16, i16) {
        if in_dx != 0 || in_dy != 0 {
            if now_ms.wrapping_sub(self.last_motion_ms) >= self.profile.idle_ms() {
                self.notify_idle();
            }
            self.last_motion_ms = now_ms;
        }
        self.process_delta(in_dx, in_dy)
    }
}

/// Directional scale (Q16.16) for the direction `(dx, dy)`, interpolated
/// between the cardinal and diagonal multipliers of its octant, and the
/// octant index (0..=7). `(65536, 0)` for a zero vector or an identity
/// profile. Mirrors `ddpi_compute_scale_q16_16` (`.c:347-461`).
pub fn compute_scale_q16_16(profile: &Profile, dx: i16, dy: i16) -> (u32, u8) {
    if (dx == 0 && dy == 0) || profile.is_identity() {
        return (Q16_16_ONE, 0);
    }

    let ax = u32::from(dx.unsigned_abs());
    let ay = u32::from(dy.unsigned_abs());

    // (cardinal, diagonal, octant, u, v): u is the larger magnitude, v the
    // smaller; zero counts as non-negative.
    let (card_idx, diag_idx, octant, u, v) = match (dx >= 0, dy >= 0) {
        (true, true) if ax >= ay => (dir::E, dir::SE, 0, ax, ay),
        (true, true) => (dir::S, dir::SE, 1, ay, ax),
        (false, true) if ay >= ax => (dir::S, dir::SW, 2, ay, ax),
        (false, true) => (dir::W, dir::SW, 3, ax, ay),
        (false, false) if ax >= ay => (dir::W, dir::NW, 4, ax, ay),
        (false, false) => (dir::N, dir::NW, 5, ay, ax),
        (true, false) if ay >= ax => (dir::N, dir::NE, 6, ay, ax),
        (true, false) => (dir::E, dir::NE, 7, ax, ay),
    };

    // Tangent ratio r = v / u in Q16.16, 0..=65536. u >= v in every octant,
    // so u > 0 whenever v > 0 and the division cannot be by zero.
    let r: u32 = if v >= u && u > 0 {
        65536
    } else if v > 0 {
        (v << 16) / u
    } else {
        0
    };
    let mut t = r;

    if profile.flags & flags::LINEAR_OCTANT_RATIO == 0 && (1..65536).contains(&r) {
        // Circular interpolation: (4/pi)·atan(x) ≈ x + 0.3477·x·(1 - x),
        // 0.3477 · 65536 = 22787.
        let one_minus_r = u64::from(65536 - r);
        let corr = (22787 * u64::from(r) * one_minus_r) >> 32;
        t = (r + corr as u32).min(65536);
    }

    let m_card_q16 = (u32::from(profile.sector_multiplier(card_idx)) << 8) as i32;
    let m_diag_q16 = (u32::from(profile.sector_multiplier(diag_idx)) << 8) as i32;
    let diff_q16 = m_diag_q16 - m_card_q16;
    // `>>` on negative i64 floors, like the C arithmetic shift.
    let m_dir_q16 = m_card_q16 + ((i64::from(diff_q16) * i64::from(t)) >> 16) as i32;

    let scaled_q16 =
        ((u64::from(m_dir_q16 as u32) * u64::from(profile.global_gain_q8_8)) >> 8) as u32;
    (scaled_q16, octant)
}

/// Adds the axis remainder to `scaled_delta_q16`, emits the integer part
/// (truncated toward zero, clamped to ±32767) and carries the rest in the
/// remainder, including any excess beyond the clamp, capped at ±2147418112.
/// Mirrors `emit_from_q16_with_remainder` (`.c:482-511`) with
/// `div_i64_by_65536_trunc` (`.c:463-470`) and `clamp_rem_i32` (`.c:472-480`).
fn emit_from_q16_with_remainder(
    scaled_delta_q16: i64,
    raw_delta: i16,
    rem_q16_16: &mut i32,
    reset_rem_on_reverse: bool,
) -> i16 {
    if scaled_delta_q16 == 0 && raw_delta == 0 {
        return 0;
    }

    if reset_rem_on_reverse
        && ((raw_delta > 0 && *rem_q16_16 < 0) || (raw_delta < 0 && *rem_q16_16 > 0))
    {
        *rem_q16_16 = 0;
    }

    let total_q16 = scaled_delta_q16 + i64::from(*rem_q16_16);
    // Rust `/` truncates toward zero, exactly like div_i64_by_65536_trunc.
    let out_int = (total_q16 / 65536).clamp(-32767, 32767);

    let new_rem = total_q16 - out_int * 65536;
    *rem_q16_16 = new_rem.clamp(-2_147_418_112, 2_147_418_112) as i32;
    out_int as i16
}
