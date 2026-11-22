//! Property tests (round-trip conversions) over seeded random inputs, and the
//! cross-target golden hash: every sensitivity function is evaluated on a
//! fixed input set and the result bits are folded into one FNV-1a hash. The
//! same constant must come out natively on every CI runner and on wasm32
//! (`ci/test-wasi.sh`), which is ROADMAP M3's "identical results native vs
//! wasm32".

use kf_core::sensitivity::{
    cm_per_360, convert_sensitivity, horizontal_fov, monitor_distance_ratio, sens_for_cm_per_360,
    vertical_fov,
};
use kf_core::{Aspect, Degrees, FovModel, LinearSensitivity, Match, Radians, ValueScale};

/// SplitMix64: inputs are derived from integers only, so they are identical
/// on every target.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [lo, hi): 53 random bits, exact conversion.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64))
    }
    fn fov(&mut self) -> Radians {
        Degrees(self.range(1.0, 179.0)).to_radians()
    }
    /// Screen shapes from 1:2 (portrait) to 4:1. Outside that the FOV
    /// round trips are ill-conditioned near 180° and lose precision
    /// inherently, not through a bug.
    fn aspect(&mut self) -> Aspect {
        let h = 1 + (self.next() % 4320) as u32;
        let w = h / 2 + 1 + (self.next() % u64::from(4 * h - h / 2)) as u32;
        Aspect::new(w, h).unwrap()
    }
    fn model(&mut self) -> FovModel {
        match self.next() % 3 {
            0 => FovModel::HorPlus {
                basis: self.aspect(),
            },
            1 => FovModel::VertMinus,
            _ => FovModel::Vertical,
        }
    }
    /// Angle per count at sensitivity 1, 0.0001°…1°.
    fn k(&mut self) -> Radians {
        Degrees(self.range(1e-4, 1.0)).to_radians()
    }
    fn p(&mut self) -> f64 {
        match self.next() % 8 {
            0 => 0.0,
            1 => 1.0,
            _ => self.range(0.0, 1.0),
        }
    }
}

#[track_caller]
fn rel_close(got: f64, want: f64, tol: f64) {
    assert!(
        (got - want).abs() <= tol * want.abs(),
        "got {got:e}, want {want:e}, rel {:e}",
        ((got - want) / want).abs()
    );
}

const N: usize = 20_000;

#[test]
fn degree_radian_round_trip() {
    let mut r = Rng(1);
    for _ in 0..N {
        let d = r.range(-720.0, 720.0);
        rel_close(Degrees(d).to_radians().to_degrees().0, d, 1e-15);
    }
}

#[test]
fn fov_round_trips() {
    let mut r = Rng(2);
    for _ in 0..N {
        let (fov, a) = (r.fov(), r.aspect());
        rel_close(horizontal_fov(vertical_fov(fov, a), a).0, fov.0, 1e-13);
        rel_close(vertical_fov(horizontal_fov(fov, a), a).0, fov.0, 1e-13);

        let model = r.model();
        let rendered = model.resolve(fov, a);
        // Inverse on the horizontal axis.
        rel_close(
            model.setting_for_horizontal(rendered.horizontal, a).0,
            fov.0,
            1e-13,
        );
        // Both axes describe one rectilinear view.
        rel_close(
            vertical_fov(rendered.horizontal, a).0,
            rendered.vertical.0,
            1e-13,
        );
        // Hor+ keeps the vertical FOV across aspects; Vert− the horizontal.
        let b = r.aspect();
        match model {
            FovModel::HorPlus { basis } => {
                assert_eq!(model.resolve(fov, b).vertical, rendered.vertical);
                rel_close(model.resolve(fov, basis).horizontal.0, fov.0, 1e-13);
            }
            FovModel::VertMinus => assert_eq!(model.resolve(fov, b).horizontal, fov),
            FovModel::Vertical => assert_eq!(model.resolve(fov, b).vertical, fov),
        }
    }
}

#[test]
fn cm_per_360_round_trip() {
    let mut r = Rng(3);
    for _ in 0..N {
        let (k, s, cpi) = (r.k(), r.range(0.01, 100.0), r.range(100.0, 32_000.0));
        let cm = cm_per_360(k, s, cpi);
        rel_close(sens_for_cm_per_360(k, cpi, cm), s, 1e-14);
        // cm/360 is inversely proportional to sensitivity and CPI.
        rel_close(cm_per_360(k, 2.0 * s, cpi), 0.5 * cm, 1e-15);
    }
}

#[test]
fn sensitivity_conversion_round_trip() {
    let mut r = Rng(4);
    for _ in 0..N {
        let (s, ka, kb) = (r.range(0.01, 100.0), r.k(), r.k());
        let (fa, fb, p) = (r.fov(), r.fov(), r.p());
        let there = Match::MonitorDistance {
            p,
            from_fov: fa,
            to_fov: fb,
        };
        let back = Match::MonitorDistance {
            p,
            from_fov: fb,
            to_fov: fa,
        };
        let s2 = convert_sensitivity(s, ka, kb, there);
        rel_close(convert_sensitivity(s2, kb, ka, back), s, 1e-13);
        let s3 = convert_sensitivity(s, ka, kb, Match::Cm360);
        rel_close(convert_sensitivity(s3, kb, ka, Match::Cm360), s, 1e-15);
        // Equal cm/360 means equal angle per count.
        rel_close(s3 * kb.0, s * ka.0, 1e-15);
        // Monitor distance: 100 % is the FOV ratio; small p tends to the 0 % limit.
        rel_close(monitor_distance_ratio(fa, fb, 1.0), fb.0 / fa.0, 1e-14);
        rel_close(
            monitor_distance_ratio(fa, fb, 1e-9),
            monitor_distance_ratio(fa, fb, 0.0),
            1e-12,
        );
    }
}

/// FNV-1a over the result bits of every function on a fixed input set.
#[test]
fn golden_hash_identical_on_every_target() {
    let mut r = Rng(0x5eed_0003);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut fold = |x: f64| {
        for b in x.to_bits().to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for _ in 0..N {
        let (fov, a, model) = (r.fov(), r.aspect(), r.model());
        let (k, s, cpi, p) = (r.k(), r.range(0.01, 100.0), r.range(100.0, 32_000.0), r.p());
        let d = r.range(-720.0, 720.0);
        fold(Degrees(d).to_radians().0);
        fold(Radians(d).to_degrees().0);
        fold(a.ratio());
        fold(a.ratio_to(r.aspect()));
        fold(vertical_fov(fov, a).0);
        fold(horizontal_fov(fov, a).0);
        let rendered = model.resolve(fov, a);
        fold(rendered.horizontal.0);
        fold(rendered.vertical.0);
        fold(model.setting_for_horizontal(fov, a).0);
        fold(cm_per_360(k, s, cpi));
        fold(sens_for_cm_per_360(k, cpi, r.range(1.0, 200.0)));
        let other = r.fov();
        fold(monitor_distance_ratio(fov, other, p));
        fold(convert_sensitivity(s, k, r.k(), Match::Cm360));
        fold(convert_sensitivity(
            s,
            k,
            r.k(),
            Match::MonitorDistance {
                p,
                from_fov: fov,
                to_fov: other,
            },
        ));
        let lin = LinearSensitivity {
            yaw_rad_per_unit: k.0,
            pitch_rad_per_unit: r.k().0,
            sens_x: s,
            sens_y: r.range(0.01, 100.0),
            invert_y: r.next() & 1 == 1,
        };
        let units = r.next() as i64 >> (r.next() % 64);
        let scale = [ValueScale::X1, ValueScale::X256, ValueScale::X1024][(r.next() % 3) as usize];
        fold(lin.yaw(units, scale).0);
        fold(lin.pitch(units, scale).0);
    }
    assert_eq!(h, GOLDEN, "golden hash {h:#018x}");
}

const GOLDEN: u64 = 0xc463_5f9b_f77f_a5aa;
