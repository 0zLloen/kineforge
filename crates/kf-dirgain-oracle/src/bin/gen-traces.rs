//! Records stateful traces of the C reference engine for
//! `crates/kf-dirgain/tests/traces.rs`: hand-written scenarios covering
//! OPEN_DIRECTIONAL_DPI_AUDIT.md §8 plus fixed-seed random ones. The JSON
//! format is described in README.md.
//!
//! Usage: `cargo run -p kf-dirgain-oracle --bin gen-traces -- <output.json>`

use std::collections::BTreeMap;
use std::fmt::Write as _;

use kf_dirgain::{BOOTLOADER_MAGIC_ARG, Engine, PROFILE_MAGIC, PROFILE_VERSION, Profile, cmd};
use kf_dirgain_oracle::{
    COracle, Op, Outcome, XorShift, c, hex, hid_request, profile_to_raw, sign_request, signed_raw,
};

use kf_dirgain::flags::{
    BYPASS_FOR_CALIBRATION as BYPASS, ENABLED, FRAME_SENSOR_CCW as CCW,
    KEEP_REMAINDER_ON_REVERSE as KEEP, LINEAR_OCTANT_RATIO as LINEAR, MODEL_LINEAR_2X2 as M2X2,
    ONLY_AT_BASE_DPI as ONLY, RESET_REMAINDER_ON_REVERSE as RESET,
};

/// Eight distinct multipliers so an index mix-up changes results (as in the
/// C repo's `tests/gen_vectors.c`).
const ASYM: [u16; 8] = [256, 300, 384, 340, 230, 200, 170, 210];
/// 2x2 model gains E=300, NW=150, N=200, NE=400 (HID clockwise indices 0, 5, 6, 7).
const SHEAR: [u16; 8] = [300, 256, 256, 256, 256, 150, 200, 400];

fn profile(flags: u8, base_dpi: u16, gain: u16, multipliers: [u16; 8], reserved: u16) -> Profile {
    Profile {
        magic: PROFILE_MAGIC,
        version: PROFILE_VERSION,
        flags,
        base_dpi,
        global_gain_q8_8: gain,
        multipliers_q8_8: multipliers,
        reserved,
        crc16: 0,
    }
}

fn state_json(e: &Engine) -> String {
    format!(
        "\"rem_x\":{},\"rem_y\":{},\"dir_x\":{},\"dir_y\":{},\"last_octant\":{},\
         \"last_scale\":{},\"last_motion_ms\":{},\"sensor_dpi\":{},\"clock_known\":{},\
         \"clock_ms\":{},\"profile_hex\":\"{}\"",
        e.rem_x_q16_16,
        e.rem_y_q16_16,
        e.dir_x_q16_16,
        e.dir_y_q16_16,
        e.last_octant,
        e.last_scale_q16_16,
        e.last_motion_ms,
        e.sensor_dpi,
        e.clock_known,
        e.clock_ms,
        hex(&profile_to_raw(&e.profile)),
    )
}

fn op_json(op: &Op) -> String {
    match *op {
        Op::Delta(dx, dy) => format!("\"op\":\"delta\",\"dx\":{dx},\"dy\":{dy}"),
        Op::Motion(dx, dy, now) => {
            format!("\"op\":\"motion\",\"dx\":{dx},\"dy\":{dy},\"now\":{now}")
        }
        Op::SensorDpi(dpi) => format!("\"op\":\"sensor_dpi\",\"dpi\":{dpi}"),
        Op::Clock(now) => format!("\"op\":\"clock\",\"now\":{now}"),
        Op::SetProfile(p) => format!(
            "\"op\":\"set_profile\",\"profile_hex\":\"{}\"",
            hex(&profile_to_raw(&p))
        ),
        Op::NotifyIdle => "\"op\":\"notify_idle\"".into(),
        Op::ResetRemainders => "\"op\":\"reset_remainders\"".into(),
        Op::Normalize => "\"op\":\"normalize\"".into(),
        Op::Ccw(use_ccw) => format!("\"op\":\"ccw\",\"use_ccw\":{use_ccw}"),
        Op::Window(w) => format!("\"op\":\"window\",\"counts\":{w}"),
        Op::Hid { req, save, boot } => format!(
            "\"op\":\"hid\",\"request\":\"{}\",\"save\":{},\"boot\":{boot}",
            hex(&req),
            save.map_or("null".into(), |s| s.to_string())
        ),
    }
}

fn outcome_json(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Nothing => String::new(),
        Outcome::Delta(x, y) => format!("\"out_dx\":{x},\"out_dy\":{y},"),
        Outcome::SensorCpi(cpi) => format!("\"sensor_cpi\":{cpi},"),
        Outcome::Hid {
            handled,
            resp,
            calls,
        } => format!(
            "\"handled\":{handled},\"response\":\"{}\",\"save_calls\":{},\
             \"saved_profile_hex\":{},\"boot_calls\":{},",
            hex(resp),
            calls.save_calls,
            calls.saved_profile.map_or("null".into(), |p| format!(
                "\"{}\"",
                hex(&profile_to_raw(&p))
            )),
            calls.boot_calls
        ),
    }
}

/// One engine run on the C oracle, recorded step by step.
struct Trace {
    name: String,
    base_dpi: u16,
    profile: Option<Profile>,
    init: String,
    c: COracle,
    steps: Vec<String>,
    /// Clock for [`Trace::tick`].
    now: u32,
    /// Sequence byte for [`Trace::hid`].
    seq: u8,
}

impl Trace {
    /// `Engine::new(base_dpi)`, then `set_profile(profile)` if given.
    fn new(name: &str, base_dpi: u16, profile: Option<Profile>) -> Trace {
        let mut c = COracle::new(base_dpi);
        if let Some(p) = &profile {
            c.set_profile(p);
        }
        Trace {
            name: name.into(),
            base_dpi,
            profile,
            init: state_json(&c.engine()),
            c,
            steps: Vec::new(),
            now: 1000,
            seq: 0,
        }
    }

    fn with(name: &str, p: Profile) -> Trace {
        Trace::new(name, p.base_dpi, Some(p))
    }

    fn op(&mut self, op: Op) -> &mut Self {
        let outcome = op.run_c(&mut self.c);
        self.steps.push(format!(
            "{{{},\"expect\":{{{}{}}}}}",
            op_json(&op),
            outcome_json(&outcome),
            state_json(&self.c.engine())
        ));
        self
    }

    /// `process_motion` `dt` ms after the previous tick.
    fn tick(&mut self, dx: i16, dy: i16, dt: u32) -> &mut Self {
        self.now = self.now.wrapping_add(dt);
        self.op(Op::Motion(dx, dy, self.now))
    }

    /// One tick per move, 1 ms apart.
    fn ticks(&mut self, moves: &[(i16, i16)]) -> &mut Self {
        for &(dx, dy) in moves {
            self.tick(dx, dy, 1);
        }
        self
    }

    fn delta(&mut self, dx: i16, dy: i16) -> &mut Self {
        self.op(Op::Delta(dx, dy))
    }

    fn hid_req(&mut self, req: [u8; 64], save: Option<bool>, boot: bool) -> &mut Self {
        self.op(Op::Hid { req, save, boot })
    }

    /// Well-formed request, slot 0, a fresh sequence number.
    fn hid(&mut self, opcode: u8, payload: &[u8], save: Option<bool>, boot: bool) -> &mut Self {
        self.seq = self.seq.wrapping_add(1);
        self.hid_req(hid_request(self.seq, opcode, 0, payload), save, boot)
    }

    fn json(&self) -> String {
        let profile = self.profile.map_or("null".into(), |p| {
            format!("\"{}\"", hex(&profile_to_raw(&p)))
        });
        format!(
            "    {{\"name\":\"{}\",\"base_dpi\":{},\"profile_hex\":{},\"init\":{{{}}},\"steps\":[\n      {}\n    ]}}",
            self.name,
            self.base_dpi,
            profile,
            self.init,
            self.steps.join(",\n      ")
        )
    }
}

const FLAG_NAMES: [&str; 8] = [
    "enabled",
    "keep",
    "linear",
    "ccw",
    "2x2",
    "reset",
    "bypass",
    "only-base",
];

fn flag_name(f: u8) -> String {
    let names: Vec<&str> = (0..8)
        .filter(|b| f & (1 << b) != 0)
        .map(|b| FLAG_NAMES[b])
        .collect();
    names.join("+")
}

/// Every direction, reversals, octant edges, a zero report, int16 extremes.
const SCRIPT: [(i16, i16); 24] = [
    (1, 0),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
    (3, -2),
    (-7, 5),
    (5, 5),
    (-5, -5),
    (100, -250),
    (-250, 100),
    (0, 0),
    (2, 1),
    (1, 2),
    (-32768, 1),
    (1, -32768),
    (32767, 32767),
    (12, -3),
    (-3, 12),
    (40, 41),
];

/// [`SCRIPT`], a different DPI stage and back, an idle gap, plain deltas.
fn script(t: &mut Trace, base_dpi: u16) {
    t.ticks(&SCRIPT);
    t.op(Op::SensorDpi(base_dpi.wrapping_add(800)));
    t.ticks(&[(9, -4), (-32768, -32768)]);
    t.op(Op::SensorDpi(base_dpi));
    t.ticks(&[(9, -4), (4, 9)]);
    t.tick(6, 6, 100);
    for _ in 0..3 {
        t.delta(-2, 1);
    }
}

fn window_traces(out: &mut Vec<Trace>) {
    // 129..255 are stored as is and clamped to 128 by set_profile.
    for l in [0, 1, 16, 31, 32, 64, 128, 129, 200, 255] {
        let mut t = Trace::with(&format!("window/L{l}"), profile(ENABLED, 800, 256, ASYM, l));
        for _ in 0..12 {
            t.tick(1, 0, 1);
        }
        // 90-degree turn in 1-count steps, then a 1-count staircase.
        for _ in 0..12 {
            t.tick(0, 1, 1);
        }
        for i in 0..12 {
            if i % 2 == 0 {
                t.tick(1, 0, 1)
            } else {
                t.tick(0, -1, 1)
            };
        }
        // 90-degree turn in larger steps, then steps past the |d| <= 16383 clamp.
        for _ in 0..6 {
            t.tick(37, -12, 1);
        }
        for _ in 0..6 {
            t.tick(12, 37, 1);
        }
        for _ in 0..3 {
            t.tick(-300, 2, 1);
        }
        for _ in 0..3 {
            t.tick(0, -3000, 1);
        }
        t.ticks(&[
            (32767, -32768),
            (-32768, 32767),
            (-1, -1),
            (127, 0),
            (128, 0),
            (-129, 1),
        ]);
        for _ in 0..3 {
            t.delta(5, 5);
        }
        out.push(t);
    }
}

fn flag_traces(out: &mut Vec<Trace>) {
    let mut sets: Vec<u8> = (0..8).map(|b| 1 << b).collect();
    sets.extend((1..8).map(|b| ENABLED | (1 << b)));
    sets.extend([
        ENABLED | LINEAR | CCW,
        ENABLED | M2X2 | CCW,
        ENABLED | M2X2 | RESET,
        ENABLED | RESET | KEEP,
        ENABLED | BYPASS | M2X2,
        ENABLED | ONLY | M2X2 | RESET,
        ENABLED | LINEAR | RESET | CCW | ONLY,
        0xFF,
        !BYPASS,
        !ENABLED,
    ]);
    for f in sets {
        let name = format!("flags/{f:#04x}-{}", flag_name(f));
        let mut t = Trace::with(&name, profile(f, 800, 300, ASYM, 16));
        script(&mut t, 800);
        out.push(t);
    }
}

fn model_2x2_traces(out: &mut Vec<Trace>) {
    let cases: [(&str, u8, u16, [u16; 8]); 5] = [
        ("2x2/shear", ENABLED | M2X2, 300, SHEAR),
        ("2x2/shear-ccw", ENABLED | M2X2 | CCW, 300, SHEAR),
        // NW > NE: negative shear.
        (
            "2x2/shear-negative",
            ENABLED | M2X2,
            256,
            [256, 256, 256, 256, 256, 400, 256, 150],
        ),
        // (NE - NW) * G = -17: C division truncates toward zero.
        (
            "2x2/odd-shear",
            ENABLED | M2X2,
            17,
            [17, 256, 256, 256, 256, 17, 16, 16],
        ),
        (
            "2x2/extreme-reset",
            ENABLED | M2X2 | RESET,
            1024,
            [1024, 256, 256, 256, 256, 16, 16, 1024],
        ),
    ];
    for (name, f, gain, m) in cases {
        let mut t = Trace::with(name, profile(f, 800, gain, m, 16));
        script(&mut t, 800);
        t.ticks(&[(32767, 32767), (-32768, 32767), (0, 5), (5, 0), (0, -32768)]);
        out.push(t);
    }

    // dx·gx == dy·shear: X's scaled delta is exactly 0 while the raw dx is
    // not, so only the remainder (and the reverse reset) decides the output.
    // Each trace runs the cancelling step after both a same-sign and an
    // opposite-sign remainder.
    const M16X: [u16; 8] = [1024, 1024, 1024, 1024, 1024, 16, 1024, 1024];
    let cancel = [
        ("cancel", 300, SHEAR, [(-1, 0), (5, 12), (-1, 0), (5, 12)]),
        (
            "cancel-16x",
            1024,
            M16X,
            [(32767, 0), (63, 128), (-32768, 0), (63, 128)],
        ),
    ];
    for (name, gain, m, steps) in cancel {
        // gx = M[E]·G, shear = (M[NE]·G - M[NW]·G) / 2, as in process_delta.
        let g = i64::from(gain);
        let (gx, shear) = (
            i64::from(m[0]) * g,
            (i64::from(m[7]) - i64::from(m[5])) * g / 2,
        );
        for &(dx, dy) in steps.iter().skip(1).step_by(2) {
            assert_eq!(
                i64::from(dx) * gx,
                i64::from(dy) * shear,
                "{name}: X not cancelled"
            );
        }
        for (suffix, f) in [("", ENABLED | M2X2), ("-reset", ENABLED | M2X2 | RESET)] {
            let mut t = Trace::with(&format!("2x2/{name}{suffix}"), profile(f, 800, gain, m, 0));
            for (dx, dy) in steps {
                t.delta(dx, dy);
            }
            out.push(t);
        }
    }
}

fn reverse_traces(out: &mut Vec<Trace>) {
    // (0, 5) under 2x2 shear: a non-zero scaled X with a zero raw X.
    const REVERSALS: [(i16, i16); 16] = [
        (1, 1),
        (1, 1),
        (-1, -1),
        (1, 0),
        (-1, 0),
        (-1, 0),
        (1, 0),
        (0, 1),
        (0, -1),
        (3, -3),
        (-3, 3),
        (2, 0),
        (0, 0),
        (-2, 0),
        (0, 5),
        (0, -5),
    ];
    let cases: [(&str, u8, u16, [u16; 8], u16); 5] = [
        ("reverse/keep", ENABLED, 384, [256; 8], 0),
        ("reverse/keep-legacy-flag", ENABLED | KEEP, 384, [256; 8], 0),
        ("reverse/reset", ENABLED | RESET, 384, [256; 8], 0),
        ("reverse/reset-window", ENABLED | RESET, 451, ASYM, 16),
        ("reverse/reset-2x2", ENABLED | M2X2 | RESET, 300, SHEAR, 0),
    ];
    for (name, f, gain, m, reserved) in cases {
        let mut t = Trace::with(name, profile(f, 800, gain, m, reserved));
        t.ticks(&REVERSALS);
        t.ticks(&REVERSALS);
        out.push(t);
    }
}

fn carry_traces(out: &mut Vec<Trace>) {
    // 2x of 32767 leaves exactly the 32767-count remainder cap; 2x of -32768
    // and anything at 4x/16x exceed it, so the excess is dropped.
    const CARRY: [(i16, i16); 10] = [
        (32767, -32768),
        (32767, -32768),
        (1, -1),
        (1, -1),
        (1, -1),
        (-1, 1),
        (0, 0),
        (-32768, 32767),
        (-1, -1),
        (1, 1),
    ];
    let cases: [(&str, u8, u16, [u16; 8], u16); 8] = [
        ("carry/2x", ENABLED, 256, [512; 8], 0),
        ("carry/4x", ENABLED, 256, [1024; 8], 0),
        ("carry/16x", ENABLED, 1024, [1024; 8], 0),
        ("carry/16x-reset", ENABLED | RESET, 1024, [1024; 8], 0),
        ("carry/16x-window", ENABLED, 1024, [1024; 8], 16),
        (
            "carry/2x2-16x",
            ENABLED | M2X2,
            1024,
            [1024, 1024, 1024, 1024, 1024, 16, 1024, 1024],
            0,
        ),
        ("carry/tiny", ENABLED, 16, [16; 8], 0),
        ("carry/tiny-window", ENABLED, 16, [16; 8], 32),
    ];
    for (name, f, gain, m, reserved) in cases {
        let mut t = Trace::with(name, profile(f, 800, gain, m, reserved));
        for (dx, dy) in CARRY {
            t.delta(dx, dy);
        }
        for _ in 0..4 {
            t.delta(-32768, 32767);
        }
        t.ticks(&CARRY);
        out.push(t);
    }

    // Curve off: -32768 passes through as -32767, state is kept.
    let off: [(&str, u8, u16, [u16; 8]); 4] = [
        ("carry/off-disabled", 0, 300, ASYM),
        ("carry/off-bypass", ENABLED | BYPASS, 300, ASYM),
        ("carry/off-identity", ENABLED, 256, [256; 8]),
        ("carry/off-other-stage", ENABLED | ONLY, 300, ASYM),
    ];
    for (name, f, gain, m) in off {
        let mut t = Trace::with(name, profile(f, 800, gain, m, 16));
        t.op(Op::SensorDpi(1600));
        for (dx, dy) in [
            (-32768, -32768),
            (32767, -32767),
            (-32767, -32768),
            (0, 0),
            (-32768, 0),
            (0, -32768),
        ] {
            t.delta(dx, dy);
        }
        t.ticks(&[(-32768, 5), (5, -32768)]);
        out.push(t);
    }
}

fn idle_traces(out: &mut Vec<Trace>) {
    for idle_byte in [0u8, 1, 2, 10, 255] {
        let idle = if idle_byte == 0 {
            50
        } else {
            5 * u32::from(idle_byte)
        };
        let reserved = (u16::from(idle_byte) << 8) | 16;
        let mut t = Trace::with(
            &format!("idle/{idle}ms-byte{idle_byte}"),
            profile(ENABLED, 800, 256, ASYM, reserved),
        );
        // First motion at t = 0 (last_motion_ms starts at 0).
        t.now = 0;
        t.tick(3, 0, 0);
        t.now = 1000;
        t.tick(10, 0, 0).tick(10, 0, 1);
        t.tick(0, 10, idle - 1); // gap idle - 1: estimate kept
        t.tick(0, 10, idle); // gap == idle: reset
        t.tick(10, 0, idle + 1); // gap idle + 1: reset
        // A zero report does not count as motion.
        t.op(Op::Motion(0, 0, t.now.wrapping_add(10 * idle)));
        t.tick(-10, 0, idle - 1);
        t.op(Op::NotifyIdle);
        t.tick(-10, 0, 1);
        // The same thresholds, each gap g crossing the u32 clock wrap: a
        // motion at u32::MAX - g/2, then one g ms later (past 0).
        for g in [idle - 1, idle, idle + 1] {
            t.now = u32::MAX - g / 2;
            t.tick(10, 0, 0).tick(0, 10, g);
        }
        // One ms backwards is a gap of u32::MAX ms.
        t.tick(5, 5, u32::MAX);
        t.tick(5, 5, 1);
        out.push(t);
    }
}

fn dpi_traces(out: &mut Vec<Trace>) {
    let cases: [(&str, u8, u16); 4] = [
        ("dpi/only-at-base", ENABLED | ONLY, 800),
        ("dpi/without-flag", ENABLED, 800),
        ("dpi/only-at-base-65535", ENABLED | ONLY, 65535),
        ("dpi/only-at-base-2x2", ENABLED | ONLY | M2X2, 1600),
    ];
    for (name, f, base) in cases {
        let other = if base == 65535 { 50 } else { base * 2 };
        let mut t = Trace::with(name, profile(f, base, 300, ASYM, 16));
        t.ticks(&[(5, 3), (5, 3)]); // sensor DPI unknown
        t.op(Op::SensorDpi(base));
        t.ticks(&[(5, 3), (5, 3)]);
        t.op(Op::SensorDpi(base)); // unchanged: no reset
        t.tick(5, 3, 1);
        t.op(Op::SensorDpi(other));
        t.ticks(&[(5, 3), (-32768, 7), (7, 7)]);
        t.op(Op::SensorDpi(base));
        t.ticks(&[(5, 3), (5, 3)]);
        t.op(Op::SensorDpi(0));
        t.tick(5, 3, 1);
        t.op(Op::SensorDpi(other)).op(Op::SensorDpi(0));
        t.tick(5, 3, 1);
        out.push(t);
    }
}

fn set_profile_traces(out: &mut Vec<Trace>) {
    let start = profile(ENABLED, 800, 451, ASYM, 16);
    let mut t = Trace::with("set-profile/remainder-survival", start);
    t.ticks(&[(3, 1), (3, 1), (1, 3), (-2, 5)]);
    // Non-identity: remainders and direction survive.
    let linear = profile(ENABLED | LINEAR, 800, 300, ASYM, 16);
    t.op(Op::SetProfile(linear)).tick(3, 1, 1);
    t.op(Op::SetProfile(linear));
    // Identity (disabled, all 1.00x, bypass): state reset.
    t.op(Op::SetProfile(profile(0, 800, 300, ASYM, 16)));
    t.tick(7, 3, 1);
    t.op(Op::SetProfile(start)).ticks(&[(3, 1), (3, 1)]);
    t.op(Op::SetProfile(profile(ENABLED, 800, 256, [256; 8], 16)));
    t.op(Op::SetProfile(start)).ticks(&[(3, 1), (3, 1)]);
    t.op(Op::SetProfile(profile(
        ENABLED | BYPASS,
        800,
        451,
        ASYM,
        16,
    )));
    t.op(Op::SetProfile(profile(ENABLED | M2X2, 1600, 451, SHEAR, 0)));
    t.ticks(&[(3, 1), (3, 1)]);
    // Out-of-range fields are clamped and the profile re-signed.
    t.op(Op::SetProfile(Profile {
        magic: 0,
        version: 7,
        flags: ENABLED,
        base_dpi: 10,
        global_gain_q8_8: 2000,
        multipliers_q8_8: [0, 15, 16, 1024, 1025, 65535, 256, 300],
        reserved: 0xFFFF,
        crc16: 0x1234,
    }));
    t.ticks(&[(3, 1), (-3, -1), (100, 0)]);
    t.op(Op::SetProfile(Profile {
        base_dpi: 0,
        global_gain_q8_8: 0,
        reserved: 0x0081,
        ..profile(ENABLED | LINEAR, 0, 0, ASYM, 0)
    }));
    t.ticks(&[(3, 1), (1, 3)]);
    out.push(t);
}

fn misc_traces(out: &mut Vec<Trace>) {
    let mut t = Trace::with("misc/state-ops", profile(ENABLED, 800, 451, ASYM, 16));
    t.ticks(&[(3, 1), (3, 1), (1, 3)]);
    t.op(Op::NotifyIdle).tick(1, 3, 1);
    t.op(Op::ResetRemainders).tick(1, 3, 1);
    t.op(Op::Clock(0))
        .op(Op::Clock(u32::MAX))
        .op(Op::Clock(12345));
    // set_window_counts on the live profile: clamps to 128, does not re-sign.
    for w in [0, 16, 129, 255, 200, 1, 128] {
        t.op(Op::Window(w)).ticks(&[(3, 1), (1, 3)]);
    }
    out.push(t);

    // Engine::new(base): 0 means 800, below 50 clamps to 50.
    for base in [0, 1, 49, 50, 51, 65535] {
        let mut t = Trace::new(&format!("misc/init-base-{base}"), base, None);
        t.ticks(&[(3, 1), (-32768, -32768)]);
        out.push(t);
    }
}

fn headroom_traces(out: &mut Vec<Trace>) {
    let cases: [(&str, u8, u16, u16, [u16; 8]); 11] = [
        ("headroom/asym-gain-451", ENABLED, 800, 451, ASYM),
        // max(M·G) == 1.00x exactly: no rescale, gain stays 512.
        (
            "headroom/exact-1x-gain-512",
            ENABLED,
            800,
            512,
            [128, 100, 64, 128, 16, 90, 128, 50],
        ),
        ("headroom/asym-ccw", ENABLED | CCW, 1600, 256, ASYM),
        (
            "headroom/no-boost",
            ENABLED,
            800,
            256,
            [256, 200, 128, 16, 255, 100, 50, 256],
        ),
        ("headroom/clamp-65535", ENABLED, 65535, 1024, [1024; 8]),
        ("headroom/tiny-at-50", ENABLED, 50, 16, [16; 8]),
        (
            "headroom/boost-from-50",
            ENABLED,
            50,
            1024,
            [1024, 16, 16, 16, 16, 16, 16, 16],
        ),
        (
            "headroom/rounding",
            ENABLED,
            1001,
            256,
            [257, 256, 256, 256, 256, 256, 256, 256],
        ),
        ("headroom/disabled", 0, 800, 600, ASYM),
        (
            "headroom/min-clamp",
            ENABLED,
            800,
            1024,
            [16, 1024, 16, 1024, 16, 1024, 16, 1024],
        ),
        ("headroom/2x2", ENABLED | M2X2, 800, 300, SHEAR),
    ];
    for (name, f, base, gain, m) in cases {
        let mut t = Trace::with(name, profile(f, base, gain, m, 16));
        t.ticks(&[(5, 2), (-3, 7)]);
        t.op(Op::Normalize).op(Op::Normalize);
        t.ticks(&[(5, 2), (-3, 7), (2, -9), (-32768, 32767)]);
        out.push(t);
    }
}

fn ccw_traces(out: &mut Vec<Trace>) {
    const MOVES: [(i16, i16); 4] = [(5, 2), (2, 5), (-5, 2), (-2, -5)];
    let cases: [(&str, u8, [u16; 8], bool); 4] = [
        ("ccw/round-trip", ENABLED, ASYM, true),
        ("ccw/from-ccw", ENABLED | CCW, ASYM, false),
        ("ccw/2x2", ENABLED | M2X2, SHEAR, true),
        ("ccw/disabled", 0, ASYM, true),
    ];
    for (name, f, m, first) in cases {
        let mut t = Trace::with(name, profile(f, 800, 300, m, 16));
        t.ticks(&MOVES);
        // Converting the frame keeps the physical curve: same outputs.
        t.op(Op::Ccw(first)).ticks(&MOVES);
        t.op(Op::Ccw(first)); // already in that frame: no-op
        t.op(Op::Ccw(!first)).ticks(&MOVES);
        out.push(t);
    }
}

fn delta_payload(dx: i16, dy: i16) -> [u8; 4] {
    let (x, y) = (dx.to_le_bytes(), dy.to_le_bytes());
    [x[0], x[1], y[0], y[1]]
}

fn hid_traces(out: &mut Vec<Trace>) {
    let base = profile(ENABLED, 800, 300, ASYM, 16);
    let magic = BOOTLOADER_MAGIC_ARG.to_le_bytes();

    // Framing: wrong 'D'/'P'/namespace -> not handled (response untouched);
    // wrong version or CRC -> ERR_CRC_OR_FMT and no side effect.
    let mut t = Trace::with("hid/framing", base);
    t.ticks(&[(5, 2), (5, 2)]);
    let caps = hid_request(1, cmd::GET_CAPABILITIES, 0, &[]);
    t.hid_req(caps, None, false);
    for (i, v) in [(0, 0x45), (1, 0x51), (5, 0xDE), (5, 0x00)] {
        let mut r = caps;
        r[i] = v;
        sign_request(&mut r);
        t.hid_req(r, None, false);
    }
    t.hid_req([0; 64], None, false)
        .hid_req([0xFF; 64], None, false);
    for v in [0, 2, 0xFF] {
        let mut r = caps;
        r[2] = v;
        sign_request(&mut r);
        t.hid_req(r, Some(true), true);
    }
    for i in [62, 63, 8, 30] {
        let mut r = caps;
        r[i] ^= 0x01;
        t.hid_req(r, Some(true), true);
    }
    let set = c::serialize(&profile(ENABLED | LINEAR, 1600, 451, ASYM, 32));
    let state_changing = [
        hid_request(2, cmd::SET_PROFILE, 0, &set),
        hid_request(3, cmd::RESET_DEFAULTS, 0, &[]),
        hid_request(4, cmd::SET_ENABLED, 0, &[0]),
        hid_request(5, cmd::SIMULATE_DELTA, 0, &delta_payload(100, 100)),
        hid_request(6, cmd::SAVE_FLASH, 0, &[]),
        hid_request(7, cmd::ENTER_BOOTLOADER, 0, &magic),
    ];
    for req in state_changing {
        let mut r = req;
        r[2] = 2;
        sign_request(&mut r);
        t.hid_req(r, Some(true), true);
        let mut r = req;
        r[63] ^= 0x80;
        t.hid_req(r, Some(true), true);
    }
    t.tick(5, 2, 1);
    out.push(t);

    // Capability bits follow the callbacks, sensor DPI and clock.
    let mut t = Trace::with("hid/capabilities", base);
    for (save, boot) in [
        (None, false),
        (Some(true), false),
        (None, true),
        (Some(false), true),
    ] {
        t.hid(cmd::GET_CAPABILITIES, &[], save, boot);
    }
    t.op(Op::SensorDpi(1600))
        .hid(cmd::GET_CAPABILITIES, &[], None, false);
    t.op(Op::Clock(0xDEAD_BEEF))
        .hid(cmd::GET_CAPABILITIES, &[], Some(true), true);
    t.op(Op::Clock(0))
        .hid(cmd::GET_CAPABILITIES, &[], None, false);
    t.op(Op::SensorDpi(0))
        .hid(cmd::GET_CAPABILITIES, &[], None, false);
    // Sequence and slot are echoed, the payload is ignored.
    t.hid_req(
        hid_request(0xFF, cmd::GET_CAPABILITIES, 0x7F, &[0xAA; 54]),
        None,
        false,
    );
    out.push(t);

    // GET_PROFILE / SET_PROFILE incl. every deserialize rejection.
    let mut t = Trace::with("hid/profile", base);
    t.hid(cmd::GET_PROFILE, &[], None, false);
    t.ticks(&[(5, 2), (5, 2), (2, 5)]);
    let linear = c::serialize(&profile(ENABLED | LINEAR, 800, 451, ASYM, 32));
    t.hid(cmd::SET_PROFILE, &linear, None, false); // non-identity: remainders survive
    t.tick(5, 2, 1);
    let disabled = c::serialize(&profile(0, 800, 451, ASYM, 32));
    t.hid(cmd::SET_PROFILE, &disabled, None, false); // identity: reset
    t.hid(cmd::SET_PROFILE, &linear, None, false);
    t.ticks(&[(5, 2), (5, 2)]);
    let good = profile(ENABLED, 1600, 300, ASYM, 16);
    let mut rejected = Vec::new();
    let edits: [fn(&mut Profile); 10] = [
        |p: &mut Profile| p.magic = 0x4451,
        |p: &mut Profile| p.version = 2,
        |p: &mut Profile| p.base_dpi = 0,
        |p: &mut Profile| p.base_dpi = 49,
        |p: &mut Profile| p.global_gain_q8_8 = 15,
        |p: &mut Profile| p.global_gain_q8_8 = 1025,
        |p: &mut Profile| p.global_gain_q8_8 = 0,
        |p: &mut Profile| p.multipliers_q8_8[0] = 15,
        |p: &mut Profile| p.multipliers_q8_8[7] = 1025,
        |p: &mut Profile| p.multipliers_q8_8[3] = 0xFFFF,
    ];
    for edit in edits {
        let mut p = good;
        edit(&mut p);
        rejected.push(signed_raw(&p));
    }
    let mut b = c::serialize(&good);
    b[26] ^= 1; // CRC byte
    rejected.push(b);
    let mut b = c::serialize(&good);
    b[10] ^= 1; // payload changed after signing
    rejected.push(b);
    rejected.push([0; 28]);
    for b in rejected {
        t.hid(cmd::SET_PROFILE, &b, Some(true), true);
    }
    // Accepted as stored (flags 0xFF, window 200, idle 255, bounds 50/16/1024),
    // then sanitised by set_profile.
    let edge = profile(
        0xFF,
        50,
        16,
        [1024, 16, 1024, 16, 1024, 16, 1024, 16],
        0xFFC8,
    );
    t.hid(cmd::SET_PROFILE, &signed_raw(&edge), None, false);
    t.hid(cmd::GET_PROFILE, &[], None, false);
    let top = profile(ENABLED, 65535, 1024, ASYM, 0x0280);
    t.hid(cmd::SET_PROFILE, &signed_raw(&top), None, false);
    t.ticks(&[(5, 2), (2, 5)]);
    // Slot byte and trailing payload bytes are ignored.
    let mut payload = [0x5A; 54];
    payload[..28].copy_from_slice(&c::serialize(&good));
    t.hid_req(hid_request(9, cmd::SET_PROFILE, 3, &payload), None, false);
    out.push(t);

    // SAVE_FLASH: no callback -> unsupported; callback false -> CRC_OR_FMT.
    let mut t = Trace::with("hid/save-flash", base);
    for save in [None, Some(true), Some(false)] {
        t.hid(cmd::SAVE_FLASH, &[], save, false);
        t.hid(cmd::SAVE_FLASH, &[], save, true);
    }
    // The callback sees the stored profile (stale CRC after Window); the
    // response is re-signed.
    t.op(Op::Window(200))
        .hid(cmd::SAVE_FLASH, &[], Some(true), false);
    t.op(Op::SetProfile(profile(
        ENABLED | M2X2,
        3200,
        451,
        SHEAR,
        0x0A20,
    )));
    t.hid(cmd::SAVE_FLASH, &[], Some(true), true);
    out.push(t);

    // RESET_DEFAULTS keeps base_dpi, sensor DPI and clock.
    let mut t = Trace::with("hid/reset-defaults", profile(ENABLED, 1600, 451, ASYM, 16));
    t.op(Op::SensorDpi(1600)).op(Op::Clock(777));
    t.ticks(&[(5, 2), (5, 2)]);
    t.hid(cmd::RESET_DEFAULTS, &[], None, false);
    t.tick(5, 2, 1);
    let p3200 = c::serialize(&profile(ENABLED, 3200, 451, ASYM, 16));
    t.hid(cmd::SET_PROFILE, &p3200, None, false).tick(5, 2, 1);
    t.hid(cmd::RESET_DEFAULTS, &[], Some(true), true);
    t.hid(cmd::GET_PROFILE, &[], None, false);
    out.push(t);
    let mut t = Trace::new("hid/reset-defaults-from-init", 0, None);
    t.hid(cmd::RESET_DEFAULTS, &[], None, false).tick(5, 2, 1);
    out.push(t);

    // SET_ENABLED 0 / 1 / 2 (calibration bypass) / other non-zero values.
    let mut t = Trace::with("hid/set-enabled", profile(ENABLED, 800, 451, ASYM, 16));
    t.ticks(&[(5, 2), (5, 2)]);
    for mode in [0, 2, 1, 2, 3, 255, 0, 0, 1, 1] {
        t.hid(cmd::SET_ENABLED, &[mode], None, false);
        t.ticks(&[(5, 2), (3, 7)]);
    }
    out.push(t);

    // SIMULATE_DELTA runs process_delta on the live engine.
    let cases: [(&str, u8, u16, [u16; 8]); 3] = [
        ("hid/simulate-16x", ENABLED, 1024, [1024; 8]),
        ("hid/simulate-2x2", ENABLED | M2X2, 300, SHEAR),
        ("hid/simulate-off", 0, 300, ASYM),
    ];
    for (name, f, gain, m) in cases {
        let mut t = Trace::with(name, profile(f, 800, gain, m, 16));
        for (dx, dy) in [
            (1, 0),
            (32767, -32768),
            (-32768, -32768),
            (0, 0),
            (5, 0),
            (-32768, 32767),
            (3, -1),
        ] {
            t.hid(cmd::SIMULATE_DELTA, &delta_payload(dx, dy), None, false);
        }
        t.hid(cmd::SET_ENABLED, &[0], None, false);
        t.hid(
            cmd::SIMULATE_DELTA,
            &delta_payload(-32768, -32768),
            None,
            false,
        );
        out.push(t);
    }

    // ENTER_BOOTLOADER: wrong magic -> ERR_RANGE (before the callback check),
    // no callback -> unsupported, else the callback runs once.
    let mut t = Trace::with("hid/bootloader", base);
    t.hid(cmd::ENTER_BOOTLOADER, &[0; 4], None, true);
    t.hid(cmd::ENTER_BOOTLOADER, &[0; 4], None, false);
    t.hid(
        cmd::ENTER_BOOTLOADER,
        &BOOTLOADER_MAGIC_ARG.to_be_bytes(),
        Some(true),
        true,
    );
    t.hid(cmd::ENTER_BOOTLOADER, &magic, None, false);
    t.hid(cmd::ENTER_BOOTLOADER, &magic, Some(false), true);
    let mut payload = [0xA5; 54];
    payload[..4].copy_from_slice(&magic);
    t.hid(cmd::ENTER_BOOTLOADER, &payload, None, true);
    out.push(t);

    let mut t = Trace::with("hid/unknown-opcode", base);
    for opcode in [0x00, 0x08, 0x0E, 0x10, 0x7F, 0xDF, 0xFF] {
        t.hid(opcode, &[0x11; 54], Some(true), true);
    }
    let mut r = hid_request(1, 0x08, 0, &[]);
    r[62] ^= 1;
    t.hid_req(r, None, false);
    out.push(t);
}

/// Fixed-seed random runs: the fuzzer's operation mix, and a HID-heavy mix.
fn random_traces(out: &mut Vec<Trace>) {
    for i in 0..13u64 {
        let mut rng = XorShift::new(0x7EAC_E000 + i);
        let base = rng.pick(&[0, 50, 800, 1600, 65535]);
        let profile = rng.profile();
        let hid_heavy = i >= 10;
        let name = if hid_heavy {
            format!("random/hid-{i}")
        } else {
            format!("random/{i}")
        };
        let mut t = Trace::new(&name, base, Some(profile));
        for _ in 0..if hid_heavy { 60 } else { 110 } {
            let op = if hid_heavy && rng.bool() {
                rng.hid()
            } else {
                rng.op(&t.c.engine())
            };
            t.op(op);
        }
        out.push(t);
    }
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: gen-traces <output.json>");
        std::process::exit(2);
    };

    let mut traces = Vec::new();
    window_traces(&mut traces);
    flag_traces(&mut traces);
    model_2x2_traces(&mut traces);
    reverse_traces(&mut traces);
    carry_traces(&mut traces);
    idle_traces(&mut traces);
    dpi_traces(&mut traces);
    set_profile_traces(&mut traces);
    misc_traces(&mut traces);
    headroom_traces(&mut traces);
    ccw_traces(&mut traces);
    hid_traces(&mut traces);
    random_traces(&mut traces);

    let step_count: usize = traces.iter().map(|t| t.steps.len()).sum();
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"suite\": \"kineforge/kf-dirgain/engine-traces\",\n");
    json.push_str("  \"generator\": \"crates/kf-dirgain-oracle/src/bin/gen-traces.rs\",\n");
    json.push_str(
        "  \"reference\": \"open-directional-dpi core/directional_dpi.c @ c177d23c2d6243f676ab49a9531ae120fd0d896c\",\n",
    );
    let _ = writeln!(
        json,
        "  \"response_fill\": {},",
        kf_dirgain_oracle::RESPONSE_FILL
    );
    let _ = writeln!(json, "  \"trace_count\": {},", traces.len());
    let _ = writeln!(json, "  \"step_count\": {step_count},");
    json.push_str("  \"traces\": [\n");
    let bodies: Vec<String> = traces.iter().map(Trace::json).collect();
    json.push_str(&bodies.join(",\n"));
    json.push_str("\n  ]\n}\n");

    if let Err(e) = std::fs::write(&path, &json) {
        eprintln!("gen-traces: cannot write {path}: {e}");
        std::process::exit(1);
    }

    let mut categories: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for t in &traces {
        let category = t.name.split('/').next().unwrap_or(&t.name);
        let entry = categories.entry(category).or_default();
        entry.0 += 1;
        entry.1 += t.steps.len();
    }
    for (category, (n, steps)) in &categories {
        eprintln!("{category:>12}: {n:3} traces, {steps:5} steps");
    }
    eprintln!(
        "{:>12}: {:3} traces, {step_count:5} steps, {} bytes -> {path}",
        "total",
        traces.len(),
        json.len()
    );
}
