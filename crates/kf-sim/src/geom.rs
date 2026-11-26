//! World geometry (DATA_SPECIFICATION.md §2.2): right-handed, metres, +X
//! right, +Y up, −Z forward at yaw 0; the eye sits at the origin. Every
//! operation is written out in a fixed order with IEEE-exact arithmetic
//! (`+ − × ÷ √`) and the pinned `libm` for trigonometry, so all targets
//! compute the same bits. No fused multiply-add: Rust never contracts
//! `a * b + c` on its own.

/// A 3-vector in metres (or a direction).
pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `(a₀b₀ + a₁b₁) + a₂b₂`.
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// Camera forward for yaw ψ (positive = right) and pitch θ (positive = up):
/// `(sin ψ cos θ, sin θ, −cos ψ cos θ)`, a unit vector up to rounding.
pub fn forward(yaw: f64, pitch: f64) -> V3 {
    let (sy, cy) = (libm::sin(yaw), libm::cos(yaw));
    let (sp, cp) = (libm::sin(pitch), libm::cos(pitch));
    [sy * cp, sp, -(cy * cp)]
}

/// Angle between two non-zero vectors, `atan2(|a × b|, a · b)` (accurate
/// for small angles, unlike `acos`).
pub fn angle_between(a: V3, b: V3) -> f64 {
    libm::atan2(norm(cross(a, b)), dot(a, b))
}

/// Entry distance along the unit ray `o + t·d` (t ≥ 0) into the sphere
/// (centre `c`, radius `r`), boundary included; 0 when `o` is inside.
/// `b = (c − o)·d`, `disc = b² − (|c − o|² − r²)`; hit iff `disc ≥ 0` and
/// the far root `b + √disc ≥ 0`; entry = `max(b − √disc, 0)`.
pub fn ray_sphere(o: V3, d: V3, c: V3, r: f64) -> Option<f64> {
    let oc = sub(c, o);
    let b = dot(oc, d);
    let disc = b * b - (dot(oc, oc) - r * r);
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    if b + s < 0.0 {
        return None;
    }
    Some(nonneg(b - s))
}

/// Entry distance along the ray `o + t·d` (t ≥ 0) into the axis-aligned
/// cube with centre `c` and half size `h`, boundary included; 0 when `o`
/// is inside. Slab test; an axis with `d_i == 0` is a miss iff `o_i` lies
/// outside the slab (no division by zero, no `∞ × 0`).
pub fn ray_box(o: V3, d: V3, c: V3, h: f64) -> Option<f64> {
    let (mut enter, mut exit) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..3 {
        let (lo, hi) = (c[i] - h, c[i] + h);
        if d[i] == 0.0 {
            if o[i] < lo || o[i] > hi {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((lo - o[i]) / d[i], (hi - o[i]) / d[i]);
        let (near, far) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
        if near > enter {
            enter = near;
        }
        if far < exit {
            exit = far;
        }
    }
    if enter > exit || exit < 0.0 {
        return None;
    }
    Some(nonneg(enter))
}

/// `t` if positive, else `+0.0`. Written as a comparison: `f64::max` may
/// return either zero for ±0.0, and targets differ in which.
fn nonneg(t: f64) -> f64 {
    if t > 0.0 { t } else { 0.0 }
}

/// Target-relative angles of the point `p` (relative to the eye) for a
/// camera at yaw ψ, pitch θ: `(d_yaw, d_pitch)` with d_yaw ∈ [−π, π] the
/// turn right needed and d_pitch the elevation of `p` minus θ
/// (DATA_SPECIFICATION.md §2.2 "Target-relative"). `p` is first rotated by
/// −ψ about +Y so the camera's yaw maps to −Z: `x' = cos ψ·x + sin ψ·z`,
/// `z' = cos ψ·z − sin ψ·x`; then `d_yaw = atan2(x', −z')`, elevation
/// `atan2(y, hypot(x', z'))`.
pub fn relative_angles(p: V3, yaw: f64, pitch: f64) -> (f64, f64) {
    let (sy, cy) = (libm::sin(yaw), libm::cos(yaw));
    let x = cy * p[0] + sy * p[2];
    let z = cy * p[2] - sy * p[0];
    let d_yaw = libm::atan2(x, -z);
    let elevation = libm::atan2(p[1], libm::hypot(x, z));
    (d_yaw, elevation - pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    const FWD: V3 = [0.0, 0.0, -1.0];
    const O: V3 = [0.0; 3];

    #[test]
    fn forward_axes() {
        assert_eq!(forward(0.0, 0.0), [0.0, 0.0, -1.0]);
        let r = forward(FRAC_PI_2, 0.0); // turn right → +X
        assert!((r[0] - 1.0).abs() < 1e-15 && r[2].abs() < 1e-15);
        let u = forward(0.0, FRAC_PI_2); // look up → +Y
        assert!((u[1] - 1.0).abs() < 1e-15 && u[0] == 0.0);
    }

    #[test]
    fn sphere_cases() {
        // Straight ahead, 10 m, radius 1: entry at 9.
        assert_eq!(ray_sphere(O, FWD, [0.0, 0.0, -10.0], 1.0), Some(9.0));
        // Behind the eye: miss.
        assert_eq!(ray_sphere(O, FWD, [0.0, 0.0, 10.0], 1.0), None);
        // Tangent (disc = 0) counts; just outside does not.
        assert_eq!(ray_sphere(O, FWD, [1.0, 0.0, -10.0], 1.0), Some(10.0));
        assert_eq!(ray_sphere(O, FWD, [1.0 + 1e-12, 0.0, -10.0], 1.0), None);
        // Eye inside the sphere: entry 0.
        assert_eq!(ray_sphere(O, FWD, [0.0, 0.0, -0.5], 1.0), Some(0.0));
        // Sphere surrounding the eye but centred behind: still inside → 0.
        assert_eq!(ray_sphere(O, FWD, [0.0, 0.0, 0.5], 1.0), Some(0.0));
    }

    #[test]
    fn box_cases() {
        // Axis-parallel ray: the other two axes have d = 0.
        assert_eq!(ray_box(O, FWD, [0.0, 0.0, -10.0], 1.0), Some(9.0));
        // Ray along the face plane (o on the slab boundary) counts.
        assert_eq!(ray_box(O, FWD, [1.0, 0.0, -10.0], 1.0), Some(9.0));
        assert_eq!(ray_box(O, FWD, [1.0 + 1e-12, 0.0, -10.0], 1.0), None);
        assert_eq!(ray_box(O, FWD, [0.0, 0.0, 10.0], 1.0), None);
        assert_eq!(ray_box(O, FWD, [0.0, 0.0, -0.5], 1.0), Some(0.0));
        // Diagonal ray through the edge region.
        let d = [libm::sqrt(0.5), 0.0, -libm::sqrt(0.5)];
        let t = ray_box(O, d, [5.0, 0.0, -5.0], 0.5).unwrap();
        assert!((t - (4.5 * 2f64.sqrt())).abs() < 1e-12, "{t}");
        assert_eq!(ray_box(O, d, [5.0, 2.0, -5.0], 0.5), None);
    }

    #[test]
    fn angles() {
        assert_eq!(angle_between(FWD, FWD), 0.0);
        assert!((angle_between(FWD, [1.0, 0.0, 0.0]) - FRAC_PI_2).abs() < 1e-15);
        assert!((angle_between(FWD, [0.0, 0.0, 1.0]) - PI).abs() < 1e-15);
        // Target 45° right and level: turn right by π/4, no pitch change.
        let (dy, dp) = relative_angles([1.0, 0.0, -1.0], 0.0, 0.0);
        assert!((dy - FRAC_PI_4).abs() < 1e-15 && dp == 0.0);
        // Camera already turned 45° right: no turn needed.
        let (dy, _) = relative_angles([1.0, 0.0, -1.0], FRAC_PI_4, 0.0);
        assert!(dy.abs() < 1e-15, "{dy}");
        // Several full turns of yaw do not change the relative angle.
        let (dy2, _) = relative_angles([1.0, 0.0, -1.0], FRAC_PI_4 + 6.0 * PI, 0.0);
        assert!(dy2.abs() < 1e-12, "{dy2}");
        // Target above while looking 10° down.
        let (_, dp) = relative_angles([0.0, 1.0, -1.0], 0.0, -10f64.to_radians());
        assert!((dp - (FRAC_PI_4 + 10f64.to_radians())).abs() < 1e-15);
    }
}
