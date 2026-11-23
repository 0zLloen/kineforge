//! One failing case per validator and reader rule (DATA_SPECIFICATION.md
//! §5, §10). Each row is a valid log (golden `full`, or a Recorder-built
//! one) with one targeted edit, and names the error that edit must raise,
//! so dropping or weakening any rule fails a row.

use kf_core::records::{annotation_code, control_action};
use kf_core::{
    Annotation, ClockSync, Drain, Frame, Motion, Nanos, SimCamera, SimControl, ValueScale,
};
use kf_telemetry::format::{ChunkHeader, FILE_HEADER, Kind, chunk_crc, write_chunk};
use kf_telemetry::golden;
use kf_telemetry::json::{SessionEnd, SessionHeader, SourceDeclaration, from_value, to_value};
use kf_telemetry::reader::{ChunkReader, GroupReader, RawChunk, ReadError, decode_chunk};
use kf_telemetry::recover::{Outcome, recover};
use kf_telemetry::validate::{Report, validate};
use kf_telemetry::{Chunk, FlushPolicy, Record, Recorder};
use serde_json::{Value, json};

// --- golden full, split into framed chunks and edited -----------------------

fn chunks() -> Vec<Vec<u8>> {
    let b = golden::full();
    let mut out = Vec::new();
    let mut at = FILE_HEADER.len();
    while at < b.len() {
        let len = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        out.push(b[at..at + 24 + len].to_vec());
        at += 24 + len;
    }
    out
}

fn join(chunks: &[Vec<u8>]) -> Vec<u8> {
    [&FILE_HEADER[..], &chunks.concat()].concat()
}

/// Rewrites a framed chunk's CRC over its header and payload.
fn reseal(c: &mut [u8]) {
    let n = c.len() - 8;
    let crc = chunk_crc(c[..16].try_into().unwrap(), &c[16..n]);
    c[n..n + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Index of the `n`th chunk of `kind`.
fn find(chunks: &[Vec<u8>], kind: Kind, n: usize) -> usize {
    (0..chunks.len())
        .filter(|&i| chunks[i][4] == kind as u8)
        .nth(n)
        .unwrap()
}

/// golden full with its chunk list edited.
fn full_chunks(edit: impl FnOnce(&mut Vec<Vec<u8>>)) -> Vec<u8> {
    let mut cs = chunks();
    edit(&mut cs);
    join(&cs)
}

/// golden full with its `n`th chunk of `kind` edited (header at 0, payload
/// at 16) and resealed.
fn full_with(kind: Kind, n: usize, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    full_chunks(|cs| {
        let i = find(cs, kind, n);
        edit(&mut cs[i]);
        reseal(&mut cs[i]);
    })
}

/// golden full with the first chunks of kinds `a` and `b` swapped.
fn swapped(a: Kind, b: Kind) -> Vec<u8> {
    full_chunks(|cs| {
        let (i, j) = (find(cs, a, 0), find(cs, b, 0));
        cs.swap(i, j);
    })
}

fn json_chunk(kind: Kind, v: &Value) -> Vec<u8> {
    let mut p = serde_json::to_vec(v).unwrap();
    p.resize(p.len().next_multiple_of(8), b' ');
    let mut c = Vec::new();
    write_chunk(&mut c, kind, 1, &p);
    c
}

/// golden full with its session_end JSON edited.
fn end_with(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    full_with(Kind::SessionEnd, 0, |c| {
        let mut v: Value = serde_json::from_slice(&c[16..c.len() - 8]).unwrap();
        edit(&mut v);
        *c = json_chunk(Kind::SessionEnd, &v);
    })
}

/// Writes `v` at byte `at` of a framed chunk.
fn put(c: &mut [u8], at: usize, v: &[u8]) {
    c[at..at + v.len()].copy_from_slice(v);
}

// --- Recorder-built logs ----------------------------------------------------

/// golden full's records in file order.
fn full_records() -> Vec<Record> {
    let full = golden::full();
    GroupReader::new(full.as_slice())
        .unwrap()
        .flat_map(|g| g.records().cloned().collect::<Vec<_>>())
        .collect()
}

/// golden full's header: simulation at 1000 Hz driven by source 0, linear
/// sensitivity, clock domains 0 and 1.
fn header() -> SessionHeader {
    let Record::Json { value, .. } = &full_records()[0] else {
        panic!()
    };
    from_value(value).unwrap()
}

/// golden full's source 0 declaration (counts, value_scale 1, clock domain
/// 1) as source `id`, edited.
fn source(id: u16, edit: impl FnOnce(&mut SourceDeclaration)) -> Record {
    let Record::Json { value, .. } = &full_records()[1] else {
        panic!()
    };
    let mut d: SourceDeclaration = from_value(value).unwrap();
    assert_eq!((d.id, d.value_scale), (0, 1));
    d.id = id;
    edit(&mut d);
    Record::Json {
        kind: Kind::SourceDeclare,
        value: to_value(&d),
    }
}

fn src(id: u16) -> Record {
    source(id, |_| {})
}

/// One group per element, then the final group and session_end.
fn recorded(h: &SessionHeader, groups: &[Vec<Record>]) -> Vec<u8> {
    let mut r = Recorder::new(h, FlushPolicy::default()).unwrap();
    let mut out = Vec::new();
    for (t, g) in (1..).zip(groups) {
        for rec in g {
            r.push(rec.clone()).unwrap();
        }
        out.extend(r.cut(Nanos(t)));
    }
    let end: SessionEnd = from_value(&json!({
        "completed": true, "end_reason": "completed", "t_end_ns": 100, "counts": {}
    }))
    .unwrap();
    out.extend(r.finish(end, Nanos(100)).unwrap());
    out
}

/// golden full's header, one group: source 0 declared, then `recs`.
fn with(recs: Vec<Record>) -> Vec<u8> {
    recorded(&header(), &[[vec![src(0)], recs].concat()])
}

fn motion(source: u16, seq: u32, t: i64) -> Record {
    Record::Motion(Motion {
        t_ns: Nanos(t),
        src_ts_ns: None,
        dx: 1,
        dy: 0,
        seq,
        source,
        flags: 0,
    })
}

/// A one-tick frame.
fn frame(frame: u64, first_tick: u64) -> Record {
    Record::Frame(Frame {
        frame,
        t_start_ns: Nanos(0),
        t_input_cutoff_ns: Nanos(0),
        first_tick,
        ticks: 1,
        flags: 0,
    })
}

fn drain(frame: u64, source: u16) -> Record {
    Record::Drain(Drain {
        frame,
        source,
        last_seq: 0,
    })
}

fn control(action: u8, new_source: Option<u16>) -> Record {
    Record::SimControl(SimControl {
        tick: 0,
        t_ns: Nanos(0),
        frame: 0,
        action,
        phase: 0,
        reason: 0,
        new_source,
    })
}

/// A camera at tick 5 whose radians are off by the given ulps.
fn camera(yaw_ulps: u64, pitch_ulps: u64) -> Record {
    let lin = header().sensitivity.unwrap().linear().unwrap();
    let (yaw_units, pitch_units) = (100, -50);
    let off = |x: f64, ulps| f64::from_bits(x.to_bits() + ulps);
    Record::SimCamera(SimCamera {
        tick: 5,
        yaw_units,
        pitch_units,
        yaw_rad: off(lin.yaw(yaw_units, ValueScale::X1).0, yaw_ulps),
        pitch_rad: off(lin.pitch(pitch_units, ValueScale::X1).0, pitch_ulps),
    })
}

// --- checks -----------------------------------------------------------------

/// Each row must raise an error whose message contains its text.
fn assert_errors(rows: Vec<(&str, Vec<u8>, &str)>) {
    for (name, bytes, want) in rows {
        let rep = validate(bytes.as_slice());
        assert!(
            rep.errors().any(|i| i.message.contains(want)),
            "{name}: no error containing {want:?} in {:#?}",
            rep.issues
        );
    }
}

fn assert_clean(name: &str, bytes: &[u8]) -> Report {
    let rep = validate(bytes);
    assert!(
        rep.complete && rep.issues.is_empty(),
        "{name}: {:#?}",
        rep.issues
    );
    rep
}

/// Review finding 37 (structure, declarations, header) and 41 (non-finite
/// floats on disk).
#[test]
fn structure_and_declaration_rules() {
    let header_in_group_2 = || {
        full_chunks(|cs| {
            let h = cs.remove(0);
            let at = find(cs, Kind::Commit, 0) + 1;
            cs.insert(at, h);
        })
    };
    let mut no_linear = header();
    no_linear.sensitivity = None;
    let mut sim_hz_7 = header();
    sim_hz_7.simulation.as_mut().unwrap().sim_hz = 7;
    let mut schema_x = header();
    schema_x.schema = "x".into();
    assert_errors(vec![
        (
            "session_header in group 2",
            header_in_group_2(),
            "session_header is not the first chunk",
        ),
        (
            "session_header in group 2",
            header_in_group_2(),
            "the first chunk is not session_header",
        ),
        (
            "first group is a bare commit",
            full_chunks(|cs| {
                // t_ns 0, tick and frame None, no sources.
                let mut p = [0u8; 32];
                p[8..24].fill(0xFF);
                let mut c = Vec::new();
                write_chunk(&mut c, Kind::Commit, 1, &p);
                cs.insert(0, c);
            }),
            "the first group has no session_header",
        ),
        (
            "device_info after input_motion",
            swapped(Kind::DeviceInfo, Kind::InputMotion),
            "declaration device_info after non-declaration chunks",
        ),
        (
            "drain before frame",
            swapped(Kind::Frame, Kind::Drain),
            "kind 0x10 after 0x11: kinds not ascending",
        ),
        (
            "CRC-valid drain with non-zero padding",
            full_with(Kind::Drain, 0, |c| c[16 + 10] = 1),
            "bad drain payload",
        ),
        (
            "CRC-valid chunk after session_end",
            full_chunks(|cs| cs.push(cs[1].clone())),
            "chunk after the terminal session_end",
        ),
        (
            "NaN ang_err_rad",
            full_with(Kind::SimShot, 0, |c| {
                put(c, 16 + 24, &f64::NAN.to_le_bytes())
            }),
            "NaN or infinity in sim_shot",
        ),
        (
            "source declared twice",
            with(vec![src(0)]),
            "source 0 declared twice",
        ),
        (
            "source 0xFFFF",
            with(vec![src(0xFFFF)]),
            "source id 0xFFFF is reserved",
        ),
        (
            "value_scale 3",
            recorded(&header(), &[vec![source(0, |d| d.value_scale = 3)]]),
            "source 0: value_scale 3 not 1/256/1024",
        ),
        (
            "undeclared clock_domain",
            recorded(&header(), &[vec![source(0, |d| d.clock_domain = 9)]]),
            "source 0: clock_domain 9 not declared",
        ),
        (
            "sim_hz 7",
            recorded(&sim_hz_7, &[vec![src(0)]]),
            "sim_hz 7 does not divide 1e9",
        ),
        (
            "schema x",
            recorded(&schema_x, &[vec![src(0)]]),
            "unsupported schema \"x\"",
        ),
        (
            "simulation without sensitivity",
            recorded(&no_linear, &[vec![src(0)]]),
            "simulation without a linear sensitivity",
        ),
        (
            "motion from undeclared source",
            with(vec![motion(9, 0, 1)]),
            "input_motion from undeclared source 9",
        ),
        (
            "annotation names undeclared source",
            with(vec![Record::Annotation(Annotation {
                t_ns: Nanos(1),
                code: annotation_code::USER_MARKER,
                source: Some(9),
                first_seq: 0,
                lost: None,
                text: String::new(),
            })]),
            "annotation names undeclared source Some(9)",
        ),
        (
            "drain of undeclared source",
            with(vec![drain(0, 9)]),
            "drain of undeclared source 9",
        ),
        (
            "clock_sync in undeclared domain",
            with(vec![Record::ClockSync(ClockSync {
                t_ns: Nanos(1),
                domain: 9,
                src_ts_ns: Nanos(1),
                uncertainty_ns: 0,
            })]),
            "clock_sync for undeclared domain 9",
        ),
    ]);
}

/// Review finding 38: per-source seq and t_ns order, and the u32 seq wrap.
#[test]
fn seq_and_t_ns_rules() {
    assert_errors(vec![
        (
            "repeated seq",
            with(vec![motion(0, 0, 10), motion(0, 1, 20), motion(0, 1, 30)]),
            "source 0: seq 1 repeats or goes back",
        ),
        (
            "t_ns backwards",
            with(vec![motion(0, 0, 20), motion(0, 1, 10)]),
            "source 0: t_ns 10 goes backwards in seq order",
        ),
    ]);
    // seq wraps after u32::MAX (§6.2): no repeat, no loss.
    let wrap = recorded(
        &header(),
        &[
            vec![
                src(0),
                motion(0, 0xFFFF_FFFE, 10),
                motion(0, 0xFFFF_FFFF, 20),
            ],
            vec![motion(0, 0, 30), motion(0, 1, 40)],
        ],
    );
    let rep = assert_clean("seq wrap", &wrap);
    assert!(rep.losses.is_empty(), "{:?}", rep.losses);
    assert_eq!(rep.counts["input_motion"], 4);
}

/// Review finding 39: commit watermarks and session_end consistency. Edits
/// the second commit of golden full (sources 0 and 1; tick, frame Some).
#[test]
fn commit_and_session_end_rules() {
    /// Writes `v` at byte `at` of the commit chunk.
    fn commit(at: usize, v: &[u8]) -> Vec<u8> {
        full_with(Kind::Commit, 1, |c| put(c, at, v))
    }
    // Chunk bytes: header, then t_ns, tick, frame, count, pad, then
    // (source, pad, last_seq) per source.
    let (t_ns, tick, frame, entry0, entry1) = (16, 24, 32, 48, 56);
    let back = "commit tick/frame watermark goes backwards";
    assert_errors(vec![
        (
            "commit t_ns goes back",
            commit(t_ns, &0i64.to_le_bytes()),
            "commit t_ns goes backwards",
        ),
        (
            "commit tick goes back",
            commit(tick, &0u64.to_le_bytes()),
            back,
        ),
        (
            "commit frame goes back",
            commit(frame, &0u64.to_le_bytes()),
            back,
        ),
        (
            "commit tick Some to None",
            commit(tick, &u64::MAX.to_le_bytes()),
            back,
        ),
        (
            "commit claims an unwritten last_seq",
            commit(entry0 + 4, &999u32.to_le_bytes()),
            "commit: source 0 last_seq 999 but",
        ),
        (
            "commit lists source 0 twice",
            commit(entry1, &0u16.to_le_bytes()),
            "commit lists a source twice",
        ),
        (
            "session_end counts one motion too many",
            end_with(|v| {
                let n = v["counts"]["input_motion"].as_u64().unwrap();
                v["counts"]["input_motion"] = json!(n + 1);
            }),
            "session_end counts",
        ),
        (
            "session_end before the last commit",
            end_with(|v| v["t_end_ns"] = json!(0)),
            "session_end t_end_ns precedes the last commit",
        ),
    ]);
}

/// Review finding 40: simulation rules. Golden full's header: driving
/// source 0, value_scale 1.
#[test]
fn simulation_rules() {
    let pause = || control(control_action::PAUSE, None);
    let switch = || control(control_action::SWITCH_SOURCE, Some(1));
    let radians = "sim_camera tick 5: radians differ from the recomputation";
    assert_errors(vec![
        (
            "frame number repeats",
            with(vec![frame(0, 0), frame(0, 1)]),
            "frame 0 does not increase",
        ),
        (
            "tick gap",
            with(vec![frame(0, 0), frame(1, 2)]),
            "frame 1: first_tick 2 breaks tick continuity",
        ),
        (
            "drain of a non-driving source",
            with(vec![src(1), frame(0, 0), drain(0, 1)]),
            "drain of source 1 but the driving source is 0",
        ),
        (
            "two drains in one frame",
            with(vec![frame(0, 0), drain(0, 0), drain(0, 0)]),
            "frame 0 has two drains",
        ),
        (
            "switch outside a pause",
            with(vec![src(1), switch()]),
            "switch_source outside a pause",
        ),
        (
            "switch to another value_scale",
            with(vec![source(1, |d| d.value_scale = 256), pause(), switch()]),
            "switch from source 0 to 1 changes level, units or scale",
        ),
        ("pitch_rad 1 ulp off", with(vec![camera(0, 1)]), radians),
        ("yaw_rad 1 ulp off", with(vec![camera(1, 0)]), radians),
    ]);
    assert_clean("exact camera", &with(vec![camera(0, 0)]));
    // After a switch, the new source drives later groups.
    let switched = recorded(
        &header(),
        &[
            vec![
                src(0),
                src(1),
                pause(),
                switch(),
                control(control_action::RESUME, None),
            ],
            vec![frame(0, 0), drain(0, 1)],
        ],
    );
    assert_clean("valid switch", &switched);
}

/// Review finding 43: file header version and reserved field, chunk
/// reserved field and trailer.
#[test]
fn reader_rejects_version_reserved_and_trailer() {
    let mut h = FILE_HEADER;
    h[8] = 2;
    assert!(matches!(
        ChunkReader::new(&h[..]).err(),
        Some(ReadError::UnsupportedVersion(2))
    ));
    let mut h = FILE_HEADER;
    h[12] = 1;
    assert!(matches!(
        ChunkReader::new(&h[..]).err(),
        Some(ReadError::NonZeroReserved { offset: 12 })
    ));

    let error = |b: Vec<u8>| {
        ChunkReader::new(b.as_slice())
            .unwrap()
            .find_map(Result::err)
    };
    let second = (FILE_HEADER.len() + chunks()[0].len()) as u64;
    let reserved = full_chunks(|cs| {
        cs[1][12] = 1;
        reseal(&mut cs[1]);
    });
    assert!(matches!(
        error(reserved),
        Some(ReadError::NonZeroReserved { offset }) if offset == second
    ));
    let trailer = full_chunks(|cs| *cs[1].last_mut().unwrap() = 1);
    assert!(matches!(
        error(trailer),
        Some(ReadError::NonZeroTrailer { offset }) if offset == second
    ));
}

/// Review finding 44: a known kind with unsupported flags is skipped, not
/// decoded (§5.3).
#[test]
fn flagged_chunks_are_skipped() {
    let raw = |flags| RawChunk {
        offset: 16,
        header: ChunkHeader {
            payload_len: 32,
            kind: Kind::InputMotion as u8,
            kind_version: 1,
            flags,
            record_count: 1,
            reserved: 0,
        },
        payload: vec![0; 32],
    };
    assert!(matches!(decode_chunk(&raw(0)), Chunk::Records { .. }));
    assert!(matches!(decode_chunk(&raw(1)), Chunk::Skipped { .. }));
}

/// Review finding 45: session_end is terminal only directly after a commit
/// (§5.4).
#[test]
fn session_end_counts_only_directly_after_a_commit() {
    // FILE_HEADER ‖ session_end: nothing committed.
    let lone = full_chunks(|cs| *cs = vec![cs.pop().unwrap()]);
    let (ending, _) = GroupReader::new(lone.as_slice()).unwrap().ending();
    assert!(ending.session_end.is_none());
    assert_eq!(ending.uncommitted.len(), 1);
    assert!(matches!(
        recover(lone.as_slice()),
        Outcome::Unrecoverable(_)
    ));

    // golden full without its final commit: the last group and session_end
    // are uncommitted.
    let uncommitted = full_chunks(|cs| {
        let i = cs.len() - 2;
        assert_eq!(cs[i][4], Kind::Commit as u8);
        cs.remove(i);
    });
    let (ending, _) = GroupReader::new(uncommitted.as_slice()).unwrap().ending();
    assert!(ending.session_end.is_none());
    assert!(!ending.uncommitted.is_empty());
    assert!(matches!(
        recover(uncommitted.as_slice()),
        Outcome::Recovered(_)
    ));
}
