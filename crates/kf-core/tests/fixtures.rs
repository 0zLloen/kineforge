//! Hand-computed fixtures (ROADMAP M3). References are exact rationals or
//! 50-digit values computed independently of any libm (Machin π and a
//! Decimal arctan series); the decimal strings parse to the correctly rounded
//! f64, and results must lie within a few ulps of them.

use kf_core::sensitivity::{
    cm_per_360, convert_sensitivity, horizontal_fov, monitor_distance_ratio, sens_for_cm_per_360,
    vertical_fov,
};
use kf_core::{Aspect, Degrees, FovModel, GameProfile, LinearSensitivity, Match, ValueScale};

fn ulps(a: f64, b: f64) -> u64 {
    assert!(
        a.is_finite() && b.is_finite() && (a >= 0.0) == (b >= 0.0),
        "{a} vs {b}"
    );
    (a.to_bits() as i64).abs_diff(b.to_bits() as i64)
}

#[track_caller]
fn close(got: f64, want: &str, max_ulps: u64) {
    let want: f64 = want.parse().unwrap();
    let d = ulps(got.abs(), want.abs());
    assert!(
        d <= max_ulps && (got >= 0.0) == (want >= 0.0),
        "got {got:e}, want {want:e} ({d} ulps)"
    );
}

fn deg(d: f64) -> kf_core::Radians {
    Degrees(d).to_radians()
}

/// The fixtures read the profile table, so they also pin its Source entry.
fn source() -> &'static GameProfile {
    GameProfile::find("source_sdk_2013").unwrap()
}

#[test]
fn cm_per_360_source_800_cpi() {
    // 2.54 × 360 / (0.022 × 1 × 800) = 1143/22 = 51.9545…
    let k = source().yaw_per_count().unwrap();
    let cm = cm_per_360(k, 1.0, 800.0);
    close(cm, "51.954545454545454545454545454545", 2);
    assert_eq!(format!("{cm:.2}"), "51.95");
    // Inverse: 30 cm/360 at 1600 CPI → 914.4 / (0.022 × 1600 × 30) = 381/440.
    close(
        sens_for_cm_per_360(k, 1600.0, 30.0),
        "0.86590909090909090909090909090909",
        2,
    );
    close(sens_for_cm_per_360(k, 800.0, cm), "1", 2);
    assert_eq!(k, deg(0.022));
}

#[test]
fn source_hor_plus_90_at_4_3_to_16_9() {
    // H = 2·atan((4/3)·tan 45°) = 2·atan(4/3); V = 2·atan(3/4); H + V = 180°.
    let model = source().fov.value.unwrap();
    let fov = model.resolve(deg(90.0), Aspect::R16_9);
    let h = fov.horizontal.to_degrees().0;
    close(h, "106.26020470831195740628877488181", 4);
    assert_eq!(format!("{h:.2}"), "106.26");
    close(
        fov.vertical.to_degrees().0,
        "73.739795291688042593711225118187",
        4,
    );
    // At the basis aspect the setting is the horizontal FOV.
    close(
        model
            .resolve(deg(90.0), Aspect::R4_3)
            .horizontal
            .to_degrees()
            .0,
        "90",
        4,
    );
    // Same physical ratio from pixel sizes.
    assert_eq!(
        model.resolve(deg(90.0), Aspect::new(2560, 1440).unwrap()),
        fov
    );
    close(
        model
            .setting_for_horizontal(fov.horizontal, Aspect::R16_9)
            .to_degrees()
            .0,
        "90",
        4,
    );
}

#[test]
fn horizontal_vertical_conversion() {
    // 90° horizontal at 16:9 → V = 2·atan(9/16).
    let v = vertical_fov(deg(90.0), Aspect::R16_9);
    close(v.to_degrees().0, "58.715507085582544914359511102399", 4);
    close(horizontal_fov(v, Aspect::R16_9).to_degrees().0, "90", 4);
    // Vert−: horizontal fixed; Vertical: vertical fixed.
    let vm = FovModel::VertMinus.resolve(deg(90.0), Aspect::R16_9);
    assert_eq!(vm.horizontal, deg(90.0));
    assert_eq!(vm.vertical, v);
    let vf = FovModel::Vertical.resolve(v, Aspect::R16_9);
    assert_eq!(vf.vertical, v);
    close(vf.horizontal.to_degrees().0, "90", 4);
}

#[test]
fn monitor_distance() {
    // 0 %: tan 30° / tan 45° = 1/√3.
    close(
        monitor_distance_ratio(deg(90.0), deg(60.0), 0.0),
        "0.57735026918962576450914878050196",
        4,
    );
    // 100 %: atan(tan(x)) = x, so the ratio is 60/90.
    close(
        monitor_distance_ratio(deg(90.0), deg(60.0), 1.0),
        "0.66666666666666666666666666666667",
        4,
    );
    // 50 % from 90° to 2·atan 2: atan(1) / atan(1/2).
    let to = kf_core::Radians(2.0 * libm::atan(2.0));
    close(
        monitor_distance_ratio(deg(90.0), to, 0.5),
        "1.6939549523182870227254367944114",
        4,
    );
    close(to.to_degrees().0, "126.86989764584402129685561255909", 4);
    assert_eq!(monitor_distance_ratio(deg(75.0), deg(75.0), 0.4), 1.0);
    // p small enough that p·tan underflows: the 0 % limit, not 0/0 or a
    // subnormal quotient.
    let limit = monitor_distance_ratio(deg(90.0), deg(60.0), 0.0);
    for p in [5e-324, 1e-310, -0.0] {
        assert_eq!(monitor_distance_ratio(deg(90.0), deg(60.0), p), limit);
    }
    assert_eq!(monitor_distance_ratio(deg(40.0), deg(40.0), 5e-324), 1.0);
}

#[test]
fn profile_conversions() {
    let cs2 = GameProfile::find("cs2").unwrap();
    let ow2 = GameProfile::find("overwatch2").unwrap();
    let val = GameProfile::find("valorant").unwrap();
    let conv = |s, a: &GameProfile, b: &GameProfile, how| {
        convert_sensitivity(
            s,
            a.yaw_per_count().unwrap(),
            b.yaw_per_count().unwrap(),
            how,
        )
    };
    // Equal cm/360: s₂ = s₁·k₁/k₂. 2 × 0.022/0.0066 = 20/3.
    close(
        conv(2.0, cs2, ow2, Match::Cm360),
        "6.6666666666666666666666666666667",
        4,
    );
    // VALORANT 0.4 → Overwatch 2 = 0.4 × 0.07/0.0066 = 140/33 ≈ 4.24
    // (AIMLABS_RESEARCH.md §2 and §10's "computed 4.24").
    let s = conv(0.4, val, ow2, Match::Cm360);
    close(s, "4.2424242424242424242424242424242", 4);
    // Monitor distance at 100 % from 90° to 60° with equal constants.
    close(
        conv(
            1.5,
            cs2,
            cs2,
            Match::MonitorDistance {
                p: 1.0,
                from_fov: deg(90.0),
                to_fov: deg(60.0),
            },
        ),
        "1",
        4,
    );
    // Unknown constants cannot be converted.
    assert_eq!(GameProfile::find("aimlabs").unwrap().yaw_per_count(), None);
}

#[test]
fn camera_angles_from_unit_totals() {
    let k = source().yaw_per_count().unwrap().0;
    let s = LinearSensitivity {
        yaw_rad_per_unit: k,
        pitch_rad_per_unit: source().pitch.value.unwrap().to_radians().0,
        sens_x: 1.0,
        sens_y: 1.0,
        invert_y: false,
    };
    // 1000 counts × 0.022° = 22° = 22π/180 rad.
    let yaw = s.yaw(1000, ValueScale::X1);
    close(yaw.0, "0.38397243543875250692321196906749", 2);
    // Scaling by a power of two commutes with rounding: identical bits.
    assert_eq!(s.yaw(256_000, ValueScale::X256), yaw);
    assert_eq!(s.yaw(1_024_000, ValueScale::X1024), yaw);
    assert_eq!(s.yaw(-1000, ValueScale::X1).0, -yaw.0);
    // Device +y (toward the user) looks down; invert_y flips it.
    assert_eq!(s.pitch(1000, ValueScale::X1).0, -yaw.0);
    let inv = LinearSensitivity {
        invert_y: true,
        ..s
    };
    assert_eq!(inv.pitch(1000, ValueScale::X1).0, yaw.0);
    // No overflow at the extremes (negation happens in f64).
    assert!(s.pitch(i64::MIN, ValueScale::X1).0 > 0.0);
    // At rest both axes are +0.0, as with integer negation.
    for v in [s, inv] {
        assert_eq!(v.pitch(0, ValueScale::X1).0.to_bits(), 0);
        assert_eq!(v.yaw(0, ValueScale::X1).0.to_bits(), 0);
    }
    // The header example's constant (DATA_SPECIFICATION.md §7) is 0.022° in radians.
    assert_eq!(k, 0.000_383_972_435_438_752_5);
}
