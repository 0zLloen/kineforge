//! kf-store: a session through `Recorder` + `SessionWriter`, the liveness
//! lock, recovery sidecars, storage-error fault injection, the memory cap,
//! and a SIMULATED power-loss model for PERFORMANCE_PLAN N9c.

use kf_core::{
    ClockKind, ClockMeaning, EvidenceClass, Frame, InputLevel, Motion, Nanos, ReceiptMeaning, Units,
};
use kf_store::{
    MANIFEST, RAW, RESERVE, Recovery, SIDECAR, SessionWriter, WriterOptions, create_session_dir,
    identity_sha256, new_session_id, recover_session, session_stream, utc_now_rfc3339,
    write_manifest,
};
use kf_telemetry::json::{
    ClockDomain, SCHEMA, SessionEnd, SessionHeader, SourceDeclaration, end_reason, to_value,
};
use kf_telemetry::manifest::{Manifest, sha256_hex};
use kf_telemetry::recover::{Outcome, Sidecar, recover};
use kf_telemetry::validate::validate;
use kf_telemetry::{FlushPolicy, GroupReader, Kind, Record, Recorder, golden};
use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const MS: i64 = 1_000_000;

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("kf-store-{name}-{}", new_session_id()));
    fs::create_dir_all(&p).unwrap();
    p
}

fn header(id: &str) -> SessionHeader {
    SessionHeader {
        schema: SCHEMA.into(),
        session_id: id.into(),
        started_at_utc: Some(utc_now_rfc3339()),
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
        simulation: None,
        sensitivity: None,
        rest: BTreeMap::new(),
    }
}

fn source(id: u16) -> Record {
    Record::Json {
        kind: Kind::SourceDeclare,
        value: to_value(&SourceDeclaration {
            id,
            source_rank: None,
            backend: "synthetic".into(),
            input_level: InputLevel::Synthetic,
            units: Units::Counts,
            value_scale: 1,
            clock_domain: 0,
            receipt_meaning: ReceiptMeaning::AppDequeue,
            evidence_class: EvidenceClass::SimulatedTruth,
            processing: vec![],
            rest: BTreeMap::new(),
        }),
    }
}

fn motion(t_ns: i64, seq: u32, source: u16) -> Record {
    Record::Motion(Motion {
        t_ns: Nanos(t_ns),
        src_ts_ns: None,
        dx: 1,
        dy: -1,
        seq,
        source,
        flags: 0,
    })
}

fn end(reason: &str, t_end_ns: i64) -> SessionEnd {
    SessionEnd {
        completed: reason == end_reason::COMPLETED,
        end_reason: reason.into(),
        t_end_ns,
        counts: BTreeMap::new(),
        dropped_chunks: 0,
        max_writer_backlog_bytes: 0,
        interruptions: 0,
        refresh_hz_measured: None,
        raf_hz_measured: None,
        rest: BTreeMap::new(),
    }
}

fn sha(b: &[u8]) -> String {
    sha256_hex(b).unwrap()
}

/// A new bundle with a live writer and a recorder with source 0 declared.
fn start(
    root: &Path,
    opts: WriterOptions,
    policy: FlushPolicy,
) -> (PathBuf, SessionWriter, Recorder, String) {
    let id = new_session_id();
    let dir = create_session_dir(root, &id).unwrap();
    assert!(dir.ends_with(format!("sessions/{id}")));
    let w = SessionWriter::create(&dir, opts).unwrap();
    let reserve = fs::read(dir.join(RESERVE)).unwrap();
    assert!(reserve.len() >= 64 << 10);
    // Not zeros, which compressing filesystems store as holes.
    assert!(reserve.chunks(4096).all(|b| b.iter().any(|&x| x != 0)));
    let mut rec = Recorder::new(&header(&id), policy).unwrap();
    rec.push(source(0)).unwrap();
    (dir, w, rec, id)
}

#[test]
fn session_validates_and_manifest_is_right() {
    let root = tmp("session");
    let (dir, w, mut rec, id) = start(&root, WriterOptions::default(), FlushPolicy::default());
    rec.push(Record::Json {
        kind: Kind::DeviceInfo,
        value: serde_json::json!({"cpi_declared": 1600}),
    })
    .unwrap();
    let mut seq = 0;
    for ms in 0..300 {
        for i in 0..8 {
            rec.push(motion(ms * MS + i * 125_000, seq, 0)).unwrap();
            seq += 1;
        }
        let now = Nanos((ms + 1) * MS);
        if rec.due(now) {
            assert!(w.submit(rec.cut(now)));
        }
    }
    let t = 300 * MS;
    // The recorder thread fills in what only the writer knows.
    let mut e = end(end_reason::COMPLETED, t);
    e.max_writer_backlog_bytes = w.drain().max_backlog_bytes;
    let st = w.close(rec.finish(e, Nanos(t)).unwrap()).unwrap();
    let raw = fs::read(dir.join(RAW)).unwrap();
    let len = raw.len() as u64;
    assert_eq!((st.written_bytes, st.durable_bytes), (len, len));
    assert_eq!((st.dropped_groups, st.storage_error), (0, false));
    assert!(st.max_backlog_bytes > 0);
    assert!(!dir.join(RESERVE).exists());

    let report = validate(raw.as_slice());
    assert!(
        report.issues.is_empty() && report.complete,
        "{:?}",
        report.issues
    );
    assert_eq!(report.counts["input_motion"], 2400);
    let e = report.session_end.unwrap();
    assert_eq!(e.end_reason, "completed");
    assert_eq!(e.dropped_chunks, 0);
    assert_eq!(e.max_writer_backlog_bytes, st.max_backlog_bytes);

    let m = write_manifest(&dir).unwrap();
    assert_eq!(m.schema, "kineforge.manifest/0.1");
    assert_eq!(m.header["session_id"], id.as_str());
    assert_eq!((m.sources.len(), m.device_info.len()), (1, 1));
    assert!(!m.recovered);
    assert_eq!(m.session_end.as_ref().unwrap()["end_reason"], "completed");
    assert_eq!(m.session_identity_sha256, sha(&raw));
    assert_eq!(identity_sha256(&dir).unwrap(), sha(&raw));
    assert_eq!(m.files, BTreeMap::from([(RAW.to_owned(), sha(&raw))]));
    let on_disk: Manifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(on_disk, m);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn liveness_lock() {
    let root = tmp("lock");
    let (dir, w, mut rec, _) = start(&root, WriterOptions::default(), FlushPolicy::default());
    rec.push(motion(0, 0, 0)).unwrap();
    assert!(w.submit(rec.cut(Nanos(MS))));

    let second = SessionWriter::create(&dir, WriterOptions::default());
    assert_eq!(second.err().unwrap().kind(), ErrorKind::AlreadyExists);
    // Another handle cannot take the lock either.
    let other = File::open(dir.join(RAW)).unwrap();
    assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
    drop(other);
    assert_eq!(
        recover_session(&dir).unwrap_err().kind(),
        ErrorKind::ResourceBusy
    );

    let t = 2 * MS;
    w.close(rec.finish(end(end_reason::COMPLETED, t), Nanos(t)).unwrap())
        .unwrap();
    let raw = fs::read(dir.join(RAW)).unwrap();
    match recover_session(&dir).unwrap() {
        Recovery::Complete {
            sha256,
            stale_sidecar: false,
        } => assert_eq!(sha256, sha(&raw)),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join(SIDECAR).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovery_sidecar() {
    let root = tmp("recovery");
    let dir = create_session_dir(&root, &new_session_id()).unwrap();
    let torn = golden::torn();
    fs::write(dir.join(RAW), &torn).unwrap();
    // A crashed writer's reserve.
    fs::write(dir.join(RESERVE), [1; 64]).unwrap();
    // A concurrent recovery (a shared lock) does not block this one.
    let other = File::open(dir.join(RAW)).unwrap();
    other.try_lock_shared().unwrap();

    let Recovery::Recovered(s) = recover_session(&dir).unwrap() else {
        panic!("torn recovers")
    };
    drop(other);
    assert!(!dir.join(RESERVE).exists());
    assert_eq!(
        fs::read(dir.join(RAW)).unwrap(),
        torn,
        "raw.kflog untouched"
    );
    let on_disk: Sidecar = serde_json::from_slice(&fs::read(dir.join(SIDECAR)).unwrap()).unwrap();
    assert_eq!(on_disk, s);
    assert_eq!(identity_sha256(&dir).unwrap(), s.recovered_sha256);
    let mut stream = Vec::new();
    session_stream(&dir)
        .unwrap()
        .read_to_end(&mut stream)
        .unwrap();
    let report = validate(stream.as_slice());
    assert!(report.is_valid() && report.complete, "{:?}", report.issues);
    assert_eq!(report.session_end.unwrap().end_reason, "recovered");
    let m = write_manifest(&dir).unwrap();
    assert!(m.recovered);
    assert_eq!(m.session_identity_sha256, s.recovered_sha256);
    assert_eq!(
        m.files.keys().collect::<Vec<_>>(),
        [RAW, SIDECAR].map(String::from).iter().collect::<Vec<_>>()
    );

    // A sidecar that recovery does not reproduce is refused.
    let wrong = Sidecar {
        tail_bytes_ignored: 1,
        ..s.clone()
    };
    fs::write(dir.join(SIDECAR), serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert_eq!(
        session_stream(&dir).err().unwrap().kind(),
        ErrorKind::InvalidData
    );

    // raw.kflog complete after all: the sidecar is ignored, reported and kept.
    let full = golden::full();
    fs::write(dir.join(RAW), &full).unwrap();
    assert!(matches!(
        recover_session(&dir).unwrap(),
        Recovery::Complete {
            stale_sidecar: true,
            ..
        }
    ));
    assert!(dir.join(SIDECAR).exists());
    assert_eq!(identity_sha256(&dir).unwrap(), sha(&full));

    fs::write(dir.join(RAW), b"not a log").unwrap();
    assert!(matches!(
        recover_session(&dir).unwrap(),
        Recovery::Unrecoverable(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

/// The recorder thread's part after any writer drop: drain, roll the
/// recorder back to the file, end with `storage_error`.
fn end_after_drop(w: SessionWriter, mut rec: Recorder, t: i64) -> kf_store::Status {
    let st = w.drain();
    rec.writer_lost(st.written_bytes, Nanos(t)).unwrap();
    let mut e = end(end_reason::STORAGE_ERROR, t);
    e.max_writer_backlog_bytes = st.max_backlog_bytes;
    w.close(rec.finish(e, Nanos(t)).unwrap()).unwrap()
}

#[test]
fn storage_error_mid_session() {
    let root = tmp("enospc");
    let opts = WriterOptions {
        fail_after_bytes: Some(40_000),
        ..WriterOptions::default()
    };
    let (dir, w, mut rec, _) = start(&root, opts, FlushPolicy::default());
    let (mut seq, mut ms) = (0u32, 0i64);
    loop {
        for i in 0..8 {
            rec.push(motion(ms * MS + i * 125_000, seq, 0)).unwrap();
            seq += 1;
        }
        ms += 1;
        let now = Nanos(ms * MS);
        if rec.due(now) {
            let ok = w.submit(rec.cut(now));
            thread::sleep(Duration::from_millis(1));
            if !ok {
                break;
            }
        }
        let st = w.status();
        // The protocol step that bounds the recorder's memory; writer_lost
        // must still find the boundary it drains to.
        rec.written(st.written_bytes);
        if st.storage_error {
            break;
        }
        assert!(ms < 1_000_000, "no storage error");
    }
    // The reserve is kept until close writes the final bytes, and every
    // later group is refused.
    assert!(w.status().storage_error && dir.join(RESERVE).exists());
    assert!(!w.submit(vec![0; 8]));
    let st = end_after_drop(w, rec, ms * MS);
    assert!(st.storage_error);
    assert!(!dir.join(RESERVE).exists());

    let raw = fs::read(dir.join(RAW)).unwrap();
    assert_eq!(st.written_bytes, raw.len() as u64);
    // No torn group: whole groups, then the final group and session_end.
    let (ending, _) = GroupReader::new(raw.as_slice()).unwrap().ending();
    assert!(ending.error.is_none() && ending.uncommitted.is_empty() && ending.after_end.is_empty());
    let report = validate(raw.as_slice());
    assert!(report.is_valid() && report.complete, "{:?}", report.issues);
    let e = report.session_end.unwrap();
    assert_eq!(
        (e.completed, e.end_reason.as_str()),
        (false, "storage_error")
    );
    assert!(e.dropped_chunks >= 2, "the torn group's chunks");
    assert_eq!(
        report.counts["annotation"], 2,
        "a storage_error gap for source 0 and one without a source"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn final_write_error_fails_close() {
    let root = tmp("final");
    let opts = WriterOptions {
        fail_after_bytes: Some(1),
        ..WriterOptions::default()
    };
    let (dir, w, rec, _) = start(&root, opts, FlushPolicy::default());
    let e = w
        .close(
            rec.finish(end(end_reason::COMPLETED, MS), Nanos(MS))
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::StorageFull);
    assert!(!dir.join(RESERVE).exists());
    assert_eq!(fs::read(dir.join(RAW)).unwrap(), b"", "torn bytes cut off");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn memory_cap_drop_ends_the_session() {
    let root = tmp("cap");
    let opts = WriterOptions {
        memory_cap_bytes: 4096,
        ..WriterOptions::default()
    };
    let (dir, w, mut rec, _) = start(&root, opts, FlushPolicy::default());
    rec.push(motion(0, 0, 0)).unwrap();
    assert!(w.submit(rec.cut(Nanos(MS))));
    // Larger than the cap: dropped, and the session ends.
    for i in 0..300 {
        rec.push(motion(2 * MS + i, 1 + i as u32, 0)).unwrap();
    }
    let big = rec.cut(Nanos(3 * MS));
    let big_len = big.len() as u64;
    assert!(big_len > 4096 && !w.submit(big));
    let st = end_after_drop(w, rec, 4 * MS);
    assert_eq!(
        (st.dropped_groups, st.dropped_bytes, st.storage_error),
        (1, big_len, false)
    );
    let raw = fs::read(dir.join(RAW)).unwrap();
    let report = validate(raw.as_slice());
    assert!(report.is_valid() && report.complete, "{:?}", report.issues);
    assert_eq!(report.counts["input_motion"], 1);
    let e = report.session_end.unwrap();
    assert_eq!(
        (e.end_reason.as_str(), e.dropped_chunks),
        ("storage_error", 2)
    );
    fs::remove_dir_all(root).unwrap();
}

/// A SIMULATED power-loss model, not a real power cut (N9c proper needs a
/// VM hard reset or dm-log-writes replay on H1, PERFORMANCE_PLAN §4.6).
/// The model: after power loss the file holds the bytes covered by the last
/// completed `sync_data` (`durable_bytes`) plus an arbitrary tail — zeros,
/// garbage, or part of the written but unsynced bytes. The log is append-only,
/// so the durable prefix at any instant is a prefix of the closed file.
/// `loss <= bound` holds by construction (bound is built from the observed
/// lags); what this really checks is the sync lag against N9c and recovery
/// of crash images with zero, garbage and partial tails.
#[test]
fn power_loss_model() {
    const FSYNC_MS: u64 = 1000;
    /// Thread wake-up and 1 ms sampling on loaded CI runners. The sync
    /// durations themselves (F_FULLFSYNC on macOS, FlushFileBuffers on
    /// Windows, slow shared disks) come from the writer's `max_sync_ns`.
    const SCHED_ALLOWANCE_MS: i64 = 100;
    const FLUSH_MS: i64 = 100;
    const RUN_MS: i64 = 4000;
    struct Sample {
        wall: i64,
        generated: i64,
        written: u64,
        durable: u64,
    }
    let root = tmp("power");
    let opts = WriterOptions {
        fsync_ms: FSYNC_MS,
        ..WriterOptions::default()
    };
    let policy = FlushPolicy {
        chunk_flush_ns: FLUSH_MS * MS,
        max_records: 4096,
    };
    let (dir, w, mut rec, _) = start(&root, opts, policy);
    let mut samples = Vec::new();
    // (cut time, end offset) of every group.
    let mut groups: Vec<(i64, u64)> = Vec::new();
    let (mut seq, mut off) = (0u32, 0u64);
    let t0 = Instant::now();
    for ms in 0..RUN_MS {
        // The batch for [ms, ms + 1) arrives at ms + 1: 8 kHz input, a 1 kHz frame.
        let now = (ms + 1) * MS;
        if let Some(d) = Duration::from_nanos(now as u64).checked_sub(t0.elapsed()) {
            thread::sleep(d);
        }
        for i in 0..8 {
            rec.push(motion(ms * MS + i * 125_000, seq, 0)).unwrap();
            seq += 1;
        }
        rec.push(Record::Frame(Frame {
            frame: ms as u64,
            t_start_ns: Nanos(ms * MS),
            t_input_cutoff_ns: Nanos(ms * MS + 875_000),
            first_tick: ms as u64,
            ticks: 1,
            flags: 0,
        }))
        .unwrap();
        if rec.due(Nanos(now)) {
            let g = rec.cut(Nanos(now));
            off += g.len() as u64;
            assert!(w.submit(g));
            groups.push((now, off));
        }
        let st = w.status();
        samples.push(Sample {
            wall: t0.elapsed().as_nanos() as i64,
            generated: now,
            written: st.written_bytes,
            durable: st.durable_bytes,
        });
    }
    let t = RUN_MS * MS;
    let max_sync = w.status().max_sync_ns as i64;
    w.close(rec.finish(end(end_reason::COMPLETED, t), Nanos(t)).unwrap())
        .unwrap();
    let raw = fs::read(dir.join(RAW)).unwrap();

    // Observed lags per group: cut → written (writer backlog) and
    // written → durable (sync interval + sync duration + scheduling).
    let first = |f: &dyn Fn(&Sample) -> bool| samples.iter().find(|s| f(s)).map(|s| s.wall);
    let (mut backlog, mut sync, mut covered) = (0i64, 0i64, 0u64);
    // A group still not durable at the end lags at least until the last
    // sample, so a sync loop that stalls mid-run fails the bound below.
    let last_wall = samples.last().map_or(0, |s| s.wall);
    for &(cut, end) in &groups {
        let Some(tw) = first(&|s| s.written >= end) else {
            break;
        };
        backlog = backlog.max(tw - cut);
        match first(&|s| s.durable >= end) {
            Some(td) => {
                sync = sync.max(td - tw);
                covered = end;
            }
            None => sync = sync.max(last_wall - tw),
        }
    }
    // The sync loop's guarantee on any disk: durable within
    // max(fsync_ms, longest sync) + longest sync. (Not checked: that the
    // early start keeps the lag near fsync_ms with steady syncs; on a fast
    // disk syncs are too short to tell, and without it the lag still fits.)
    let allowed = (FSYNC_MS as i64 * MS).max(max_sync) + max_sync + SCHED_ALLOWANCE_MS * MS;
    assert!(
        sync <= allowed,
        "sync lag {:.1} ms > max(fsync_ms {FSYNC_MS}, longest sync {:.1} ms) + longest sync + {SCHED_ALLOWANCE_MS} ms",
        sync as f64 / 1e6,
        max_sync as f64 / 1e6
    );
    // Crash instants whose next group became durable within the run.
    let candidates: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.durable > 0 && s.durable < covered)
        .collect();
    assert!(candidates.len() > 100, "{} candidates", candidates.len());

    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let bound = FLUSH_MS * MS + backlog + sync;
    let mut max_loss = 0;
    for k in 0..12 {
        let s = candidates[next() as usize % candidates.len()];
        let (d, wr) = (s.durable as usize, s.written as usize);
        let n = next() as usize % 8193;
        let mut img = raw[..d].to_vec();
        match k % 3 {
            0 => img.resize(d + n, 0),
            1 => img.extend((0..n).map(|_| next() as u8)),
            _ => img.extend_from_slice(&raw[d..d + n % (wr - d + 1)]),
        }
        let Outcome::Recovered(sc) = recover(img.as_slice()) else {
            panic!("crash image {k} does not recover")
        };
        let stream: Vec<u8> = img[..sc.valid_prefix_bytes as usize]
            .iter()
            .copied()
            .chain(sc.synthetic_end().unwrap())
            .collect();
        let report = validate(stream.as_slice());
        assert!(report.is_valid() && report.complete, "{:?}", report.issues);
        let loss = s.generated - report.session_end.unwrap().t_end_ns;
        assert!(
            (0..=bound).contains(&loss),
            "image {k}: loss {loss} ns > bound {bound} ns"
        );
        max_loss = max_loss.max(loss);
    }
    println!(
        "power-loss model: max loss {:.1} ms; bound = chunk_flush {FLUSH_MS} ms + backlog {:.1} ms + sync {:.1} ms (fsync_ms {FSYNC_MS}, longest sync {:.1} ms)",
        max_loss as f64 / 1e6,
        backlog as f64 / 1e6,
        sync as f64 / 1e6,
        max_sync as f64 / 1e6,
    );
    fs::remove_dir_all(root).unwrap();
}
