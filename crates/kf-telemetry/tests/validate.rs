//! Validator and group-reader regressions on small crafted logs.

use kf_core::records::{control_action, control_phase};
use kf_core::{Annotation, Drain, Frame, GapReason, Motion, Nanos, SimControl};
use kf_telemetry::format::{Kind, chunk_crc, write_chunk};
use kf_telemetry::json::{SessionEnd, SessionHeader, from_value, is_uuid};
use kf_telemetry::reader::{ChunkReader, GroupReader};
use kf_telemetry::recover::{Outcome, recover, recovered_stream};
use kf_telemetry::validate::{Issue, Level, MAX_ISSUES, Report, validate};
use kf_telemetry::{FlushPolicy, Record, Recorder};
use serde_json::json;
use std::io::Read;

const FULL: &[u8] = include_bytes!("golden/full.kflog");
const MINIMAL: &[u8] = include_bytes!("golden/minimal.kflog");

/// The golden header (clock domains 0 and 1); `sim` false drops the simulation.
fn header(sim: bool) -> SessionHeader {
    let first = GroupReader::new(FULL).unwrap().next().unwrap();
    let Some(Record::Json { value, .. }) = first.records().next() else {
        panic!("full starts with its header")
    };
    let mut h: SessionHeader = from_value(value).unwrap();
    if !sim {
        h.simulation = None;
        h.sensitivity = None;
    }
    h
}

fn source(id: u16) -> Record {
    Record::Json {
        kind: Kind::SourceDeclare,
        value: json!({"id": id, "backend": "synthetic", "input_level": "synthetic",
            "units": "counts", "value_scale": 1, "clock_domain": 1,
            "receipt_meaning": "app_dequeue", "evidence_class": "simulated_truth"}),
    }
}

fn motion(source: u16, seq: u32) -> Record {
    Record::Motion(Motion {
        t_ns: Nanos(i64::from(seq)),
        src_ts_ns: None,
        dx: 1,
        dy: 0,
        seq,
        source,
        flags: 0,
    })
}

fn gap(source: u16, first_seq: u32, lost: Option<u32>) -> Record {
    Record::Annotation(Annotation {
        t_ns: Nanos(0),
        code: GapReason::QueueOverflow.code(),
        source: Some(source),
        first_seq,
        lost,
        text: String::new(),
    })
}

/// A complete session whose records all fall in one group.
fn session(h: &SessionHeader, records: Vec<Record>) -> Vec<u8> {
    let mut r = Recorder::new(h, FlushPolicy::default()).unwrap();
    for rec in records {
        r.push(rec).unwrap();
    }
    let end: SessionEnd = from_value(&json!({"completed": true, "end_reason": "completed",
        "t_end_ns": 1000, "counts": {}}))
    .unwrap();
    r.finish(end, Nanos(1000)).unwrap()
}

fn messages(r: &Report, level: Level) -> Vec<&str> {
    r.issues
        .iter()
        .filter(|i| i.level == level)
        .map(|i| i.message.as_str())
        .collect()
}

/// Re-seals the CRC of the chunk at `at` after an edit.
fn reseal(log: &mut [u8], at: usize) {
    let len = u32::from_le_bytes(log[at..at + 4].try_into().unwrap()) as usize;
    let crc = chunk_crc(
        log[at..at + 16].try_into().unwrap(),
        &log[at + 16..at + 16 + len],
    );
    log[at + 16 + len..at + 20 + len].copy_from_slice(&crc.to_le_bytes());
}

/// Offsets of a log's CRC-valid chunks.
fn offsets(log: &[u8]) -> Vec<u64> {
    ChunkReader::new(log)
        .unwrap()
        .map(|c| c.unwrap().offset)
        .collect()
}

/// A re-lock from source 0 to source 1 within one group: pause after frame
/// 1's drain, then in frame 2 `switch_source` and `resume` at `phase`, and
/// drains of source 1 in frames 2 and 3.
fn relock(phase: u8) -> Report {
    let frame = |f: u64| {
        Record::Frame(Frame {
            frame: f,
            t_start_ns: Nanos(f as i64 * 10),
            t_input_cutoff_ns: Nanos(f as i64 * 10 + 5),
            first_tick: f,
            ticks: 1,
            flags: 0,
        })
    };
    let drain = |frame, source, last_seq| {
        Record::Drain(Drain {
            frame,
            source,
            last_seq,
        })
    };
    let control = |frame: u64, action, phase, new_source| {
        Record::SimControl(SimControl {
            tick: frame,
            t_ns: Nanos(frame as i64 * 10),
            frame,
            action,
            phase,
            reason: 0,
            new_source,
        })
    };
    let records = vec![
        source(0),
        source(1),
        frame(0),
        motion(0, 0),
        drain(0, 0, 0),
        frame(1),
        control(1, control_action::PAUSE, control_phase::AFTER_DRAIN, None),
        frame(2),
        control(2, control_action::SWITCH_SOURCE, phase, Some(1)),
        control(2, control_action::RESUME, phase, None),
        motion(1, 0),
        drain(2, 1, 0),
        frame(3),
        motion(1, 1),
        drain(3, 1, 1),
    ];
    validate(session(&header(true), records).as_slice())
}

#[test]
fn switch_source_and_a_drain_of_the_new_source_share_a_group() {
    let r = relock(control_phase::BEFORE_DRAIN);
    assert!(
        r.is_valid() && r.complete && r.issues.is_empty(),
        "{:?}",
        r.issues
    );
    assert_eq!(r.groups, 1);
    // Switched after frame 2's drain: that drain is still of source 0's turn.
    let r = relock(control_phase::AFTER_DRAIN);
    assert_eq!(
        messages(&r, Level::Error),
        ["drain of source 1 but the driving source is 0"]
    );
}

#[test]
fn unreadable_session_header_is_an_error() {
    let mut r = Recorder::new(&header(false), FlushPolicy::default()).unwrap();
    r.push(Record::Json {
        kind: Kind::AppResult,
        value: json!({"metric": "accuracy"}),
    })
    .unwrap();
    let mut log = r.cut(Nanos(1));
    // The header chunk at kind_version 2: a v1 reader skips it.
    log[16 + 5] = 2;
    reseal(&mut log, 16);
    // Recovery's synthetic end counts no skipped chunk, so counts agree.
    let Outcome::Recovered(sidecar) = recover(log.as_slice()) else {
        panic!("one group, no session_end")
    };
    let mut stream = Vec::new();
    recovered_stream(log.as_slice(), &sidecar)
        .unwrap()
        .read_to_end(&mut stream)
        .unwrap();
    let r = validate(stream.as_slice());
    assert!(!r.is_valid() && r.header.is_none());
    assert_eq!(
        messages(&r, Level::Error),
        ["session_header unreadable by this reader (kind_version/flags)"]
    );
}

#[test]
fn gap_annotations_cover_only_their_own_run() {
    let mut records = vec![source(0), source(1)];
    // Source 0: an unknown-length gap at 2 covers 2..=4, not the later 7..=99.
    records.extend([0, 1, 5, 6, 100].map(|q| motion(0, q)));
    records.push(gap(0, 2, None));
    // Source 1: 12..=29 is covered by the earlier, longer note (1, +100),
    // not by the nearest one (11, +1); 31..=199 by none (40 starts too late).
    records.extend([0, 11, 30, 200].map(|q| motion(1, q)));
    records.extend([
        gap(1, 1, Some(100)),
        gap(1, 11, Some(1)),
        gap(1, 40, Some(1000)),
    ]);
    let r = validate(session(&header(false), records).as_slice());
    let losses: Vec<_> = r
        .losses
        .iter()
        .map(|l| (l.source, l.first_seq, l.count, l.annotated))
        .collect();
    assert_eq!(
        losses,
        [
            (0, 2, 3, true),
            (0, 7, 93, false),
            (1, 1, 10, true),
            (1, 12, 18, true),
            (1, 31, 169, false),
        ]
    );
    assert!(r.is_valid(), "{:?}", r.issues);
    assert_eq!(
        messages(&r, Level::Warning),
        [
            "source 0: 93 seqs missing from 7 without a gap annotation (capture bug)",
            "source 1: 169 seqs missing from 31 without a gap annotation (capture bug)",
        ]
    );
}

#[test]
fn many_losses_and_gap_notes_stay_bounded() {
    // n losses on source 0 and n gap notes of source 1, matching none.
    let n = 3000u32;
    let mut records = vec![source(0), source(1)];
    records.extend((0..n).map(|i| motion(0, 2 * i)));
    records.extend((0..n).map(|i| gap(1, 2 * i + 1, Some(1))));
    let r = validate(session(&header(false), records).as_slice());
    assert_eq!(r.losses.len(), n as usize - 1);
    assert!(r.losses.iter().all(|l| !l.annotated));
    // One capture-bug warning per loss; past MAX_ISSUES only counted.
    assert_eq!(r.issues.len(), MAX_ISSUES);
    assert_eq!(
        (r.suppressed_errors, r.suppressed_warnings),
        (0, u64::from(n) - 1 - MAX_ISSUES as u64)
    );
    assert!(r.is_valid());
}

/// One group with a motion record from each of `sources` declared
/// sources, then `commits` empty commit chunks (no sources, no watermarks);
/// returns the log and the empty commits' offsets.
fn omitting_log(sources: u16, commits: usize) -> (Vec<u8>, Vec<u64>) {
    let mut r = Recorder::new(&header(false), FlushPolicy::default()).unwrap();
    for s in 0..sources {
        r.push(source(s)).unwrap();
        r.push(motion(s, 0)).unwrap();
    }
    let mut log = r.cut(Nanos(1));
    let mut empty = 1i64.to_le_bytes().to_vec();
    empty.extend_from_slice(&[0xFF; 16]);
    empty.extend_from_slice(&[0; 8]);
    let at = (0..commits)
        .map(|_| {
            let at = log.len() as u64;
            write_chunk(&mut log, Kind::Commit, 1, &empty);
            at
        })
        .collect();
    (log, at)
}

#[test]
fn a_commit_reports_omitted_sources_once() {
    let (log, at) = omitting_log(20, 10);
    let r = validate(log.as_slice());
    let omits: Vec<_> = r
        .issues
        .iter()
        .filter(|i| i.message.starts_with("commit omits"))
        .map(|i| (i.offset, i.message.as_str()))
        .collect();
    let expected: Vec<_> = at
        .iter()
        .map(|&o| (Some(o), "commit omits 20 sources"))
        .collect();
    assert_eq!(omits, expected);
}

#[test]
fn issues_are_capped() {
    let (log, _) = omitting_log(2, 1500);
    let r = validate(log.as_slice());
    // 1500 omissions and the missing session_end.
    assert_eq!(r.issues.len(), MAX_ISSUES);
    assert_eq!((r.suppressed_errors, r.suppressed_warnings), (501, 0));
    assert!(!r.is_valid());
    let suppressed = Report {
        suppressed_errors: 1,
        ..Report::default()
    };
    assert!(!suppressed.is_valid());
}

#[test]
fn commit_issues_name_the_commit_chunk() {
    let mut log = session(&header(false), vec![source(3), motion(3, 0)]);
    let commit = ChunkReader::new(log.as_slice())
        .unwrap()
        .map(Result::unwrap)
        .find(|c| c.header.kind == Kind::Commit as u8)
        .unwrap()
        .offset as usize;
    // The first source entry's last_seq (payload bytes 36..40): 0 → 9.
    log[commit + 16 + 36] = 9;
    reseal(&mut log, commit);
    assert_eq!(
        validate(log.as_slice()).issues,
        [Issue {
            level: Level::Error,
            offset: Some(commit as u64),
            message: "commit: source 3 last_seq 9 but Some(0) written".into(),
        }]
    );
}

#[test]
fn session_id_and_started_at_utc_are_checked() {
    let check = |id: &str, at: Option<&str>| {
        let mut h = header(false);
        h.session_id = id.into();
        h.started_at_utc = at.map(Into::into);
        let r = validate(session(&h, Vec::new()).as_slice());
        r.issues.into_iter().map(|i| i.message).collect::<Vec<_>>()
    };
    let id = "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11";
    for ok in [
        None,
        Some("2026-11-14T21:04:05Z"),
        Some("2024-02-29T23:59:60.123456789Z"),
        Some("2000-02-29T00:00:00.0Z"),
    ] {
        assert!(check(id, ok).is_empty(), "{ok:?}");
    }
    for bad in [
        "",
        "yesterday",
        "2026-11-14T21:04:05",
        "2026-11-14 21:04:05Z",
        "2026-11-14t21:04:05z",
        "2026-11-14T21:04:05+00:00",
        "2026-11-14T21:04:05.Z",
        "2026-11-14T21:04:5Z",
        "+026-11-14T21:04:05Z",
        "2026-02-29T00:00:00Z",
        "1900-02-29T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "2026-11-31T00:00:00Z",
        "2026-11-00T00:00:00Z",
        "2026-11-14T24:00:00Z",
        "2026-11-14T21:60:00Z",
        "2026-11-14T21:04:61Z",
    ] {
        assert_eq!(
            check(id, Some(bad)),
            [format!("started_at_utc {bad:?} is not RFC 3339 UTC")]
        );
    }
    assert_eq!(
        check("session-42", None),
        [r#"session_id "session-42" is not a UUID"#]
    );
    // Any version and case: kf-store accepts these as directory names.
    assert!(check("6F1C2B9E-4D7A-1F3E-0C21-0A5B7D3E8F11", None).is_empty());
    assert!(!is_uuid("../../../../etc/passwd-0000-0000-0000"));
    assert!(!is_uuid("6f1c2b9e04d7a-4f3e-9c21-0a5b7d3e8f11"));
}

#[test]
fn uncommitted_and_after_end_tails_are_offsets() {
    // Header, source_declare, input_motion, commit, session_end.
    let at = offsets(MINIMAL);
    assert_eq!(at.len(), 5);
    // Without its commit every chunk is uncommitted, session_end included.
    let log = [&MINIMAL[..at[3] as usize], &MINIMAL[at[4] as usize..]].concat();
    let (e, _) = GroupReader::new(log.as_slice()).unwrap().ending();
    assert!(e.session_end.is_none() && e.after_end.is_empty());
    assert_eq!(e.uncommitted, at[..4]);
    let r = validate(log.as_slice());
    let warned: Vec<_> = r
        .issues
        .iter()
        .filter(|i| i.level == Level::Warning)
        .map(|i| i.offset.unwrap())
        .collect();
    assert_eq!(warned, at[..4]);
    // A chunk after the terminal session_end.
    let log = [MINIMAL, &MINIMAL[at[2] as usize..at[3] as usize]].concat();
    let (e, _) = GroupReader::new(log.as_slice()).unwrap().ending();
    assert!(e.session_end.is_some() && e.uncommitted.is_empty());
    assert_eq!(e.after_end, [MINIMAL.len() as u64]);
    assert_eq!(
        validate(log.as_slice()).issues,
        [Issue {
            level: Level::Error,
            offset: Some(MINIMAL.len() as u64),
            message: "chunk after the terminal session_end".into(),
        }]
    );
}
