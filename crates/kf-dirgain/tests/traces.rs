//! Stateful engine traces recorded from the C reference engine by
//! `crates/kf-dirgain-oracle` (`gen-traces`; format in that crate's README).
//! Every step is replayed on the Rust engine and its outputs and full state
//! must match bit for bit. Embedded with `include_str!` so the test also runs
//! on `wasm32-wasip1`.

use kf_dirgain::{Engine, HID_PACKET_LEN, PROFILE_LEN, Profile};
use serde_json::{Map, Value, json};

const TRACES: &str = include_str!("vectors/traces.json");

fn bytes<const N: usize>(v: &Value) -> [u8; N] {
    let hex = v.as_str().expect("hex string");
    assert_eq!(hex.len(), 2 * N, "expected {N} hex bytes");
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex byte");
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fields as stored, little-endian, without validation.
fn profile_from_hex(v: &Value) -> Profile {
    let b: [u8; PROFILE_LEN] = bytes(v);
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    Profile {
        magic: u16_at(0),
        version: b[2],
        flags: b[3],
        base_dpi: u16_at(4),
        global_gain_q8_8: u16_at(6),
        multipliers_q8_8: core::array::from_fn(|i| u16_at(8 + 2 * i)),
        reserved: u16_at(24),
        crc16: u16_at(26),
    }
}

/// Inverse of [`profile_from_hex`].
fn profile_hex(p: &Profile) -> String {
    let mut b = Vec::with_capacity(PROFILE_LEN);
    b.extend(p.magic.to_le_bytes());
    b.extend([p.version, p.flags]);
    b.extend(p.base_dpi.to_le_bytes());
    b.extend(p.global_gain_q8_8.to_le_bytes());
    for m in p.multipliers_q8_8 {
        b.extend(m.to_le_bytes());
    }
    b.extend(p.reserved.to_le_bytes());
    b.extend(p.crc16.to_le_bytes());
    hex(&b)
}

fn state(e: &Engine) -> Map<String, Value> {
    let Value::Object(map) = json!({
        "rem_x": e.rem_x_q16_16,
        "rem_y": e.rem_y_q16_16,
        "dir_x": e.dir_x_q16_16,
        "dir_y": e.dir_y_q16_16,
        "last_octant": e.last_octant,
        "last_scale": e.last_scale_q16_16,
        "last_motion_ms": e.last_motion_ms,
        "sensor_dpi": e.sensor_dpi,
        "clock_known": e.clock_known,
        "clock_ms": e.clock_ms,
        "profile_hex": profile_hex(&e.profile),
    }) else {
        unreachable!()
    };
    map
}

fn int<T: TryFrom<i64>>(step: &Value, key: &str) -> T {
    step[key]
        .as_i64()
        .and_then(|v| T::try_from(v).ok())
        .unwrap_or_else(|| panic!("field {key} missing or out of range"))
}

/// Runs one step and returns everything the trace's `expect` object records.
fn run_step(e: &mut Engine, step: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    match step["op"].as_str().expect("op") {
        "delta" => {
            let (x, y) = e.process_delta(int(step, "dx"), int(step, "dy"));
            out.insert("out_dx".into(), x.into());
            out.insert("out_dy".into(), y.into());
        }
        "motion" => {
            let (x, y) = e.process_motion(int(step, "dx"), int(step, "dy"), int(step, "now"));
            out.insert("out_dx".into(), x.into());
            out.insert("out_dy".into(), y.into());
        }
        "sensor_dpi" => e.set_sensor_dpi(int(step, "dpi")),
        "clock" => e.set_clock_ms(int(step, "now")),
        "set_profile" => e.set_profile(&profile_from_hex(&step["profile_hex"])),
        "notify_idle" => e.notify_idle(),
        "reset_remainders" => e.reset_remainders(),
        "normalize" => {
            let cpi = e.profile.normalize_cpi_headroom();
            out.insert("sensor_cpi".into(), cpi.into());
        }
        "ccw" => e
            .profile
            .set_ccw_frame(step["use_ccw"].as_bool().expect("use_ccw")),
        "window" => e.profile.set_window_counts(int(step, "counts")),
        "hid" => {
            let req: [u8; HID_PACKET_LEN] = bytes(&step["request"]);
            let save = step["save"].as_bool();
            let boot = step["boot"].as_bool().expect("boot");
            let mut resp = [0xEE; HID_PACKET_LEN];
            let (mut save_calls, mut saved, mut boot_calls) = (0u32, Value::Null, 0u32);
            let mut save_cb = |p: &Profile| {
                save_calls += 1;
                saved = profile_hex(p).into();
                save.unwrap_or(false)
            };
            let mut boot_cb = || boot_calls += 1;
            let save_arg: Option<&mut dyn FnMut(&Profile) -> bool> = if save.is_some() {
                Some(&mut save_cb)
            } else {
                None
            };
            let boot_arg: Option<&mut dyn FnMut()> = if boot { Some(&mut boot_cb) } else { None };
            let handled = e.handle_feature_report(&req, &mut resp, save_arg, boot_arg);
            out.insert("handled".into(), handled.into());
            out.insert("response".into(), hex(&resp).into());
            out.insert("save_calls".into(), save_calls.into());
            out.insert("saved_profile_hex".into(), saved);
            out.insert("boot_calls".into(), boot_calls.into());
        }
        other => panic!("unknown op {other}"),
    }
    out.extend(state(e));
    out
}

/// Panics with every differing field unless `actual` equals `expect` exactly.
fn check(trace: &str, at: &str, what: &Value, expect: &Value, actual: &Map<String, Value>) {
    let expect = expect.as_object().expect("expect object");
    let mut diffs = Vec::new();
    for (key, want) in expect {
        match actual.get(key) {
            Some(got) if got == want => {}
            got => diffs.push(format!(
                "{key}: C {want}, Rust {}",
                got.map_or("missing".into(), Value::to_string)
            )),
        }
    }
    for key in actual.keys().filter(|k| !expect.contains_key(*k)) {
        diffs.push(format!("{key}: not recorded in the trace"));
    }
    assert!(
        diffs.is_empty(),
        "trace {trace}, {at} {what}:\n  {}",
        diffs.join("\n  ")
    );
}

#[test]
fn traces_match_c_reference() {
    let doc: Value = serde_json::from_str(TRACES).expect("valid JSON");
    assert_eq!(
        doc["response_fill"],
        json!(0xEE),
        "response buffers start as 0xEE"
    );
    let traces = doc["traces"].as_array().expect("traces array");
    assert_eq!(traces.len(), int::<usize>(&doc, "trace_count"));

    let mut steps_run = 0;
    for trace in traces {
        let name = trace["name"].as_str().expect("trace name");
        let mut engine = Engine::new(int(trace, "base_dpi"));
        if !trace["profile_hex"].is_null() {
            engine.set_profile(&profile_from_hex(&trace["profile_hex"]));
        }
        check(
            name,
            "init",
            &trace["profile_hex"],
            &trace["init"],
            &state(&engine),
        );

        for (i, step) in trace["steps"].as_array().expect("steps").iter().enumerate() {
            let actual = run_step(&mut engine, step);
            let mut op = step.clone();
            op.as_object_mut().expect("step object").remove("expect");
            check(name, &format!("step {i}"), &op, &step["expect"], &actual);
            steps_run += 1;
        }
    }
    assert_eq!(steps_run, int::<usize>(&doc, "step_count"));
}
