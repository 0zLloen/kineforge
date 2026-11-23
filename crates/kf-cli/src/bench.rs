//! `kf-cli bench`: the PERFORMANCE_PLAN N9a synthetic telemetry generator
//! (§4.6). Per source one generator thread emits `--rate` motion events/s in
//! 1 ms batches (per-source `seq`, `t_ns` from the shared session clock); a
//! frame thread emits one `frame` + one `sim_camera` per 1/`--frames` s.
//! Both `try_send` into a bounded queue that never blocks: a full queue
//! drops the message and counts it. The recorder thread pushes everything
//! into a `Recorder`, cuts a group whenever one is due (between messages)
//! and submits it to a `kf_store::SessionWriter`.
//!
//! stdout: one JSON result. stderr: a `{"session_dir"}` line, then every
//! 100 ms `{"t_ns", "backlog_bytes", "backlog_ns"}`: the latest generated
//! `t_ns` and the writer backlog (bytes submitted and not yet written, and
//! the session time between the last submitted and the last written
//! commit). The N9b kill test reads these.
//!
//! Exit 1 when anything was dropped or the log does not validate.

use crate::{Opts, Res, num, usage};
use kf_core::{
    ClockKind, ClockMeaning, Degrees, EvidenceClass, Frame, InputLevel, LinearSensitivity, Motion,
    Nanos, ReceiptMeaning, SimCamera, Units, ValueScale,
};
use kf_store::{RAW, SessionWriter, Status, WriterOptions};
use kf_telemetry::json::{
    ClockDomain, SCHEMA, Sensitivity, SessionEnd, SessionHeader, Simulation, SourceDeclaration,
    end_reason, to_value,
};
use kf_telemetry::validate::validate;
use kf_telemetry::{FlushPolicy, Kind, Record, Recorder};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

pub const FLAGS: [&str; 7] = [
    "--seconds",
    "--sources",
    "--rate",
    "--frames",
    "--out",
    "--fsync-ms",
    "--flush-ms",
];

/// The recorder's wake interval; it also bounds its cut lag.
const POLL: Duration = Duration::from_millis(5);

enum Msg {
    Motion(Vec<Motion>),
    Frame(Frame, SimCamera),
}

/// Session time: nanoseconds since `start`.
fn now(start: Instant) -> i64 {
    start.elapsed().as_nanos() as i64
}

fn sleep_until(start: Instant, at: Duration) {
    if let Some(d) = at.checked_sub(start.elapsed()) {
        thread::sleep(d);
    }
}

fn header() -> SessionHeader {
    let k = Degrees(0.022).to_radians().0;
    SessionHeader {
        schema: SCHEMA.into(),
        session_id: kf_store::new_session_id(),
        started_at_utc: Some(kf_store::utc_now_rfc3339()),
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
            sim_hz: 1000,
            input_source: 0,
            rest: BTreeMap::new(),
        }),
        sensitivity: Some(Sensitivity {
            model: "linear".into(),
            yaw_rad_per_unit: k,
            pitch_rad_per_unit: k,
            sens_x: 1.0,
            sens_y: 1.0,
            invert_y: false,
            rest: BTreeMap::new(),
        }),
        rest: [(
            "app".to_owned(),
            json!({"name": "kf-cli bench", "version": env!("CARGO_PKG_VERSION")}),
        )]
        .into(),
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
            processing: vec!["injected_test_stream".into()],
            rest: [("kind".to_owned(), json!("mouse"))].into(),
        }),
    }
}

/// Returns (events generated, events dropped at the queue).
fn generator(
    source: u16,
    rate: u64,
    start: Instant,
    end: Duration,
    tx: &SyncSender<Msg>,
    latest: &AtomicI64,
) -> (u64, u64) {
    let (mut generated, mut dropped, mut seq) = (0, 0, 0u32);
    for k in 0u64.. {
        let due = Duration::from_millis(k);
        if due >= end {
            break;
        }
        sleep_until(start, due);
        let t = now(start);
        let n = rate * (k + 1) / 1000 - rate * k / 1000;
        let batch: Vec<Motion> = (0..n)
            .map(|_| {
                let m = Motion {
                    t_ns: Nanos(t),
                    src_ts_ns: None,
                    dx: (seq % 7) as i32 - 3,
                    dy: (seq % 5) as i32 - 2,
                    seq,
                    source,
                    flags: 0,
                };
                seq = seq.wrapping_add(1);
                m
            })
            .collect();
        generated += n;
        if tx.try_send(Msg::Motion(batch)).is_err() {
            dropped += n;
        }
        latest.fetch_max(t, Relaxed);
    }
    (generated, dropped)
}

/// One `frame` (one tick at 1000 Hz) and one `sim_camera` per 1/`fps` s;
/// the radians come from the header's sensitivity, so the log validates.
/// Returns (frames generated, frames dropped at the queue).
fn framer(
    fps: u64,
    start: Instant,
    end: Duration,
    tx: &SyncSender<Msg>,
    latest: &AtomicI64,
    lin: LinearSensitivity,
) -> (u64, u64) {
    let (mut n, mut dropped) = (0u64, 0);
    loop {
        let due = Duration::from_nanos(n * 1_000_000_000 / fps);
        if due >= end {
            return (n, dropped);
        }
        sleep_until(start, due);
        let t = Nanos(now(start));
        let frame = Frame {
            frame: n,
            t_start_ns: t,
            t_input_cutoff_ns: t,
            first_tick: n,
            ticks: 1,
            flags: 0,
        };
        let (yaw_units, pitch_units) = (n as i64 * 3, (n % 2000) as i64 - 1000);
        let cam = SimCamera {
            tick: n,
            yaw_units,
            pitch_units,
            yaw_rad: lin.yaw(yaw_units, ValueScale::X1).0,
            pitch_rad: lin.pitch(pitch_units, ValueScale::X1).0,
        };
        if tx.try_send(Msg::Frame(frame, cam)).is_err() {
            dropped += 1;
        }
        latest.fetch_max(t.0, Relaxed);
        n += 1;
    }
}

/// On-CPU seconds from a `/proc` thread `schedstat` file: its first field,
/// in ns (Linux; `None` elsewhere).
fn cpu_seconds(schedstat: &Path) -> Option<f64> {
    let s = fs::read_to_string(schedstat).ok()?;
    Some(s.split_whitespace().next()?.parse::<u64>().ok()? as f64 / 1e9)
}

/// CPU seconds of this process's thread named `name` (Linux).
fn task_cpu(name: &str) -> Option<f64> {
    let task = fs::read_dir("/proc/self/task")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| fs::read_to_string(p.join("comm")).is_ok_and(|c| c.trim_end() == name))?;
    cpu_seconds(&task.join("schedstat"))
}

struct RecorderOut {
    status: Status,
    recorder_cpu: Option<f64>,
    writer_cpu: Option<f64>,
    sync_cpu: Option<f64>,
}

fn recorder(
    rx: Receiver<Msg>,
    mut rec: Recorder,
    writer: SessionWriter,
    start: Instant,
    latest: &AtomicI64,
) -> io::Result<RecorderOut> {
    // Submitted groups not yet known to be written: (end offset, commit t_ns).
    let mut pending: VecDeque<(u64, i64)> = VecDeque::new();
    let (mut submitted, mut written_t, mut next_progress) = (0u64, 0i64, 0i64);
    let push = |rec: &mut Recorder, r| rec.push(r).expect("generated records encode");
    let (mut open, mut lost) = (true, false);
    while open && !lost {
        // Wake every POLL and drain the queue: waking per message (4000/s
        // at the N9a rates) costs more CPU than the encoding.
        thread::sleep(POLL);
        loop {
            match rx.try_recv() {
                Ok(Msg::Motion(batch)) => batch
                    .into_iter()
                    .for_each(|m| push(&mut rec, Record::Motion(m))),
                Ok(Msg::Frame(f, c)) => {
                    push(&mut rec, Record::Frame(f));
                    push(&mut rec, Record::SimCamera(c));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        let t = now(start);
        // Progress before the cut in this pass, so it does not always show
        // the group just cut in flight. Both re-arm ~100 ms after they fire,
        // so in practice progress fires about one POLL after each cut.
        if t >= next_progress {
            next_progress = t + 100_000_000;
            let st = writer.status();
            rec.written(st.written_bytes);
            while let Some(&(end, ct)) = pending.front()
                && end <= st.written_bytes
            {
                written_t = ct;
                pending.pop_front();
            }
            let backlog_ns = pending.back().map_or(0, |&(_, ct)| ct - written_t);
            eprintln!(
                "{}",
                json!({"t_ns": latest.load(Relaxed), "backlog_bytes": st.backlog_bytes, "backlog_ns": backlog_ns})
            );
        }
        if rec.due(Nanos(t)) {
            let group = rec.cut(Nanos(t));
            submitted += group.len() as u64;
            pending.push_back((submitted, t));
            // Any writer drop ends the session (kf_store writer docs).
            lost = !writer.submit(group) || writer.status().storage_error;
        }
    }
    // The last regular group goes through the writer like the others, so
    // `finish` adds only the commit and session_end, which fit the reserve.
    if !lost {
        lost = !writer.submit(rec.cut(Nanos(now(start))));
    }
    // Let the writer drain, then read its CPU time while its thread still
    // runs: `close` joins it.
    let st = writer.drain();
    let lost = lost || st.storage_error;
    let (writer_cpu, sync_cpu) = (task_cpu("kf-store-writer"), task_cpu("kf-store-sync"));
    let t = now(start);
    if lost {
        rec.writer_lost(st.written_bytes, Nanos(t))
            .map_err(io::Error::other)?;
    }
    let end = SessionEnd {
        completed: !lost,
        end_reason: if lost {
            end_reason::STORAGE_ERROR
        } else {
            end_reason::COMPLETED
        }
        .into(),
        t_end_ns: t,
        counts: BTreeMap::new(),
        dropped_chunks: 0,
        max_writer_backlog_bytes: st.max_backlog_bytes,
        interruptions: 0,
        refresh_hz_measured: None,
        raf_hz_measured: None,
        rest: BTreeMap::new(),
    };
    let status = writer.close(rec.finish(end, Nanos(t)).expect("session_end encodes"))?;
    Ok(RecorderOut {
        status,
        recorder_cpu: cpu_seconds(Path::new("/proc/thread-self/schedstat")),
        writer_cpu,
        sync_cpu,
    })
}

pub fn run(opts: &Opts) -> Res {
    let seconds = num(opts, "--seconds", 10)?;
    let sources = num(opts, "--sources", 3)?;
    let rate = num(opts, "--rate", 8000)?;
    let fps = num(opts, "--frames", 1000)?;
    let fsync_ms = num(opts, "--fsync-ms", 1000)?;
    let flush_ms = num(opts, "--flush-ms", 100)?;
    let Ok(sources @ 1..=0xFFFE) = u16::try_from(sources) else {
        return usage("--sources must be 1..=65534".into());
    };
    if fps == 0 || rate > 1 << 32 || flush_ms > 1 << 32 {
        return usage("--frames must be > 0; --rate and --flush-ms ≤ 2^32".into());
    }
    let (root, keep) = match opts.get("--out") {
        Some(d) => (PathBuf::from(d), true),
        None => (
            std::env::temp_dir().join(format!("kf-bench-{}", std::process::id())),
            false,
        ),
    };
    let header = header();
    let lin = header
        .sensitivity
        .as_ref()
        .and_then(Sensitivity::linear)
        .expect("the bench header is linear");
    let dir = kf_store::create_session_dir(&root, &header.session_id)?;
    let writer = SessionWriter::create(
        &dir,
        WriterOptions {
            fsync_ms,
            ..WriterOptions::default()
        },
    )?;
    let policy = FlushPolicy {
        chunk_flush_ns: flush_ms as i64 * 1_000_000,
        ..FlushPolicy::default()
    };
    let mut rec = Recorder::new(&header, policy)?;
    for id in 0..sources {
        rec.push(source(id))?;
    }
    eprintln!("{}", json!({"session_dir": dir}));

    // About one second of messages.
    let (tx, rx) = sync_channel((usize::from(sources) + 1) * 1000);
    let latest = AtomicI64::new(0);
    let end = Duration::from_secs(seconds);
    let start = Instant::now();
    let (gens, (frames, frames_dropped), out) = thread::scope(|s| {
        let latest = &latest;
        let gens: Vec<_> = (0..sources)
            .map(|id| {
                let tx = tx.clone();
                s.spawn(move || generator(id, rate, start, end, &tx, latest))
            })
            .collect();
        let tx_frames = tx.clone();
        let fr = s.spawn(move || framer(fps, start, end, &tx_frames, latest, lin));
        drop(tx);
        let r = s.spawn(move || recorder(rx, rec, writer, start, latest));
        let join = "bench thread panicked";
        let gens: Vec<(u64, u64)> = gens.into_iter().map(|h| h.join().expect(join)).collect();
        (gens, fr.join().expect(join), r.join().expect(join))
    });
    let elapsed = start.elapsed().as_secs_f64();
    let out = out?;
    let st = out.status;

    let report = validate(BufReader::new(File::open(dir.join(RAW))?));
    let generated: u64 = gens.iter().map(|g| g.0).sum();
    let queue_dropped: u64 = gens.iter().map(|g| g.1).sum();
    let recorded = report.counts.get("input_motion").copied().unwrap_or(0);
    let lost: u64 = report.losses.iter().map(|l| l.count).sum();
    let cpu = match (out.recorder_cpu, out.writer_cpu, out.sync_cpu) {
        (Some(r), Some(w), Some(s)) => Some((r + w + s) / elapsed * 100.0),
        _ => None,
    };
    let errors: Vec<String> = report.errors().take(5).map(ToString::to_string).collect();
    let result = json!({
        "session_dir": keep.then(|| dir.display().to_string()),
        "seconds": elapsed,
        "config": {"sources": sources, "rate": rate, "frames": fps, "fsync_ms": fsync_ms, "flush_ms": flush_ms},
        "events_generated": generated,
        "events_recorded": recorded,
        "frames_generated": frames,
        "frames_recorded": report.counts.get("frame").copied().unwrap_or(0),
        "queue_drops": {"events": queue_dropped, "frames": frames_dropped},
        "writer_drops": {"groups": st.dropped_groups, "bytes": st.dropped_bytes},
        "bytes": st.written_bytes,
        "bytes_per_event": st.written_bytes as f64 / recorded.max(1) as f64,
        "max_writer_backlog_bytes": st.max_backlog_bytes,
        "cpu_seconds": {"recorder": out.recorder_cpu, "writer": out.writer_cpu, "sync": out.sync_cpu},
        "cpu_percent_of_one_core": cpu,
        "validator": {
            "valid": report.is_valid(),
            "complete": report.complete,
            "seq_gaps": report.losses.len(),
            "lost_events": lost,
            "errors": errors,
        },
    });
    println!("{}", serde_json::to_string_pretty(&result)?);
    if !keep {
        fs::remove_dir_all(&root)?;
    }
    let clean = report.is_valid()
        && report.losses.is_empty()
        && queue_dropped == 0
        && frames_dropped == 0
        && st.dropped_groups == 0;
    Ok(if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
