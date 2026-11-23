//! Deterministic example logs. The committed copies under
//! `crates/kf-telemetry/tests/golden/` must equal these bytes on every
//! target (tests/golden.rs), and the stdlib Python reader must read them as
//! the Rust reader does (`ci/python-parity.sh`). `kf-cli golden <dir>`
//! rewrites the committed copies after a deliberate change.

use crate::format::{Kind, write_chunk};
use crate::json::{
    ClockDomain, Sensitivity, SessionEnd, SessionHeader, Simulation, SourceDeclaration, end_reason,
    to_value,
};
use crate::record::Record;
use crate::recorder::{FlushPolicy, Recorder};
use kf_core::records::{
    annotation_code, control_action, control_phase, control_reason, target_event, target_shape,
};
use kf_core::{
    Annotation, Button, ClockKind, ClockMeaning, ClockSync, Degrees, Drain, EvidenceClass, Frame,
    FrameTiming, GapReason, InputLevel, LinearSensitivity, Motion, Nanos, ReceiptMeaning,
    SimCamera, SimControl, SimShot, SimTarget, Units, ValueScale, Wheel,
};
use serde_json::json;
use std::collections::BTreeMap;

const SESSION_ID: &str = "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11";

fn header(simulation: bool) -> SessionHeader {
    let k = Degrees(0.022).to_radians().0;
    SessionHeader {
        schema: crate::json::SCHEMA.into(),
        session_id: SESSION_ID.into(),
        started_at_utc: Some("2026-11-14T21:04:05Z".into()),
        clock_domains: vec![
            ClockDomain {
                id: 0,
                kind: ClockKind::Session,
                coi: None,
                meaning: ClockMeaning::SessionClock,
                resolution_ns: 1,
                offset_to_session_ns: 0,
                offset_uncertainty_ns: 0,
                drift_ppm: 0.0,
            },
            ClockDomain {
                id: 1,
                kind: ClockKind::LinuxMonotonic,
                coi: None,
                meaning: ClockMeaning::KernelFlush,
                resolution_ns: 1000,
                offset_to_session_ns: -1_834_512_000,
                offset_uncertainty_ns: 5000,
                drift_ppm: 0.25,
            },
        ],
        simulation: simulation.then(|| Simulation {
            sim_hz: 1000,
            input_source: 0,
            rest: [(
                "rng".to_owned(),
                json!({"algorithm": "xoshiro256++", "seed": "0x5eed0001"}),
            )]
            .into(),
        }),
        sensitivity: simulation.then(|| Sensitivity {
            model: "linear".into(),
            yaw_rad_per_unit: k,
            pitch_rad_per_unit: k,
            sens_x: 1.25,
            sens_y: 1.25,
            invert_y: false,
            rest: [(
                "fov".to_owned(),
                json!({"value_deg": 103.0, "model": "hor_plus", "basis_aspect": [16, 9]}),
            )]
            .into(),
        }),
        rest: [
            (
                "app".to_owned(),
                json!({"name": "kineforge-golden", "version": "0.0.0"}),
            ),
            ("source_app".to_owned(), serde_json::Value::Null),
        ]
        .into(),
    }
}

fn source(id: u16, units: Units, value_scale: u16) -> Record {
    Record::Json {
        kind: Kind::SourceDeclare,
        value: to_value(&SourceDeclaration {
            id,
            source_rank: None,
            backend: "synthetic".into(),
            input_level: InputLevel::Synthetic,
            units,
            value_scale,
            clock_domain: 1,
            receipt_meaning: ReceiptMeaning::AppDequeue,
            evidence_class: EvidenceClass::SimulatedTruth,
            processing: vec!["injected_test_stream".into()],
            rest: [("kind".to_owned(), json!("mouse"))].into(),
        }),
    }
}

fn end(reason: &str) -> SessionEnd {
    SessionEnd {
        completed: reason == end_reason::COMPLETED,
        end_reason: reason.into(),
        t_end_ns: 0,
        counts: BTreeMap::new(),
        dropped_chunks: 0,
        max_writer_backlog_bytes: 4096,
        interruptions: 1,
        refresh_hz_measured: Some(164.9997),
        raf_hz_measured: None,
        rest: BTreeMap::new(),
    }
}

fn push(r: &mut Recorder, rec: Record) {
    r.push(rec).expect("golden records encode");
}

/// Inserts a raw chunk just before a group's trailing commit chunk.
fn insert_before_commit(group: &mut Vec<u8>, chunk: &[u8], commit_len: usize) {
    let at = group.len() - commit_len;
    group.splice(at..at, chunk.iter().copied());
}

/// Every kind, two sources, three groups, a seq gap with its annotation, a
/// seq gap without one, and two chunks a v1 reader skips.
pub fn full() -> Vec<u8> {
    let lin = LinearSensitivity {
        yaw_rad_per_unit: Degrees(0.022).to_radians().0,
        pitch_rad_per_unit: Degrees(0.022).to_radians().0,
        sens_x: 1.25,
        sens_y: 1.25,
        invert_y: false,
    };
    let mut r = Recorder::new(&header(true), FlushPolicy::default()).expect("header encodes");
    let mut out = Vec::new();
    push(&mut r, source(0, Units::Counts, 1));
    push(&mut r, source(1, Units::Dip, 1024));
    push(
        &mut r,
        Record::Json {
            kind: Kind::DeviceInfo,
            value: json!({"vid": null, "cpi_declared": 1600, "report_rate_hz": 1000}),
        },
    );
    push(
        &mut r,
        Record::Json {
            kind: Kind::RenderState,
            value: json!({"surface_px": [2560, 1440], "dpr": 1.0, "crosshair_offset_px": [0.5, 0.0], "present_mode": "fifo", "frame_cap_hz": null}),
        },
    );
    let (mut yaw, mut pitch) = (0i64, 0i64);
    let mut tick = 0u64;
    // One seq counter per source, shared by motion, button and wheel.
    let (mut seq0, mut seq1) = (0u32, 0u32);
    let next = |c: &mut u32| {
        let s = *c;
        *c += 1;
        s
    };
    let mut button_seq = 0;
    for f in 0..6u64 {
        let t0 = 1_000_000 + f as i64 * 6_060_606;
        for i in 0..4i64 {
            let (dx, dy) = ((i as i32 * 7) - 9, 3 - i as i32);
            push(
                &mut r,
                Record::Motion(Motion {
                    t_ns: Nanos(t0 + i * 1000),
                    src_ts_ns: Some(Nanos(t0 + i * 1000 - 1_834_512_000 - 40_000)),
                    dx,
                    dy,
                    seq: next(&mut seq0),
                    source: 0,
                    flags: 0,
                }),
            );
            yaw += i64::from(dx);
            pitch += i64::from(dy);
        }
        if f == 1 {
            button_seq = next(&mut seq0);
            push(
                &mut r,
                Record::Button(Button {
                    t_ns: Nanos(t0 + 3500),
                    src_ts_ns: None,
                    seq: button_seq,
                    source: 0,
                    button: kf_core::input::button::LEFT,
                    state: kf_core::input::button_state::DOWN,
                }),
            );
        }
        if f == 2 {
            push(
                &mut r,
                Record::Wheel(Wheel {
                    t_ns: Nanos(t0 + 3600),
                    src_ts_ns: Some(Nanos(1)),
                    dv: -120,
                    dh: 0,
                    seq: next(&mut seq0),
                    source: 0,
                    flags: kf_core::input::event_flags::HI_RES,
                }),
            );
        }
        push(
            &mut r,
            Record::Motion(Motion {
                t_ns: Nanos(t0 + 500),
                src_ts_ns: None,
                dx: -1536,
                dy: 2048,
                seq: next(&mut seq1),
                source: 1,
                flags: kf_core::input::event_flags::COALESCED,
            }),
        );
        let ticks = 6;
        push(
            &mut r,
            Record::Frame(Frame {
                frame: f,
                t_start_ns: Nanos(t0),
                t_input_cutoff_ns: Nanos(t0 + 4000),
                first_tick: tick,
                ticks,
                flags: 0,
            }),
        );
        push(
            &mut r,
            Record::Drain(Drain {
                frame: f,
                source: 0,
                last_seq: seq0 - 1,
            }),
        );
        tick += u64::from(ticks);
        push(
            &mut r,
            Record::SimCamera(SimCamera {
                tick: tick - 1,
                yaw_units: yaw,
                pitch_units: pitch,
                yaw_rad: lin.yaw(yaw, ValueScale::X1).0,
                pitch_rad: lin.pitch(pitch, ValueScale::X1).0,
            }),
        );
        push(
            &mut r,
            Record::FrameTiming(FrameTiming {
                frame: f,
                t_submit_ns: Nanos(t0 + 900_000),
                t_present_ns: (f % 2 == 0).then_some(Nanos(t0 + 5_000_000)),
                cpu_ns: 850_000,
                gpu_ns: (f != 3).then_some(410_000),
            }),
        );
        if f == 1 {
            push(
                &mut r,
                Record::SimTarget(SimTarget {
                    tick: tick - 1,
                    target: 7,
                    event: target_event::SPAWN,
                    shape: target_shape::SPHERE,
                    pos_m: [0.25, 1.5, -10.0],
                    vel_m_s: [0.0, 0.0, 0.0],
                    radius_m: 0.3,
                }),
            );
            push(
                &mut r,
                Record::SimShot(SimShot {
                    tick: tick - 1,
                    t_ns: Nanos(t0 + 3500),
                    target: Some(7),
                    seq: button_seq,
                    ang_err_rad: 0.0123,
                    d_yaw_rad: -0.01,
                    d_pitch_rad: 0.007,
                    hit: true,
                }),
            );
        }
        if f == 2 {
            push(
                &mut r,
                Record::SimShot(SimShot {
                    tick: tick - 1,
                    t_ns: Nanos(t0 + 10),
                    target: None,
                    seq: button_seq,
                    ang_err_rad: 0.5,
                    d_yaw_rad: 0.25,
                    d_pitch_rad: -1e-7,
                    hit: false,
                }),
            );
            push(
                &mut r,
                Record::ClockSync(ClockSync {
                    t_ns: Nanos(t0),
                    domain: 1,
                    src_ts_ns: Nanos(t0 - 1_834_512_000),
                    uncertainty_ns: 5000,
                }),
            );
            push(
                &mut r,
                Record::Json {
                    kind: Kind::AppResult,
                    value: json!({"metric": "accuracy", "value": 0.875, "unit": "ratio", "method": "manual_transcription", "transcribed_by": "golden"}),
                },
            );
        }
        if f == 3 {
            push(
                &mut r,
                Record::SimControl(SimControl {
                    tick,
                    t_ns: Nanos(t0 + 5000),
                    frame: f,
                    action: control_action::PAUSE,
                    phase: control_phase::AFTER_DRAIN,
                    reason: control_reason::FOCUS_LOST,
                    new_source: None,
                }),
            );
            push(
                &mut r,
                Record::Annotation(Annotation {
                    t_ns: Nanos(t0 + 5000),
                    code: GapReason::FocusLost.code(),
                    source: None,
                    first_seq: 0,
                    lost: None,
                    text: String::new(),
                }),
            );
            push(
                &mut r,
                Record::SimControl(SimControl {
                    tick,
                    t_ns: Nanos(t0 + 6000),
                    frame: f,
                    action: control_action::RESUME,
                    phase: control_phase::AFTER_DRAIN,
                    reason: control_reason::USER,
                    new_source: None,
                }),
            );
            push(
                &mut r,
                Record::Annotation(Annotation {
                    t_ns: Nanos(t0 + 6000),
                    code: annotation_code::USER_MARKER,
                    source: Some(0),
                    first_seq: 13,
                    lost: Some(0),
                    text: "marker — ünïcødé".into(),
                }),
            );
        }
        // Cuts after frames 1, 3 and 4; the last group comes from finish().
        if f == 1 || f == 3 {
            out.extend_from_slice(&r.cut(Nanos(t0 + 6_000_000)));
        }
        if f == 4 {
            // Frame 4's group holds kinds 0x01, 0x10–0x12 and 0x20 only.
            // Append a known kind at a future layout version and a future
            // kind (0x40), in ascending order before the commit: a v1
            // reader skips both.
            let mut group = r.cut(Nanos(t0 + 6_000_000));
            let mut future = Vec::new();
            write_chunk(&mut future, Kind::SimCamera, 0, &[]);
            future[5] = 2;
            let crc = crate::format::chunk_crc(future[..16].try_into().unwrap(), &[]);
            future[16..20].copy_from_slice(&crc.to_le_bytes());
            let mut unknown = Vec::new();
            write_chunk(&mut unknown, Kind::Drain, 1, &[0xAB; 16]);
            unknown[4] = 0x40;
            let crc = crate::format::chunk_crc(unknown[..16].try_into().unwrap(), &[0xAB; 16]);
            unknown[32..36].copy_from_slice(&crc.to_le_bytes());
            let commit_len = 24 + 32 + 8 * 2;
            future.extend_from_slice(&unknown);
            insert_before_commit(&mut group, &future, commit_len);
            out.extend_from_slice(&group);
        }
    }
    // A seq gap on source 0 (two missing) with a gap annotation, and one on
    // source 1 (one missing) without: a capture-bug warning.
    let lost_from = next(&mut seq0);
    next(&mut seq0);
    push(
        &mut r,
        Record::Annotation(Annotation {
            t_ns: Nanos(40_000_000),
            code: GapReason::QueueOverflow.code(),
            source: Some(0),
            first_seq: lost_from,
            lost: Some(2),
            text: "ring full".into(),
        }),
    );
    push(
        &mut r,
        Record::Motion(Motion {
            t_ns: Nanos(40_000_100),
            src_ts_ns: None,
            dx: 1,
            dy: 0,
            seq: next(&mut seq0),
            source: 0,
            flags: 0,
        }),
    );
    next(&mut seq1);
    push(
        &mut r,
        Record::Motion(Motion {
            t_ns: Nanos(40_000_200),
            src_ts_ns: None,
            dx: -2048,
            dy: 0,
            seq: next(&mut seq1),
            source: 1,
            flags: 0,
        }),
    );
    let mut e = end(end_reason::COMPLETED);
    e.t_end_ns = 41_000_000;
    out.extend_from_slice(&r.finish(e, Nanos(41_000_000)).expect("end encodes"));
    out
}

/// No simulation (a third-party or analyzer session): header, one source,
/// a few events, the end.
pub fn minimal() -> Vec<u8> {
    let mut r = Recorder::new(&header(false), FlushPolicy::default()).expect("header encodes");
    push(&mut r, source(3, Units::Counts, 1));
    for seq in 0..3u32 {
        push(
            &mut r,
            Record::Motion(Motion {
                t_ns: Nanos(i64::from(seq) * 125_000),
                src_ts_ns: None,
                dx: 1 << seq,
                dy: -(1 << seq),
                seq,
                source: 3,
                flags: 0,
            }),
        );
    }
    let mut e = end(end_reason::COMPLETED);
    e.t_end_ns = 500_000;
    r.finish(e, Nanos(500_000)).expect("end encodes")
}

/// [`full`] cut off inside its third group, as after a crash: two committed
/// groups, then a torn chunk. Recovery keeps the two groups.
pub fn torn() -> Vec<u8> {
    let full = full();
    // The second commit ends the second group; keep that plus part of the next chunk.
    let groups: Vec<u64> = crate::reader::GroupReader::new(full.as_slice())
        .expect("full reads")
        .map(|g| g.end)
        .collect();
    let keep = groups[1] as usize + 37;
    full[..keep].to_vec()
}

/// [`minimal`] with one flipped payload byte in its only data chunk: the
/// CRC fails, reading stops, and no group is committed.
pub fn corrupt() -> Vec<u8> {
    let mut m = minimal();
    // File header (16) + header chunk; flip a byte inside the next chunk's payload.
    let header_len = 24 + u32::from_le_bytes(m[16..20].try_into().unwrap()) as usize;
    let at = 16 + header_len + 16 + 3;
    m[at] ^= 0x01;
    m
}

/// Name and bytes of every golden log.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("full", full()),
        ("minimal", minimal()),
        ("torn", torn()),
        ("corrupt", corrupt()),
    ]
}
