//! Live → KFLOG → `kf-cli replay`: a static-flick session driven by a
//! simple aiming bot is simulated live, recorded with kf-telemetry's
//! Recorder the way the trainer will (inputs logged as they arrive, up to a
//! millisecond before the frame that drains them, so some land a group
//! earlier; one message per frame; groups cut only between frame messages), then
//! validated and replayed by the binary.

use kf_core::input::{button, button_state};
use kf_core::records::control_action as act;
use kf_core::{
    Button, ClockKind, ClockMeaning, EvidenceClass, InputEvent, InputLevel, LinearSensitivity,
    Motion, Nanos, ReceiptMeaning, Units, ValueScale,
};
use kf_sim::{Control, FlickConfig, FrameIn, Scenario, Sim, SimConfig, SimRecord};
use kf_telemetry::format::Kind;
use kf_telemetry::json::{
    ClockDomain, Sensitivity, SessionEnd, SessionHeader, Simulation, SourceDeclaration,
};
use kf_telemetry::record::Record;
use kf_telemetry::recorder::{FlushPolicy, Recorder};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MS: i64 = 1_000_000;

fn tmp(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn header(cfg: &SimConfig, gain_hex: Option<String>) -> SessionHeader {
    let s = cfg.sensitivity;
    let Scenario::StaticFlick(flick) = cfg.scenario;
    SessionHeader {
        schema: kf_telemetry::json::SCHEMA.into(),
        session_id: "0b6c9a52-4f1e-4d2a-9b7c-2e8f1a3d5c40".into(),
        started_at_utc: None,
        clock_domains: vec![ClockDomain {
            id: 0,
            kind: ClockKind::Session,
            coi: None,
            meaning: ClockMeaning::SessionClock,
            resolution_ns: 1,
            offset_to_session_ns: 0,
            offset_uncertainty_ns: 0,
            drift_ppm: 0.0,
        }],
        simulation: Some(Simulation {
            sim_hz: cfg.sim_hz,
            input_source: cfg.driving_source,
            rest: [(
                "rng".to_owned(),
                json!({"algorithm": "xoshiro256++", "seeding": "splitmix64", "seed": format!("{:#x}", cfg.seed)}),
            )]
            .into(),
        }),
        sensitivity: Some(Sensitivity {
            model: "linear".into(),
            yaw_rad_per_unit: s.yaw_rad_per_unit,
            pitch_rad_per_unit: s.pitch_rad_per_unit,
            sens_x: s.sens_x,
            sens_y: s.sens_y,
            invert_y: s.invert_y,
            rest: [(
                "directional_gain".to_owned(),
                gain_hex.map_or(Value::Null, |p| {
                    json!({"profile": p, "engine": "kf-dirgain/odd-c177d23", "policy": "carry_capped"})
                }),
            )]
            .into(),
        }),
        rest: [
            (
                "scenario".to_owned(),
                json!({"id": "flick.static", "version": 1, "config": flick}),
            ),
            ("scoring".to_owned(), json!({"model": "kf.flick.v1", "scoring_version": 1})),
        ]
        .into(),
    }
}

fn declare(id: u16) -> Record {
    Record::Json {
        kind: Kind::SourceDeclare,
        value: serde_json::to_value(SourceDeclaration {
            id,
            source_rank: None,
            backend: "synthetic".into(),
            input_level: InputLevel::Synthetic,
            units: Units::Counts,
            value_scale: 1,
            clock_domain: 0,
            receipt_meaning: ReceiptMeaning::AppDequeue,
            evidence_class: EvidenceClass::SimulatedTruth,
            processing: vec!["injected_test_stream".into()],
            rest: Default::default(),
        })
        .unwrap(),
    }
}

fn to_record(r: SimRecord) -> Record {
    match r {
        SimRecord::Frame(f) => Record::Frame(f),
        SimRecord::Drain(d) => Record::Drain(d),
        SimRecord::Control(c) => Record::SimControl(c),
        SimRecord::Camera(c) => Record::SimCamera(c),
        SimRecord::Target(t) => Record::SimTarget(t),
        SimRecord::Shot(s) => Record::SimShot(s),
    }
}

/// Plays `seconds` of static flick live and returns the raw log. `tamper`
/// alters the n-th sim_shot record before it is recorded.
fn session(
    cfg: &SimConfig,
    gain_hex: Option<String>,
    seconds: i64,
    tamper: Option<usize>,
) -> Vec<u8> {
    let mut rec = Recorder::new(&header(cfg, gain_hex), FlushPolicy::default()).unwrap();
    let mut log = Vec::new();
    rec.push(declare(0)).unwrap();
    rec.push(declare(1)).unwrap(); // record-only second mouse
    let mut sim = Sim::new(cfg.clone()).unwrap();
    let (mut seq, mut seq1, mut frame, mut shots) = (0u32, 0u32, 0u64, 0usize);
    let (mut carry, mut down) = ((0f64, 0f64), false);
    let (mut pending, mut out) = (Vec::new(), Vec::new());
    let mut t_prev = 0;
    let s = cfg.sensitivity;
    for ms in 0..seconds * 1000 {
        let t = (ms + 1) * MS;
        // One report per ms from the driving mouse, aiming at the first target.
        let (mut dx, mut dy) = (0.0, 0.0);
        if let Some(tg) = sim.targets().first() {
            let p = tg.pos_m;
            let yaw = libm::atan2(p[0], -p[2]);
            let pitch = libm::atan2(p[1], libm::hypot(p[0], p[2]));
            dx = (yaw / (s.yaw_rad_per_unit * s.sens_x) - sim.camera().yaw_units() as f64) * 0.02;
            dy = (-pitch / (s.pitch_rad_per_unit * s.sens_y) - sim.camera().pitch_units() as f64)
                * 0.02;
        }
        let w = (dx + carry.0, dy + carry.1);
        let (ix, iy) = (w.0 as i32, w.1 as i32);
        carry = (w.0 - f64::from(ix), w.1 - f64::from(iy));
        let m = InputEvent::Motion(Motion {
            t_ns: Nanos(t),
            src_ts_ns: None,
            dx: ix,
            dy: iy,
            seq,
            source: 0,
            flags: 0,
        });
        seq += 1;
        let on = sim.targets().iter().any(|tg| {
            kf_sim::geom::angle_between(sim.camera().forward(), tg.pos_m) < 0.5 * tg.radius_m / 10.0
        });
        let mut evs = vec![m];
        if down || on {
            evs.push(InputEvent::Button(Button {
                t_ns: Nanos(t),
                src_ts_ns: None,
                seq,
                source: 0,
                button: button::LEFT,
                state: if down { 0 } else { button_state::DOWN },
            }));
            seq += 1;
            down = !down;
        }
        // The recorder logs input as it arrives.
        for e in &evs {
            rec.push(match *e {
                InputEvent::Motion(m) => Record::Motion(m),
                InputEvent::Button(b) => Record::Button(b),
                _ => unreachable!(),
            })
            .unwrap();
        }
        rec.push(Record::Motion(Motion {
            t_ns: Nanos(t),
            src_ts_ns: None,
            dx: 1,
            dy: 0,
            seq: seq1,
            source: 1,
            flags: 0,
        }))
        .unwrap();
        seq1 += 1;
        pending.extend(evs);
        // A frame every 7 ms.
        if (ms + 1) % 7 != 0 && ms + 1 != seconds * 1000 {
            continue;
        }
        let ctl = |a| Control {
            t_ns: Nanos(t),
            action: a,
            reason: 0,
            new_source: None,
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        match frame {
            0 => before.push(ctl(act::START_TRIAL)),
            300 => after.push(ctl(act::PAUSE)),
            340 => before.push(ctl(act::RESUME)),
            600 => before.push(ctl(act::RESTART_TRIAL)),
            _ => {}
        }
        if ms + 1 == seconds * 1000 {
            after.push(ctl(act::END));
        }
        // Input arrives before the frame that drains it: the frame's cutoff
        // is 1 ms back, so the last millisecond's reports (already logged)
        // wait for the next frame, sometimes in the next group.
        let last = ms + 1 == seconds * 1000;
        let cutoff = if last { t } else { t - MS };
        let n = pending
            .iter()
            .take_while(|e| match e {
                InputEvent::Motion(m) => m.t_ns.0 <= cutoff,
                InputEvent::Button(b) => b.t_ns.0 <= cutoff,
                _ => true,
            })
            .count();
        let f = FrameIn {
            frame,
            t_start_ns: Nanos(t_prev),
            t_input_cutoff_ns: Nanos(cutoff),
            ticks: None,
        };
        sim.run_frame(f, &before, pending.drain(..n), &after, &mut out)
            .unwrap();
        // One frame message.
        for r in out.drain(..) {
            let mut r = r;
            if let SimRecord::Shot(s) = &mut r {
                if tamper == Some(shots) {
                    s.d_yaw_rad += 1e-9;
                }
                shots += 1;
            }
            rec.push(to_record(r)).unwrap();
        }
        if rec.due(Nanos(t)) {
            log.extend(rec.cut(Nanos(t)));
        }
        frame += 1;
        t_prev = cutoff;
    }
    log.extend(rec.cut(Nanos(t_prev)));
    let end = SessionEnd {
        completed: true,
        end_reason: "completed".into(),
        t_end_ns: t_prev,
        counts: Default::default(),
        dropped_chunks: 0,
        max_writer_backlog_bytes: 0,
        interruptions: 0,
        refresh_hz_measured: None,
        raf_hz_measured: None,
        rest: Default::default(),
    };
    log.extend(rec.finish(end, Nanos(t_prev)).unwrap());
    log
}

fn run(args: &[&std::ffi::OsStr]) -> (bool, Value, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_kf-cli"))
        .args(args)
        .output()
        .unwrap();
    let v = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
    (
        o.status.success(),
        v,
        String::from_utf8_lossy(&o.stderr).into_owned() + &String::from_utf8_lossy(&o.stdout),
    )
}

fn cfg() -> SimConfig {
    let per = 0.022 * (core::f64::consts::PI / 180.0);
    let sens = LinearSensitivity {
        yaw_rad_per_unit: per,
        pitch_rad_per_unit: per,
        sens_x: 1.4,
        sens_y: 1.4,
        invert_y: false,
    };
    let mut c = SimConfig::new(sens, ValueScale::X1, 0x5eed0001);
    c.scenario = Scenario::StaticFlick(FlickConfig {
        targets: 16,
        ..FlickConfig::default()
    });
    c
}

#[test]
fn live_session_validates_and_replays_identically() {
    let d = tmp("replay-identical");
    let mut gain = kf_dirgain::Profile::new_default(800);
    gain.flags |= kf_dirgain::flags::ENABLED;
    gain.multipliers_q8_8 = [256, 300, 384, 220, 512, 256, 180, 330];
    gain.sanitize_and_sign();
    let hex: String = gain
        .serialize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    for (name, g, hexed) in [("plain", None, None), ("gain", Some(gain), Some(hex))] {
        let c = SimConfig { gain: g, ..cfg() };
        let log = session(&c, hexed, 8, None);
        let path = d.join(format!("{name}.kflog"));
        fs::write(&path, &log).unwrap();
        let (ok, _, text) = run(&["validate".as_ref(), path.as_os_str()]);
        assert!(ok, "{name}: validate: {text}");
        let (ok, v, text) = run(&["replay".as_ref(), path.as_os_str()]);
        assert!(ok, "{name}: replay: {text}");
        assert_eq!(v["identical"], json!(true));
        assert!(v["compared"]["sim_shot"].as_u64().unwrap() > 5, "{v}");
        assert!(v["compared"]["sim_target"].as_u64().unwrap() > 10, "{v}");
        let attempts = v["attempts"].as_array().unwrap();
        // The restart after a completed trial runs trial 0 again.
        let trials: Vec<_> = attempts
            .iter()
            .map(|a| (a["ordinal"].clone(), a["trial"].clone()))
            .collect();
        assert_eq!(trials, [(json!(0), json!(0)), (json!(1), json!(0))], "{v}");
        assert!(
            attempts.iter().any(|a| a["kills"].as_u64() > Some(0)),
            "{v}"
        );
    }
}

#[test]
fn a_changed_record_is_reported() {
    let d = tmp("replay-mismatch");
    let path = d.join("raw.kflog");
    fs::write(&path, session(&cfg(), None, 6, Some(3))).unwrap();
    let (ok, v, _) = run(&["replay".as_ref(), path.as_os_str()]);
    assert!(!ok);
    assert_eq!(
        (
            v["identical"].clone(),
            v["mismatch"]["kind"].clone(),
            v["mismatch"]["index"].clone()
        ),
        (json!(false), json!("sim_shot"), json!(3))
    );
}

/// A corrupt middle stops the reader; the replay of the prefix must not
/// claim the session is identical (records past it were never compared).
#[test]
fn a_corrupt_log_is_not_identical() {
    let d = tmp("replay-corrupt");
    let path = d.join("raw.kflog");
    let mut log = session(&cfg(), None, 6, None);
    let at = log.len() / 2;
    log[at] ^= 0x55;
    fs::write(&path, &log).unwrap();
    let (ok, v, text) = run(&["replay".as_ref(), path.as_os_str()]);
    assert!(!ok, "{text}");
    assert_eq!(v["identical"], json!(false));
    assert!(
        v["error"]
            .as_str()
            .is_some_and(|e| e.contains("reading stopped")),
        "{v}"
    );
}

/// A raw log cut exactly after a commit (a kill between two group writes)
/// reads cleanly but has no session_end: replaying its prefix must not
/// claim the session is identical.
#[test]
fn an_incomplete_log_is_not_identical() {
    use kf_telemetry::GroupReader;
    let d = tmp("replay-incomplete");
    let path = d.join("raw.kflog");
    let log = session(&cfg(), None, 6, None);
    let ends: Vec<u64> = GroupReader::new(&log[..]).unwrap().map(|g| g.end).collect();
    let cut = ends[ends.len() / 2] as usize;
    fs::write(&path, &log[..cut]).unwrap();
    let (ok, v, text) = run(&["replay".as_ref(), path.as_os_str()]);
    assert!(!ok, "{text}");
    assert!(
        v["error"]
            .as_str()
            .is_some_and(|e| e.contains("no terminal session_end")),
        "{v}"
    );
    assert!(v["mismatch"].is_null(), "{v}");
}

#[test]
fn logs_without_a_simulation_are_refused() {
    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../kf-telemetry/tests/golden/full.kflog");
    let (ok, _, text) = run(&["replay".as_ref(), golden.as_os_str()]);
    assert!(!ok);
    assert!(
        text.contains("simulation.rng") || text.contains("scenario"),
        "{text}"
    );
}

/// PERFORMANCE_PLAN N10: headless replay ≥ 100× real time on one core.
/// `cargo test --release -p kf-cli --test replay -- --ignored --nocapture`.
#[test]
#[ignore = "timing: run in release"]
fn n10_replay_speed() {
    let d = tmp("replay-n10");
    let path = d.join("raw.kflog");
    let seconds = 300;
    fs::write(&path, session(&cfg(), None, seconds, None)).unwrap();
    let t = std::time::Instant::now();
    let (ok, v, text) = run(&["replay".as_ref(), path.as_os_str()]);
    let wall = t.elapsed().as_secs_f64();
    assert!(ok, "{text}");
    let speed = seconds as f64 / wall;
    println!(
        "N10: {seconds} s session ({} frames, {} bytes) replayed in {wall:.3} s = {speed:.0}× real time",
        v["frames"],
        fs::metadata(&path).unwrap().len()
    );
    assert!(speed >= 100.0, "{speed:.0}× < 100×");
}
