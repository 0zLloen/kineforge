//! Differential fuzzing of kf-dirgain against the C reference engine: both run
//! the same seeded random operations, and the outputs and the full engine
//! state must match after every one. Profile functions get random 28-byte
//! buffers. The `*_long` tests (10^8 compared engine ops, 10^7 buffers) are
//! ignored by default:
//! `cargo test --release -p kf-dirgain-oracle --test fuzz -- --ignored`.

use std::fmt::Debug;

use kf_dirgain::{Engine, PROFILE_LEN, PROFILE_MAGIC, PROFILE_VERSION, Profile, flags};
use kf_dirgain_oracle::{COracle, XorShift, c, profile_from_raw, signed_raw};

/// Compared operations per case (state pokes and raw profile writes are extra).
const OPS_PER_CASE: u32 = 1000;
const REM_CAP: i32 = 2_147_418_112;
/// Upper bound for raw (unsanitised) M and G. Every M·G < 2^30, so the C 2x2
/// model's int32 `gx + gy` and `gne - gnw` cannot overflow (signed overflow is
/// UB in C), while `normalize_cpi_headroom`'s u32 `M·G·256` still wraps for
/// M·G >= 2^24 (unsigned, defined in C).
const RAW_Q8_MAX: u16 = 32767;

/// Fields that differ between two values' pretty `Debug` output.
fn diff<T: Debug>(c: &T, rust: &T) -> String {
    let (c, rust) = (format!("{c:#?}"), format!("{rust:#?}"));
    c.lines()
        .zip(rust.lines())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| format!("    C {} | Rust {}", a.trim(), b.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Overwrites the free-running state with random values, mostly reachable
/// ones (|rem| <= cap, |dir| <= 16383), sometimes any i32, plus clock edges.
fn poke(rng: &mut XorShift, e: &mut Engine) {
    let any = rng.u32() as i32;
    let rem = (rng.u32() as i32) % REM_CAP;
    e.rem_x_q16_16 = rng.pick(&[0, REM_CAP, -REM_CAP, rem, rem, any]);
    let rem = (rng.u32() as i32) % REM_CAP;
    e.rem_y_q16_16 = rng.pick(&[0, REM_CAP, -REM_CAP, rem, rem, any]);
    let d = rng.below(32767) as i32 - 16383;
    e.dir_x_q16_16 = rng.pick(&[0, 16383, -16383, d, d, any]);
    let (d, any) = (rng.below(32767) as i32 - 16383, rng.u32() as i32);
    e.dir_y_q16_16 = rng.pick(&[0, 16383, -16383, d, d, any]);
    let t = rng.u32();
    e.last_motion_ms = rng.pick(&[e.last_motion_ms, u32::MAX, u32::MAX - 3, t]);
}

/// Unsanitised profile for the pub field `Engine::profile` (in C a plain
/// struct field): any flags (2x2 forced on half the time), magic, version, base CPI,
/// window and idle bytes, and M and G in 0..=[`RAW_Q8_MAX`].
fn raw_profile(rng: &mut XorShift) -> Profile {
    let mut p = rng.profile();
    if rng.bool() {
        p.flags |= flags::MODEL_LINEAR_2X2;
    }
    let mut q8 = || match rng.below(4) {
        0 => rng.pick(&[0, 1, 15, 1025, 4096, RAW_Q8_MAX]),
        1 => rng.q8().min(RAW_Q8_MAX),
        _ => rng.below(u32::from(RAW_Q8_MAX) + 1) as u16,
    };
    p.multipliers_q8_8 = std::array::from_fn(|_| q8());
    p.global_gain_q8_8 = q8();
    p
}

/// `ops` random operations from `seed` on both engines, interleaved with
/// uncounted state pokes and raw profile writes (the same values written into
/// both engines). Returns the number of compared operations; `Err` describes
/// the first mismatch.
fn engine_case(seed: u64, ops: u32) -> Result<u32, String> {
    let mut rng = XorShift::new(seed);
    let any = rng.u16();
    let base = rng.pick(&[0, 1, 49, 50, 800, 1600, 65535, any]);
    let mut rust = Engine::new(base);
    let mut c = COracle::new(base);
    let first = rng.profile();
    rust.set_profile(&first);
    c.set_profile(&first);
    if c.engine() != rust {
        return Err(format!(
            "seed {seed:#x} init(base {base}) + set_profile({first:?}):\n{}",
            diff(&c.engine(), &rust)
        ));
    }

    let mut compared = 0;
    while compared < ops {
        // Uncounted writes: random state, or a raw profile into the pub field.
        // The states are equal here, so copying the Rust state into C writes
        // the same values there.
        let write = match rng.below(50) {
            0 => {
                poke(&mut rng, &mut rust);
                true
            }
            1 => {
                rust.profile = raw_profile(&mut rng);
                true
            }
            _ => false,
        };
        if write {
            c = COracle::from_engine(&rust);
            continue;
        }
        let before = rust;
        let op = rng.op(&rust);
        let c_out = op.run_c(&mut c);
        let rust_out = op.run_rust(&mut rust);
        let c_state = c.engine();
        if c_out != rust_out || c_state != rust {
            return Err(format!(
                "seed {seed:#x} op {compared}: {op:?}\n  state before: {before:?}\n  outcome:\n{}\n  state:\n{}",
                diff(&c_out, &rust_out),
                diff(&c_state, &rust)
            ));
        }
        compared += 1;
    }
    Ok(compared)
}

fn fuzz_engine(base_seed: u64, cases: u64) {
    let mut compared = 0u64;
    for case in 0..cases {
        let seed = base_seed.wrapping_add(case);
        match engine_case(seed, OPS_PER_CASE) {
            Ok(n) => compared += u64::from(n),
            Err(e) => panic!("{e}\nreproduce: engine_case({seed:#x}, {OPS_PER_CASE})"),
        }
    }
    eprintln!("fuzz_engine: {compared} compared ops in {cases} cases from seed {base_seed:#x}");
}

fn same<T: PartialEq + Debug>(what: &str, buf: &[u8], c: T, rust: T) -> Result<(), String> {
    if c == rust {
        return Ok(());
    }
    Err(format!("{what} on bytes {buf:02x?}:\n{}", diff(&c, &rust)))
}

/// A random 28-byte buffer through every profile function of both sides.
fn profile_case(rng: &mut XorShift) -> Result<(), String> {
    let mut buf = [0u8; PROFILE_LEN];
    match rng.below(4) {
        0 => buf.fill_with(|| rng.u8()),
        1 => buf = c::serialize(&rng.profile()),
        2 => buf = signed_raw(&rng.profile()),
        _ => {
            buf.fill_with(|| rng.u8());
            let mut p = profile_from_raw(&buf);
            p.magic = PROFILE_MAGIC;
            p.version = PROFILE_VERSION;
            buf = signed_raw(&p);
        }
    }
    if rng.below(4) == 0 {
        buf[rng.below(28) as usize] ^= 1 << rng.below(8);
    }

    same(
        "deserialize",
        &buf,
        c::deserialize(&buf),
        Profile::deserialize(&buf),
    )?;
    let p = profile_from_raw(&buf);
    same("is_identity", &buf, c::is_identity(&p), p.is_identity())?;
    same(
        "window_counts",
        &buf,
        c::window_counts(&p),
        p.window_counts(),
    )?;
    same("idle_ms", &buf, c::idle_ms(&p), p.idle_ms())?;
    same("serialize", &buf, c::serialize(&p), p.serialize())?;

    let (mut a, mut b) = (p, p);
    c::sanitize_and_sign(&mut a);
    b.sanitize_and_sign();
    same("sanitize_and_sign", &buf, a, b)?;

    let (mut a, mut b) = (p, p);
    let cpi = (
        c::normalize_cpi_headroom(&mut a),
        b.normalize_cpi_headroom(),
    );
    same("normalize_cpi_headroom", &buf, (cpi.0, a), (cpi.1, b))?;

    let use_ccw = rng.bool();
    let (mut a, mut b) = (p, p);
    c::set_ccw_frame(&mut a, use_ccw);
    b.set_ccw_frame(use_ccw);
    same("set_ccw_frame", &buf, a, b)?;

    let w = rng.u8();
    let (mut a, mut b) = (p, p);
    c::set_window_counts(&mut a, w);
    b.set_window_counts(w);
    same("set_window_counts", &buf, a, b)?;

    same(
        "profile_init_default",
        &buf,
        c::profile_init_default(p.base_dpi),
        Profile::new_default(p.base_dpi),
    )?;

    for _ in 0..4 {
        let (dx, dy) = (rng.delta(), rng.delta());
        same(
            &format!("compute_scale_q16_16({dx}, {dy})"),
            &buf,
            c::compute_scale_q16_16(&p, dx, dy),
            kf_dirgain::compute_scale_q16_16(&p, dx, dy),
        )?;
    }

    let n = rng.below(29) as usize;
    same(
        "crc16_ccitt",
        &buf[..n],
        c::crc16_ccitt(&buf[..n]),
        kf_dirgain::crc16_ccitt(&buf[..n]),
    )
}

fn fuzz_profiles(seed: u64, cases: u64) {
    let mut rng = XorShift::new(seed);
    for case in 0..cases {
        if let Err(e) = profile_case(&mut rng) {
            panic!("profile seed {seed:#x} case {case}: {e}");
        }
    }
    eprintln!("fuzz_profiles: {cases} buffers from seed {seed:#x}");
}

#[test]
fn fuzz_engine_2m_ops() {
    fuzz_engine(0xD1FF_0000_0000, 2_000);
}

#[test]
#[ignore = "10^8 compared ops: run in release mode"]
fn fuzz_engine_long() {
    fuzz_engine(0x10A6_0000_0000, 100_000);
}

#[test]
fn fuzz_profiles_200k() {
    fuzz_profiles(0xB0FF_E125, 200_000);
}

#[test]
#[ignore = "10^7 buffers: run in release mode"]
fn fuzz_profiles_long() {
    fuzz_profiles(0x10A6_B0FF, 10_000_000);
}
