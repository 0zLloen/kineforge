//! Sensitivity mathematics (AIMLABS_RESEARCH.md §4; DATA_SPECIFICATION.md
//! §2.3–2.4). Angles are radians.
//!
//! **Determinism.** Only IEEE-exact operations plus the pinned `libm` for
//! `tan`/`atan`, in the operation order written in each doc comment, so every
//! target produces the same bits (tests/properties.rs checks a golden hash
//! natively and on wasm32).
//!
//! **Domain.** Inputs are finite; angles per count, sensitivities, CPI and
//! cm/360 are > 0; FOVs are in (0, π). Out-of-domain inputs give meaningless
//! results (inf, NaN, negative): callers that take user input validate it.

use crate::units::{Aspect, Radians, ValueScale};
use core::f64::consts::TAU;

/// 1 in = 2.54 cm exactly (DATA_SPECIFICATION.md §2.4).
pub const CM_PER_INCH: f64 = 2.54;

/// The header's `sensitivity` object, model `linear` (DATA_SPECIFICATION.md
/// §2.3 item 4): camera angles from integer unit totals. Immutable per session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearSensitivity {
    /// Yaw per source unit at sensitivity 1.
    pub yaw_rad_per_unit: f64,
    /// Pitch per source unit at sensitivity 1.
    pub pitch_rad_per_unit: f64,
    pub sens_x: f64,
    pub sens_y: f64,
    pub invert_y: bool,
}

impl LinearSensitivity {
    /// ψ = (yaw_units as f64 × (yaw_rad_per_unit × sens_x)) / value_scale;
    /// positive = turn right.
    pub fn yaw(&self, yaw_units: i64, scale: ValueScale) -> Radians {
        Radians(yaw_units as f64 * (self.yaw_rad_per_unit * self.sens_x) / f64::from(scale.get()))
    }

    /// θ = (−(pitch_units as f64) × (pitch_rad_per_unit × sens_y)) /
    /// value_scale, negated again if `invert_y`, then `+ 0.0`; positive =
    /// look up. Device +y is motion toward the user, so it looks down. The
    /// negation happens in f64 (no overflow at `i64::MIN`); the final `+ 0.0`
    /// turns −0.0 at rest into +0.0, matching an integer-negation reading of
    /// the formula. The pitch clamp is the simulation's job (in unit space,
    /// per event).
    pub fn pitch(&self, pitch_units: i64, scale: ValueScale) -> Radians {
        let up = -(pitch_units as f64) * (self.pitch_rad_per_unit * self.sens_y)
            / f64::from(scale.get());
        Radians((if self.invert_y { -up } else { up }) + 0.0)
    }
}

/// Mouse travel for one full turn, in cm:
/// `(2.54 × 2π) / ((per_count × sens) × cpi)`. `per_count` is the angle per
/// count at sensitivity 1 (Source: 0.022°, see [`GameProfile::yaw_per_count`]).
///
/// [`GameProfile::yaw_per_count`]: crate::GameProfile::yaw_per_count
pub fn cm_per_360(per_count: Radians, sens: f64, cpi: f64) -> f64 {
    CM_PER_INCH * TAU / (per_count.0 * sens * cpi)
}

/// The sensitivity that gives `cm` per 360°:
/// `(2.54 × 2π) / ((per_count × cpi) × cm)`.
pub fn sens_for_cm_per_360(per_count: Radians, cpi: f64, cm: f64) -> f64 {
    CM_PER_INCH * TAU / (per_count.0 * cpi * cm)
}

/// Vertical FOV of a rectilinear view with horizontal FOV `h` at `aspect`:
/// `2 × atan(tan(h × 0.5) / aspect.ratio())`.
pub fn vertical_fov(h: Radians, aspect: Aspect) -> Radians {
    Radians(2.0 * libm::atan(libm::tan(h.0 * 0.5) / aspect.ratio()))
}

/// Horizontal FOV of a rectilinear view with vertical FOV `v` at `aspect`:
/// `2 × atan(tan(v × 0.5) × aspect.ratio())`.
pub fn horizontal_fov(v: Radians, aspect: Aspect) -> Radians {
    Radians(2.0 * libm::atan(libm::tan(v.0 * 0.5) * aspect.ratio()))
}

/// FOV on both axes for one aspect ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fov {
    pub horizontal: Radians,
    pub vertical: Radians,
}

/// How an application's FOV setting maps to the rendered FOV as the aspect
/// ratio changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FovModel {
    /// The setting is the horizontal FOV at `basis`; the vertical FOV is kept
    /// at other aspects, so wider screens see more horizontally (Hor+).
    /// Source engine: basis 4:3 (`ScaleFOVByWidthRatio`), except that the
    /// engine caps the aspect at 1.85:1 when `sv_restrict_aspect_ratio_fov`
    /// is 2, or > 0 in windowed multiplayer; this model has no cap.
    HorPlus { basis: Aspect },
    /// The setting is the horizontal FOV at every aspect; wider screens lose
    /// vertical FOV (Vert−).
    VertMinus,
    /// The setting is the vertical FOV at every aspect (Hor+ stated by its
    /// fixed axis).
    Vertical,
}

impl FovModel {
    /// Rendered FOV for `setting` at `aspect`. Hor+: horizontal =
    /// `2 × atan(tan(setting × 0.5) × aspect.ratio_to(basis))`, vertical =
    /// [`vertical_fov`]`(setting, basis)`.
    pub fn resolve(self, setting: Radians, aspect: Aspect) -> Fov {
        match self {
            FovModel::HorPlus { basis } => Fov {
                horizontal: Radians(
                    2.0 * libm::atan(libm::tan(setting.0 * 0.5) * aspect.ratio_to(basis)),
                ),
                vertical: vertical_fov(setting, basis),
            },
            FovModel::VertMinus => Fov {
                horizontal: setting,
                vertical: vertical_fov(setting, aspect),
            },
            FovModel::Vertical => Fov {
                horizontal: horizontal_fov(setting, aspect),
                vertical: setting,
            },
        }
    }

    /// The setting that renders horizontal FOV `h` at `aspect` (inverse of
    /// [`FovModel::resolve`]'s horizontal axis). Hor+:
    /// `2 × atan(tan(h × 0.5) / aspect.ratio_to(basis))`.
    pub fn setting_for_horizontal(self, h: Radians, aspect: Aspect) -> Radians {
        match self {
            FovModel::HorPlus { basis } => {
                Radians(2.0 * libm::atan(libm::tan(h.0 * 0.5) / aspect.ratio_to(basis)))
            }
            FovModel::VertMinus => h,
            FovModel::Vertical => vertical_fov(h, aspect),
        }
    }
}

/// Angle-per-count ratio (to / from) that keeps the mouse travel to a point
/// at fraction `p` of the half-screen equal between FOVs `from` and `to`
/// (monitor-distance match): `atan(p × tan(to × 0.5)) / atan(p × tan(from ×
/// 0.5))`. `p = 0` is the limit `tan(to × 0.5) / tan(from × 0.5)` (0 %,
/// focal-length / zoom-ratio match); `p = 1` matches the screen edge, where
/// the ratio is `to / from`. `p ≥ 0`.
///
/// `from` and `to` are **rendered** FOVs (each configuration's
/// [`FovModel::resolve`] at the aspect it is played at), not FOV settings,
/// and both on the matched axis: horizontal FOVs with half-width fractions,
/// vertical with half-height. Passing a Hor+ setting instead of its
/// rendered value gives a large silent error.
///
/// When `p × tan` underflows below the smallest normal f64 (p = 0 included)
/// the 0 % limit is returned: there atan is the identity, so the limit and
/// the formula agree, and the subnormal quotient would lose precision.
pub fn monitor_distance_ratio(from: Radians, to: Radians, p: f64) -> f64 {
    let (t_from, t_to) = (libm::tan(from.0 * 0.5), libm::tan(to.0 * 0.5));
    let (a, b) = (p * t_to, p * t_from);
    if a.min(b) < f64::MIN_POSITIVE {
        t_to / t_from
    } else {
        libm::atan(a) / libm::atan(b)
    }
}

/// What two configurations should have in common.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Match {
    /// The same angle per count: equal cm/360 at equal CPI.
    Cm360,
    /// Monitor-distance match at fraction `p` (see [`monitor_distance_ratio`]);
    /// `from_fov` and `to_fov` are the rendered FOVs (not settings) on the
    /// axis being matched.
    MonitorDistance {
        p: f64,
        from_fov: Radians,
        to_fov: Radians,
    },
}

/// Sensitivity in the target configuration:
/// `((sens × from_per_count) × ratio) / to_per_count`, with ratio 1 for
/// [`Match::Cm360`]. Angles per count are at sensitivity 1, with the same CPI
/// on both sides.
pub fn convert_sensitivity(
    sens: f64,
    from_per_count: Radians,
    to_per_count: Radians,
    how: Match,
) -> f64 {
    let ratio = match how {
        Match::Cm360 => 1.0,
        Match::MonitorDistance {
            p,
            from_fov,
            to_fov,
        } => monitor_distance_ratio(from_fov, to_fov, p),
    };
    sens * from_per_count.0 * ratio / to_per_count.0
}
