//! One-to-one port of the 17 C unit tests in `tests/test_directional_dpi.c`
//! (Open Directional DPI, commit `c177d23`): same names, inputs, expected
//! values and assertions. The C `NULL` callbacks become `None`. One extra
//! test at the end (not in the C file) pins the CRC-16 check value.

use kf_dirgain::{
    DPI_MAX, Engine, HID_NAMESPACE, HID_PACKET_LEN, PROFILE_MAGIC, PROFILE_VERSION, Profile,
    Q8_8_ONE, cmd, compute_scale_q16_16, crc16_ccitt, dir, flags, status,
};

/// `DDPI_CCW_AWAY` / `DDPI_CCW_TOWARD` (`ddpi_ccw_direction_t`, `.h:84-93`):
/// odd-sensor-ccw indices for -Y_HID and +Y_HID. Not part of the crate API.
const CCW_AWAY: usize = 2;
const CCW_TOWARD: usize = 6;

const E: usize = dir::E as usize;
const S: usize = dir::S as usize;
const W: usize = dir::W as usize;
const N: usize = dir::N as usize;
const NE: usize = dir::NE as usize;
const SE: usize = dir::SE as usize;
const SW: usize = dir::SW as usize;
const NW: usize = dir::NW as usize;

/// Writes the CRC-16 of bytes 0..62 little-endian into bytes 62..63.
fn sign(req: &mut [u8; HID_PACKET_LEN]) {
    let crc = crc16_ccitt(&req[..62]);
    req[62] = (crc & 0xFF) as u8;
    req[63] = (crc >> 8) as u8;
}

/* 1. Verify exact 1.00x neutral identity behavior across all quadrants */
#[test]
fn test_neutral_identity_passthrough() {
    let mut eng = Engine::new(800);

    let samples: [[i16; 2]; 9] = [
        [0, 0],
        [1, 0],
        [-1, 0],
        [0, 1],
        [0, -1],
        [15, -23],
        [-120, 85],
        [1024, -2048],
        [-32767, 32767],
    ];

    /* Test both disabled and enabled-with-all-1.00x */
    for mode in 0..2 {
        if mode == 1 {
            eng.profile.flags |= flags::ENABLED;
            eng.profile.sanitize_and_sign();
        }
        assert!(eng.profile.is_identity());

        for s in samples {
            let (ox, oy) = eng.process_delta(s[0], s[1]);
            assert_eq!(ox, s[0]);
            assert_eq!(oy, s[1]);
            assert_eq!(eng.rem_x_q16_16, 0);
            assert_eq!(eng.rem_y_q16_16, 0);
        }
    }
}

/* 2. Verify all 8 cardinal & diagonal control points at exact 45-degree angles */
#[test]
fn test_eight_octant_control_points() {
    let mut eng = Engine::new(1600);
    eng.profile.flags = flags::ENABLED;

    let mults: [u16; 8] = [256, 320, 384, 448, 512, 192, 128, 384];
    eng.profile.multipliers_q8_8 = mults;
    let profile = eng.profile;
    eng.set_profile(&profile);

    // (dx, dy, exp_dx, exp_dy)
    let cases: [(i16, i16, i16, i16); 8] = [
        (100, 0, 100, 0),       /* E:  1.00x */
        (100, 100, 125, 125),   /* SE: 1.25x */
        (0, 100, 0, 150),       /* S:  1.50x */
        (-100, 100, -175, 175), /* SW: 1.75x */
        (-100, 0, -200, 0),     /* W:  2.00x */
        (-100, -100, -75, -75), /* NW: 0.75x */
        (0, -100, 0, -50),      /* N:  0.50x */
        (100, -100, 150, -150), /* NE: 1.50x */
    ];

    for (dx, dy, exp_dx, exp_dy) in cases {
        eng.reset_remainders();
        let (ox, oy) = eng.process_delta(dx, dy);
        assert_eq!(ox, exp_dx);
        assert_eq!(oy, exp_dy);
    }
}

/* 3. Verify 360-degree circular interpolation continuity & monotonicity */
// The C test builds its input sweep with math.h cos/sin; libm gives the same
// inputs on every target (native and wasm).
#[test]
fn test_circular_360_interpolation_continuity() {
    let mut prof = Profile::new_default(800);
    prof.flags = flags::ENABLED;
    prof.multipliers_q8_8[E] = 256; /* 1.00x */
    prof.multipliers_q8_8[SE] = 320; /* 1.25x */
    prof.multipliers_q8_8[S] = 384; /* 1.50x */
    prof.multipliers_q8_8[SW] = 320; /* 1.25x */
    prof.multipliers_q8_8[W] = 256; /* 1.00x */
    prof.multipliers_q8_8[NW] = 224; /* 0.875x */
    prof.multipliers_q8_8[N] = 192; /* 0.75x */
    prof.multipliers_q8_8[NE] = 224; /* 0.875x */
    prof.sanitize_and_sign();

    let mut prev_scale: u32 = 0;
    let mut first_scale: u32 = 0;

    for deg in 0..=360 {
        // M_PI literal in C; rounds to the same f64 as PI.
        let rad = f64::from(deg) * (std::f64::consts::PI / 180.0);
        // llround: round half away from zero, like f64::round.
        let dx = (1000.0 * libm::cos(rad)).round() as i16;
        let dy = (1000.0 * libm::sin(rad)).round() as i16;
        let (sc, _oct) = compute_scale_q16_16(&prof, dx, dy);
        assert!(((192u32 << 8)..=(384u32 << 8)).contains(&sc));

        if deg == 0 {
            first_scale = sc;
        } else {
            let diff = (sc as i32 - prev_scale as i32).abs();
            assert!(diff < 1600);
        }
        prev_scale = sc;
    }
    assert_eq!(prev_scale, first_scale);
}

/* 4. Verify configurable global scalar gain */
#[test]
fn test_global_scalar_gain() {
    let mut eng = Engine::new(800);
    eng.profile.flags = flags::ENABLED;
    eng.profile.global_gain_q8_8 = 384; /* 1.50x global gain */
    eng.profile.multipliers_q8_8[E] = 320; /* 1.25x East -> total 1.875x */
    let profile = eng.profile;
    eng.set_profile(&profile);

    let (ox, oy) = eng.process_delta(80, 0);
    assert_eq!(ox, 150);
    assert_eq!(oy, 0);
}

/* 5. Verify fractional sub-pixel remainder accumulation across slow movements */
#[test]
fn test_fractional_remainder_accumulation() {
    let mut eng = Engine::new(800);
    eng.profile.flags = flags::ENABLED;
    eng.profile.multipliers_q8_8[E] = 192; /* 0.75x */
    let profile = eng.profile;
    eng.set_profile(&profile);

    let mut sum_x: i32 = 0;
    let mut sum_y: i32 = 0;
    for _ in 0..400 {
        let (ox, oy) = eng.process_delta(1, 0);
        sum_x += i32::from(ox);
        sum_y += i32::from(oy);
    }
    assert_eq!(sum_x, 300);
    assert_eq!(sum_y, 0);
    assert_eq!(eng.rem_x_q16_16, 0);

    eng.profile.multipliers_q8_8[W] = 160; /* 0.625x */
    let profile = eng.profile;
    eng.set_profile(&profile);
    sum_x = 0;
    for _ in 0..80 {
        let (ox, _oy) = eng.process_delta(-1, 0);
        sum_x += i32::from(ox);
    }
    assert_eq!(sum_x, -50);
    assert_eq!(eng.rem_x_q16_16, 0);
}

/* 6. Verify saturation clamping and excess remainder carry */
#[test]
fn test_saturation_and_overflow() {
    let mut eng = Engine::new(1600);
    eng.profile.flags = flags::ENABLED;
    eng.profile.global_gain_q8_8 = 512; /* 2.00x */
    eng.profile.multipliers_q8_8 = [256; 8]; /* 1.00x -> combined 2.0x */
    let profile = eng.profile;
    eng.set_profile(&profile);

    /* 20000 * 2.0 = 40000 -> clamps to 32767 and carries 7233 in remainder */
    let (ox, oy) = eng.process_delta(20000, -20000);
    assert_eq!(ox, 32767);
    assert_eq!(oy, -32767);
    assert_eq!(eng.rem_x_q16_16, 7233 * 65536);
    assert_eq!(eng.rem_y_q16_16, -(7233 * 65536));

    /* Next report with (+1, -1) emits (7233 + 2 = 7235, -7235) */
    let (ox, oy) = eng.process_delta(1, -1);
    assert_eq!(ox, 7235);
    assert_eq!(oy, -7235);
    assert_eq!(eng.rem_x_q16_16, 0);
    assert_eq!(eng.rem_y_q16_16, 0);
}

/* 7. Verify profile binary serialization, CRC-16-CCITT, and corruption rejection */
#[test]
fn test_profile_serialization_and_crc() {
    let mut p_in = Profile::new_default(1200);
    p_in.flags = flags::ENABLED;
    p_in.global_gain_q8_8 = 280;
    for (i, m) in p_in.multipliers_q8_8.iter_mut().enumerate() {
        *m = (200 + i * 20) as u16;
    }

    let mut buf = p_in.serialize();

    let p_out = Profile::deserialize(&buf).expect("deserialize");
    assert_eq!(p_out.magic, PROFILE_MAGIC);
    assert_eq!(p_out.version, PROFILE_VERSION);
    assert_eq!(p_out.flags, flags::ENABLED);
    assert_eq!(p_out.base_dpi, 1200);
    assert_eq!(p_out.global_gain_q8_8, 280);
    for (i, &m) in p_out.multipliers_q8_8.iter().enumerate() {
        assert_eq!(m, (200 + i * 20) as u16);
    }

    buf[10] ^= 0x01;
    assert!(Profile::deserialize(&buf).is_none());
}

/* 8. Verify 64-byte HID Feature report protocol handler */
#[test]
fn test_hid_protocol_handler() {
    let mut eng = Engine::new(800);

    let mut req = [0u8; HID_PACKET_LEN];
    let mut resp = [0u8; HID_PACKET_LEN];

    req[0] = 0x44; /* 'D' */
    req[1] = 0x50; /* 'P' */
    req[2] = PROFILE_VERSION;
    req[4] = 0x2A; /* seq */
    req[5] = HID_NAMESPACE;
    req[6] = cmd::GET_CAPABILITIES;
    sign(&mut req);

    assert!(eng.handle_feature_report(&req, &mut resp, None, None));
    assert_eq!(resp[3], status::OK);
    assert_eq!(resp[4], 0x2A);
    assert_eq!(&resp[8..12], b"DP01");
    assert_eq!(
        crc16_ccitt(&resp[..62]),
        u16::from(resp[62]) | (u16::from(resp[63]) << 8)
    );
    assert_eq!(resp[15] & 0x80, 0); /* no clock reported yet */

    /* The adapter clock appears at payload 20..23 with flag bit 7, and survives RESET_DEFAULTS */
    eng.set_clock_ms(0x1234_5678);
    req[6] = cmd::RESET_DEFAULTS;
    sign(&mut req);
    assert!(eng.handle_feature_report(&req, &mut resp, None, None));
    req[6] = cmd::GET_CAPABILITIES;
    sign(&mut req);
    assert!(eng.handle_feature_report(&req, &mut resp, None, None));
    assert_ne!(resp[15] & 0x80, 0);
    assert!(resp[28] == 0x78 && resp[29] == 0x56 && resp[30] == 0x34 && resp[31] == 0x12);
}

/* 9. Verify Travel-Window Direction Estimator & Polling-Rate Chunking Invariance */
#[test]
fn test_travel_window_chunking_invariance() {
    let mut eng_1khz = Engine::new(800);

    /* Profile where East = 1.00x (256), North = 1.00x (256), but NE (45 deg) = 0.50x (128).
     * Without travel-window smoothing, alternating (1,0) and (0,-1) reports at 8 kHz
     * would snap to E (1.00x) and N (1.00x), completely missing the NE (0.50x) diagonal!
     */
    eng_1khz.profile.flags = flags::ENABLED;
    eng_1khz.profile.multipliers_q8_8[E] = 256; /* 1.00x */
    eng_1khz.profile.multipliers_q8_8[N] = 256; /* 1.00x */
    eng_1khz.profile.multipliers_q8_8[NE] = 128; /* 0.50x */
    eng_1khz.profile.set_window_counts(16);
    let profile = eng_1khz.profile;
    eng_1khz.set_profile(&profile);

    let mut eng_8khz = Engine::new(800);
    eng_8khz.set_profile(&eng_1khz.profile);

    /* 1 kHz: 40 diagonal steps of (2, -2) = (80, -80) raw -> (40, -40) scaled */
    let (mut sum_1k_x, mut sum_1k_y) = (0i32, 0i32);
    for _ in 0..40 {
        let (ox, oy) = eng_1khz.process_delta(2, -2);
        sum_1k_x += i32::from(ox);
        sum_1k_y += i32::from(oy);
    }
    assert!(sum_1k_x == 40 && sum_1k_y == -40);

    /* 8 kHz: 80 alternating steps of (1, 0) and (0, -1) = (80, -80) raw.
     * With L=16 travel window, the direction estimator tracks ~45 deg (NE) so
     * both X and Y are scaled near 0.50x (within 3 counts of 40) instead of 80!
     */
    let (mut sum_8k_x, mut sum_8k_y) = (0i32, 0i32);
    for _ in 0..80 {
        let (ox, oy) = eng_8khz.process_delta(1, 0);
        sum_8k_x += i32::from(ox);
        sum_8k_y += i32::from(oy);
        let (ox, oy) = eng_8khz.process_delta(0, -1);
        sum_8k_x += i32::from(ox);
        sum_8k_y += i32::from(oy);
    }
    assert!((sum_8k_x - sum_1k_x).abs() <= 3);
    assert!((sum_8k_y - sum_1k_y).abs() <= 3);
}

/* 10. Verify Counter-Clockwise Frame (odd-sensor-ccw) conversion & indexing */
#[test]
fn test_ccw_frame_conversion() {
    let mut eng = Engine::new(800);
    eng.profile.flags = flags::ENABLED | flags::FRAME_SENSOR_CCW;
    /* In odd-sensor-ccw: index 2 = Away (North, -Y_HID), index 6 = Toward (South, +Y_HID) */
    eng.profile.multipliers_q8_8[CCW_AWAY] = 384; /* 1.50x Away (-Y_HID) */
    eng.profile.multipliers_q8_8[CCW_TOWARD] = 128; /* 0.50x Toward (+Y_HID) */
    let profile = eng.profile;
    eng.set_profile(&profile);

    let (ox, oy) = eng.process_delta(0, -100); /* Move Away (-Y_HID) */
    assert!(ox == 0 && oy == -150);

    eng.reset_remainders();
    let (ox, oy) = eng.process_delta(0, 100); /* Move Toward (+Y_HID) */
    assert!(ox == 0 && oy == 50);

    /* Convert in-place back to CW frame and verify behavior is identical */
    eng.profile.set_ccw_frame(false);
    assert_eq!(eng.profile.flags & flags::FRAME_SENSOR_CCW, 0);
    assert_eq!(eng.profile.multipliers_q8_8[N], 384);
    assert_eq!(eng.profile.multipliers_q8_8[S], 128);
}

/* 11. Verify CPI Headroom Normalization (max internal firmware gain <= 1.00x) */
#[test]
fn test_cpi_headroom_normalization() {
    let mut prof = Profile::new_default(800);
    prof.flags = flags::ENABLED;
    /* Gains 0.90x .. 1.10x at 800 CPI -> sensor CPI = 880, max internal gain = 1.00x (256) */
    prof.multipliers_q8_8[E] = 282; /* ~1.1015x */
    prof.multipliers_q8_8[S] = 230; /* ~0.8984x */

    let sensor_cpi = prof.normalize_cpi_headroom();
    assert!((880..=882).contains(&sensor_cpi));
    for &m in &prof.multipliers_q8_8 {
        assert!(m <= Q8_8_ONE);
    }
    assert_eq!(prof.multipliers_q8_8[E], Q8_8_ONE);
}

/* 12. Verify Linear 2x2 Matrix Model and Calibration Bypass Mode */
#[test]
fn test_linear_2x2_and_calibration_bypass() {
    let mut eng = Engine::new(800);
    eng.profile.flags = flags::ENABLED | flags::MODEL_LINEAR_2X2;
    eng.profile.multipliers_q8_8[E] = 256; /* X gain = 1.00x */
    eng.profile.multipliers_q8_8[N] = 192; /* Y gain = 0.75x */
    let profile = eng.profile;
    eng.set_profile(&profile);

    let (ox, oy) = eng.process_delta(100, -100);
    assert!(ox == 100 && oy == -75);

    /* Enable calibration bypass flag -> must become exact 1.00x identity */
    eng.profile.flags |= flags::BYPASS_FOR_CALIBRATION;
    let profile = eng.profile;
    eng.set_profile(&profile);
    let (ox, oy) = eng.process_delta(100, -100);
    assert!(ox == 100 && oy == -100);
}

/* 13. Regression: first sample after idle with |d| >= 128 must not wrap the int16 estimate */
#[test]
fn test_large_first_sample_after_idle() {
    let firsts: [i16; 6] = [128, 200, 1000, 32767, -128, -32768];
    for first in firsts {
        let mut eng = Engine::new(800);
        eng.profile.flags = flags::ENABLED;
        eng.profile.multipliers_q8_8[W] = 512; /* Left 2.00x, Right 1.00x */
        eng.profile.set_window_counts(16);
        let profile = eng.profile;
        eng.set_profile(&profile);

        let (ox, oy) = eng.process_delta(first, 0);
        let mut expect: i32 = if first > 0 {
            i32::from(first)
        } else {
            2 * i32::from(first)
        };
        if expect < -32767 {
            expect = -32767;
        }
        assert_eq!(i32::from(ox), expect);
        assert_eq!(oy, 0);
        assert_eq!(eng.last_octant, if first > 0 { 0 } else { 3 });
    }
}

/* 14. ddpi_process_motion: idle gap restarts the direction estimator (default and custom timeout) */
#[test]
fn test_process_motion_idle_reset() {
    for gap_case in 0..3 {
        let mut eng = Engine::new(800);
        eng.profile.flags = flags::ENABLED;
        eng.profile.multipliers_q8_8[N] = 128; /* Up 0.50x, Right 1.00x */
        eng.profile.set_window_counts(16);
        if gap_case == 2 {
            eng.profile.reserved |= 40 << 8; /* 200 ms idle */
        }
        let profile = eng.profile;
        eng.set_profile(&profile);

        let mut t: u32 = 1000;
        for _ in 0..20 {
            eng.process_motion(20, 0, t); /* Sweep right */
            t += 1;
        }
        let gap: u32 = if gap_case == 0 { 5 } else { 100 };
        let (_ox, oy) = eng.process_motion(0, -4, t + gap); /* Then move up */
        if gap_case == 1 {
            assert_eq!(oy, -2); /* 100 ms >= 50 ms default: fresh estimate, pure N = 0.50x */
        } else {
            assert_ne!(oy, -2); /* 5 ms gap, or 100 ms < 200 ms custom: still inherits the sweep */
        }
    }
}

/* 15. Commands the device cannot perform answer UNSUPPORTED instead of a false OK */
#[test]
fn test_hid_null_callbacks_unsupported() {
    let mut eng = Engine::new(800);
    let opcodes = [cmd::SAVE_FLASH, cmd::ENTER_BOOTLOADER];
    for opcode in opcodes {
        let mut req = [0u8; HID_PACKET_LEN];
        let mut resp = [0u8; HID_PACKET_LEN];
        req[0] = 0x44;
        req[1] = 0x50;
        req[2] = PROFILE_VERSION;
        req[5] = HID_NAMESPACE;
        req[6] = opcode;
        req[8] = 0x53; /* "BLRS" little-endian */
        req[9] = 0x52;
        req[10] = 0x4C;
        req[11] = 0x42;
        sign(&mut req);
        assert!(eng.handle_feature_report(&req, &mut resp, None, None));
        assert_eq!(resp[3], status::ERR_UNSUPPORTED);
    }
}

/* 16. High-CPI sensors (45K class) round-trip, and headroom math does not overflow */
#[test]
fn test_high_cpi_range() {
    let mut p = Profile::new_default(45000);
    p.flags = flags::ENABLED;
    let buf = p.serialize();
    let out = Profile::deserialize(&buf).expect("deserialize");
    assert_eq!(out.base_dpi, 45000);

    p.multipliers_q8_8[E] = 307; /* ~1.2x -> 54000 */
    let cpi = p.normalize_cpi_headroom();
    assert!((53900..=54100).contains(&cpi));

    p = Profile::new_default(60000);
    p.flags = flags::ENABLED;
    p.global_gain_q8_8 = 1024; /* 4x * 4x = 16x would overflow uint32 math */
    p.multipliers_q8_8[E] = 1024;
    let cpi = p.normalize_cpi_headroom();
    assert_eq!(cpi, DPI_MAX);
}

#[test]
fn test_single_dpi_stage() {
    let mut e = Engine::new(800);
    let mut p = e.profile;
    p.flags = flags::ENABLED | flags::ONLY_AT_BASE_DPI;
    p.multipliers_q8_8[N] = 128; /* up (-Y) at 0.50x */
    e.set_profile(&p);

    /* Sensor DPI unknown: the flag has no effect, the curve applies */
    let (ox, oy) = e.process_delta(0, -100);
    assert!(ox == 0 && oy == -50);

    /* Bound stage: curve applies */
    e.set_sensor_dpi(800);
    let (_ox, oy) = e.process_delta(0, -100);
    assert_eq!(oy, -50);
    let (_ox, oy) = e.process_delta(0, -1); /* leaves a -0.5 remainder */
    assert!(oy == 0 && e.rem_y_q16_16 != 0);

    /* Another stage: exact passthrough, and the stage change dropped the remainder */
    e.set_sensor_dpi(1600);
    assert_eq!(e.rem_y_q16_16, 0);
    let (ox, oy) = e.process_delta(0, -100);
    assert!(ox == 0 && oy == -100);
    let (ox, oy) = e.process_delta(37, -1);
    assert!(ox == 37 && oy == -1);

    /* Back to the bound stage */
    e.set_sensor_dpi(800);
    let (_ox, oy) = e.process_delta(0, -100);
    assert_eq!(oy, -50);

    /* Without the flag every stage gets the curve */
    p.flags = flags::ENABLED;
    e.set_profile(&p);
    e.set_sensor_dpi(1600);
    let (_ox, oy) = e.process_delta(0, -100);
    assert_eq!(oy, -50);

    /* Capabilities report the sensor DPI (bytes 18..19) and bit 6; RESET_DEFAULTS keeps it */
    let mut req = [0u8; HID_PACKET_LEN];
    req[..7].copy_from_slice(&[
        0x44,
        0x50,
        PROFILE_VERSION,
        0,
        7,
        HID_NAMESPACE,
        cmd::GET_CAPABILITIES,
    ]);
    let mut resp = [0u8; HID_PACKET_LEN];
    sign(&mut req);
    assert!(e.handle_feature_report(&req, &mut resp, None, None));
    assert!(resp[3] == status::OK && (resp[8 + 7] & 0x40) != 0);
    assert_eq!(
        u16::from(resp[8 + 18]) | (u16::from(resp[8 + 19]) << 8),
        1600
    );
    req[6] = cmd::RESET_DEFAULTS;
    sign(&mut req);
    assert!(e.handle_feature_report(&req, &mut resp, None, None));
    assert_eq!(e.sensor_dpi, 1600);
}

/* Extra: CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) standard check value */
#[test]
fn test_crc16_ccitt_check_value() {
    assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
}
