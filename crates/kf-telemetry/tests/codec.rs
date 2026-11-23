//! Record codec, recorder and reader behaviour.

use kf_core::{Annotation, Frame, Motion, Nanos, SimCamera, SimShot, SimTarget};
use kf_telemetry::format::{Kind, Layout, MAX_PAYLOAD};
use kf_telemetry::golden;
use kf_telemetry::json::{SessionEnd, SessionHeader, from_value};
use kf_telemetry::reader::{ChunkReader, GroupReader};
use kf_telemetry::recover::{Outcome, recover, recovered_stream};
use kf_telemetry::validate::validate;
use kf_telemetry::{FlushPolicy, Record, Recorder};

fn header() -> SessionHeader {
    let full = golden::full();
    let first = GroupReader::new(full.as_slice()).unwrap().next().unwrap();
    let Some(Record::Json { value, .. }) = first.records().next() else {
        panic!()
    };
    from_value(value).unwrap()
}

/// The golden file's source_declare (source 0).
fn declare() -> Record {
    let full = golden::full();
    let g = GroupReader::new(full.as_slice()).unwrap().next().unwrap();
    g.records().nth(1).unwrap().clone()
}

fn end() -> SessionEnd {
    from_value(&serde_json::json!({"completed": true, "end_reason": "completed", "t_end_ns": 9, "counts": {}})).unwrap()
}

/// The single data chunk payload of a one-record group.
fn payload_of(rec: Record) -> Vec<u8> {
    let mut r = Recorder::new(&header(), FlushPolicy::default()).unwrap();
    r.cut(Nanos(0));
    r.push(rec).unwrap();
    let group = r.cut(Nanos(1));
    let mut chunks =
        ChunkReader::new(&[&kf_telemetry::format::FILE_HEADER[..], &group].concat()[..])
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
    assert_eq!(chunks.len(), 2, "data chunk + commit");
    chunks.remove(0).payload
}

/// Byte layouts at the DATA_SPECIFICATION.md §6 offsets.
#[test]
fn fixed_layouts() {
    let m = payload_of(Record::Motion(Motion {
        t_ns: Nanos(0x0102_0304_0506_0708),
        src_ts_ns: None,
        dx: -2,
        dy: 3,
        seq: 0xA1B2_C3D4,
        source: 0x0E0F,
        flags: 0x0021,
    }));
    let mut want = Vec::new();
    want.extend_from_slice(&0x0102_0304_0506_0708i64.to_le_bytes());
    want.extend_from_slice(&i64::MIN.to_le_bytes());
    want.extend_from_slice(&(-2i32).to_le_bytes());
    want.extend_from_slice(&3i32.to_le_bytes());
    want.extend_from_slice(&0xA1B2_C3D4u32.to_le_bytes());
    want.extend_from_slice(&[0x0F, 0x0E, 0x21, 0x00]);
    assert_eq!(m, want);

    let s = payload_of(Record::SimShot(SimShot {
        tick: 5,
        t_ns: Nanos(6),
        target: None,
        seq: 7,
        ang_err_rad: 0.5,
        d_yaw_rad: -0.25,
        d_pitch_rad: 2.0,
        hit: true,
    }));
    assert_eq!(s.len(), 56);
    assert_eq!(s[16..20], u32::MAX.to_le_bytes());
    assert_eq!(s[24..32], 0.5f64.to_le_bytes());
    assert_eq!(s[48..56], [1, 0, 0, 0, 0, 0, 0, 0]);

    let a = payload_of(Record::Annotation(Annotation {
        t_ns: Nanos(1),
        code: 0x0F00,
        source: None,
        first_seq: 2,
        lost: None,
        text: "abc".into(),
    }));
    assert_eq!(a.len(), 32);
    assert_eq!(a[8..12], [0x00, 0x0F, 0xFF, 0xFF]);
    assert_eq!(a[16..24], [0xFF, 0xFF, 0xFF, 0xFF, 3, 0, 0, 0]);
    assert_eq!(a[24..32], *b"abc\0\0\0\0\0");
}

#[test]
fn every_kind_round_trips_in_kind_order() {
    let full = golden::full();
    let pushed: Vec<Record> = GroupReader::new(full.as_slice())
        .unwrap()
        .flat_map(|g| g.records().cloned().collect::<Vec<_>>())
        .collect();
    // Re-encode every committed record (header first), one group.
    let mut r = Recorder::new(&header(), FlushPolicy::default()).unwrap();
    for rec in pushed.iter().skip(1) {
        r.push(rec.clone()).unwrap();
    }
    let bytes = r.finish(end(), Nanos(10)).unwrap();
    let back: Vec<Record> = GroupReader::new(bytes.as_slice())
        .unwrap()
        .flat_map(|g| g.records().cloned().collect::<Vec<_>>())
        .collect();
    // A group orders by kind; within a kind, arrival order is kept.
    let mut want = pushed.clone();
    want.sort_by_key(|r| {
        (
            !r.kind().is_declaration(),
            if r.kind().is_declaration() {
                0
            } else {
                r.kind() as u8
            },
        )
    });
    assert_eq!(back, want);
    assert!(validate(bytes.as_slice()).complete);
}

/// `make` with each f64 field in turn set to NaN, +inf and -inf.
fn non_finite<const N: usize>(make: impl Fn([f64; N]) -> Record) -> Vec<Record> {
    let mut out = Vec::new();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for i in 0..N {
            let mut f = [0.0; N];
            f[i] = bad;
            out.push(make(f));
        }
    }
    out
}

#[test]
fn encoder_refuses_invalid_records() {
    let motion = |src_ts_ns| {
        Record::Motion(Motion {
            t_ns: Nanos(0),
            src_ts_ns,
            dx: 0,
            dy: 0,
            seq: 0,
            source: 0,
            flags: 0,
        })
    };
    let note = |source| {
        Record::Annotation(Annotation {
            t_ns: Nanos(0),
            code: 0x0F00,
            source,
            first_seq: 0,
            lost: None,
            text: "x".into(),
        })
    };
    let valid = [declare(), motion(None), note(None)];
    let mut r = Recorder::new(&header(), FlushPolicy::default()).unwrap();
    r.push(valid[0].clone()).unwrap();
    // Refused after t_ns is encoded; the next record shares the chunk.
    assert!(r.push(motion(Some(Nanos(i64::MIN)))).is_err());
    r.push(valid[1].clone()).unwrap();
    let floats = non_finite(|f: [f64; 2]| {
        Record::SimCamera(SimCamera {
            tick: 0,
            yaw_units: 0,
            pitch_units: 0,
            yaw_rad: f[0],
            pitch_rad: f[1],
        })
    })
    .into_iter()
    .chain(non_finite(|f: [f64; 7]| {
        Record::SimTarget(SimTarget {
            tick: 0,
            target: 0,
            event: 0,
            shape: 0,
            pos_m: [f[0], f[1], f[2]],
            vel_m_s: [f[3], f[4], f[5]],
            radius_m: f[6],
        })
    }))
    .chain(non_finite(|f: [f64; 3]| {
        Record::SimShot(SimShot {
            tick: 0,
            t_ns: Nanos(0),
            target: None,
            seq: 0,
            ang_err_rad: f[0],
            d_yaw_rad: f[1],
            d_pitch_rad: f[2],
            hit: false,
        })
    }));
    for rec in floats {
        assert!(r.push(rec.clone()).is_err(), "{rec:?}");
    }
    assert!(
        r.push(Record::Json {
            kind: Kind::SessionHeader,
            value: serde_json::json!({})
        })
        .is_err()
    );
    assert!(
        r.push(Record::Json {
            kind: Kind::SessionEnd,
            value: serde_json::json!({})
        })
        .is_err()
    );
    assert!(
        r.push(Record::Json {
            kind: Kind::DeviceInfo,
            value: serde_json::json!([1])
        })
        .is_err()
    );
    let long = Annotation {
        t_ns: Nanos(0),
        code: 1,
        source: None,
        first_seq: 0,
        lost: None,
        text: "x".repeat(MAX_PAYLOAD as usize),
    };
    assert!(r.push(Record::Annotation(long)).is_err());
    // Refused after 10 bytes are encoded.
    assert!(r.push(note(Some(0xFFFF))).is_err());
    r.push(valid[2].clone()).unwrap();
    // Refused records leave nothing behind (no stray bytes, no empty
    // chunks): the file is that of the valid records alone.
    let bytes = r.finish(end(), Nanos(1)).unwrap();
    let mut only_valid = Recorder::new(&header(), FlushPolicy::default()).unwrap();
    for rec in valid {
        only_valid.push(rec).unwrap();
    }
    assert_eq!(bytes, only_valid.finish(end(), Nanos(1)).unwrap());
    let report = validate(bytes.as_slice());
    assert!(report.is_valid(), "{:?}", report.issues);
    assert_eq!(
        report.counts.keys().collect::<Vec<_>>(),
        [
            "annotation",
            "input_motion",
            "session_header",
            "source_declare"
        ]
    );
}

#[test]
fn large_kinds_split_into_chunks_of_at_most_1_mib() {
    let mut r = Recorder::new(
        &header(),
        FlushPolicy {
            chunk_flush_ns: i64::MAX,
            max_records: u32::MAX,
        },
    )
    .unwrap();
    let n = 40_000u64; // 40 000 × 40 B frames = 1.6 MB
    for i in 0..n {
        r.push(Record::Frame(Frame {
            frame: i,
            t_start_ns: Nanos(0),
            t_input_cutoff_ns: Nanos(0),
            first_tick: 0,
            ticks: 0,
            flags: 0,
        }))
        .unwrap();
    }
    assert!(!r.due(Nanos(1_000_000_000_000)));
    let bytes = r.finish(end(), Nanos(5)).unwrap();
    let chunks: Vec<_> = ChunkReader::new(bytes.as_slice())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let frame_chunks: Vec<_> = chunks
        .iter()
        .filter(|c| c.header.kind == Kind::Frame as u8)
        .collect();
    assert_eq!(frame_chunks.len(), 2);
    assert!(
        frame_chunks
            .iter()
            .all(|c| c.header.payload_len <= MAX_PAYLOAD)
    );
    assert_eq!(
        frame_chunks
            .iter()
            .map(|c| u64::from(c.header.record_count))
            .sum::<u64>(),
        n
    );
}

#[test]
fn flush_policy() {
    let mut r = Recorder::new(
        &header(),
        FlushPolicy {
            chunk_flush_ns: 100,
            max_records: 3,
        },
    )
    .unwrap();
    assert!(!r.due(Nanos(99)) && r.due(Nanos(100)));
    let first = r.cut(Nanos(100));
    assert!(first.starts_with(&kf_telemetry::format::FILE_HEADER));
    assert!(!r.due(Nanos(10_000)), "nothing buffered");
    assert!(r.cut(Nanos(10_001)).is_empty());
    for seq in 0..3 {
        r.push(Record::Motion(Motion {
            t_ns: Nanos(1),
            src_ts_ns: None,
            dx: 0,
            dy: 0,
            seq,
            source: 0,
            flags: 0,
        }))
        .unwrap();
    }
    assert!(r.due(Nanos(10_002)), "max_records reached");
    // A cut resets both triggers.
    assert!(!r.cut(Nanos(10_002)).is_empty());
    assert!(!r.due(Nanos(10_003)));
    assert!(r.cut(Nanos(10_003)).is_empty());
    r.push(Record::Motion(Motion {
        t_ns: Nanos(1),
        src_ts_ns: None,
        dx: 0,
        dy: 0,
        seq: 3,
        source: 0,
        flags: 0,
    }))
    .unwrap();
    // The interval runs from the last cut call, even an empty one.
    assert!(!r.due(Nanos(10_102)) && r.due(Nanos(10_103)));
}

/// A crash may cut the file anywhere: at every length, reading and
/// recovery must not panic, and a recovered stream must validate.
#[test]
fn every_truncation_reads_and_recovers() {
    let full = golden::full();
    let mut recovered = 0;
    for len in 0..=full.len() {
        let cut = &full[..len];
        let _ = validate(cut);
        let mut sink = Vec::new();
        kf_telemetry::dump::dump(cut, &mut sink).unwrap();
        match recover(cut) {
            Outcome::Recovered(s) => {
                recovered += 1;
                let mut stream = Vec::new();
                std::io::copy(&mut recovered_stream(cut, &s).unwrap(), &mut stream).unwrap();
                let rep = validate(stream.as_slice());
                assert!(
                    rep.complete && rep.is_valid(),
                    "len {len}: {:?}",
                    rep.issues
                );
            }
            Outcome::Complete { .. } => assert!(len >= full.len() - 300, "len {len}"),
            Outcome::Unrecoverable(_) => {}
        }
    }
    // Every cut after the first commit and before the end is recoverable.
    let first_commit_end = GroupReader::new(full.as_slice())
        .unwrap()
        .next()
        .unwrap()
        .end;
    assert_eq!(recovered, full.len() as u64 - first_commit_end);
}

/// Lengths near u32::MAX must not overflow usize on 32-bit targets (wasm32):
/// both inputs were panics there before the fix.
#[test]
fn huge_counts_are_errors_not_overflows() {
    use kf_telemetry::record::decode_payload;
    let mut commit = [0u8; 32];
    commit[24..28].copy_from_slice(&0x2000_0000u32.to_le_bytes());
    assert!(decode_payload(Kind::Commit, 1, &commit).is_err());
    let mut ann = [0u8; 24];
    ann[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_payload(Kind::Annotation, 1, &ann).is_err());
    assert!(decode_payload(Kind::InputMotion, u32::MAX, &[0u8; 32]).is_err());
}

/// The decoder refuses every payload §6 makes undecodable.
#[test]
fn decoder_rejects_non_conforming_payloads() {
    use kf_telemetry::record::decode_payload;
    let ok = |kind, n, p: &[u8]| decode_payload(kind, n, p).is_ok();
    // An all-zero record decodes; one non-zero byte at `bad` does not.
    for (kind, bad, value) in [
        (Kind::Frame, 38, 1),
        (Kind::Drain, 10, 1),
        (Kind::SimTarget, 14, 1),
        (Kind::SimShot, 49, 1),
        (Kind::SimShot, 48, 2), // hit
        (Kind::SimControl, 30, 1),
        (Kind::ClockSync, 10, 1),
    ] {
        let Layout::Fixed(size) = kind.layout() else {
            panic!()
        };
        let mut p = vec![0u8; size];
        assert!(ok(kind, 1, &p), "{kind:?}");
        p[bad] = value;
        assert!(!ok(kind, 1, &p), "{kind:?} byte {bad} = {value}");
    }
    assert!(ok(Kind::InputMotion, 2, &[0; 64]));
    assert!(!ok(Kind::InputMotion, 1, &[0; 64]), "size identity");

    // A commit with one source.
    let mut commit = vec![0u8; 40];
    commit[24] = 1;
    assert!(ok(Kind::Commit, 1, &commit));
    for bad in [28, 34] {
        let mut p = commit.clone();
        p[bad] = 1;
        assert!(!ok(Kind::Commit, 1, &p), "commit padding byte {bad}");
    }
    let mut trailing = commit.clone();
    trailing.extend([0; 8]);
    assert!(!ok(Kind::Commit, 1, &trailing), "commit trailing bytes");

    // An annotation with text "abc".
    let mut ann = vec![0u8; 24];
    ann[20] = 3;
    ann.extend(*b"abc\0\0\0\0\0");
    assert!(ok(Kind::Annotation, 1, &ann) && ok(Kind::Annotation, 2, &ann.repeat(2)));
    assert!(!ok(Kind::Annotation, 1, &ann.repeat(2)), "record_count");
    let mut p = ann.clone();
    p[27] = 1;
    assert!(!ok(Kind::Annotation, 1, &p), "text padding");

    assert!(ok(Kind::DeviceInfo, 1, b"{}      "));
    for json in [b"[1]     ", b"1       "] {
        assert!(!ok(Kind::DeviceInfo, 1, json), "not an object");
    }
}

/// The writer kept only the first `kept` groups: after `writer_lost` the
/// file (those groups + the final bytes) validates, with the loss annotated
/// as storage_error gaps, the commit watermarks and counts rolled back and
/// `dropped_chunks` counted.
#[test]
fn writer_lost_rolls_back_to_the_file() {
    use kf_telemetry::json::end_reason;
    let motion = |seq: u32| {
        Record::Motion(Motion {
            t_ns: Nanos(i64::from(seq)),
            src_ts_ns: None,
            dx: 1,
            dy: 0,
            seq,
            source: 0,
            flags: 0,
        })
    };
    for kept in 0..=3usize {
        let mut r = Recorder::new(&header(), FlushPolicy::default()).unwrap();
        r.push(declare()).unwrap();
        let mut groups = Vec::new();
        for g in 0..3u32 {
            for i in 0..5 {
                r.push(motion(g * 5 + i)).unwrap();
            }
            r.push(Record::Frame(Frame {
                frame: u64::from(g),
                t_start_ns: Nanos(i64::from(g * 5)),
                t_input_cutoff_ns: Nanos(i64::from(g * 5 + 4)),
                first_tick: u64::from(2 * g),
                ticks: 2,
                flags: 0,
            }))
            .unwrap();
            groups.push(r.cut(Nanos(i64::from(g * 5 + 4))));
        }
        let file_len = groups[..kept].iter().map(Vec::len).sum::<usize>() as u64;
        if kept < 3 {
            // Buffered after the last cut: lost too.
            for seq in 15..18 {
                r.push(motion(seq)).unwrap();
            }
            // The recorder thread confirmed the kept groups before the drop.
            r.written(file_len);
            assert!(r.writer_lost(file_len + 8, Nanos(100)).is_err());
            assert_eq!(r.writer_lost(file_len, Nanos(100)), Ok(()));
        }
        let mut end = end();
        end.t_end_ns = 101;
        end.completed = kept == 3;
        end.end_reason = if kept == 3 {
            end_reason::COMPLETED
        } else {
            end_reason::STORAGE_ERROR
        }
        .into();
        let mut bytes = groups[..kept].concat();
        bytes.extend(r.finish(end, Nanos(101)).unwrap());
        let rep = validate(bytes.as_slice());
        assert!(
            rep.is_valid() && rep.complete,
            "kept {kept}: {:?}",
            rep.issues
        );
        let end = rep.session_end.unwrap();
        // Each lost group held a motion chunk, a frame chunk and a commit
        // (the first also the header and the declaration).
        let dropped = match kept {
            0 => 2 + 3 * 3,
            1 => 6,
            2 => 3,
            _ => 0,
        };
        assert_eq!(end.dropped_chunks, dropped, "kept {kept}");
        let motions = rep.counts.get("input_motion").copied().unwrap_or(0);
        assert_eq!(motions, 5 * kept as u64);
        let read: Vec<_> = GroupReader::new(bytes.as_slice()).unwrap().collect();
        // The final commit claims only the frames in the file.
        let c = &read.last().unwrap().commit;
        let last = kept.checked_sub(1).map(|g| g as u64);
        assert_eq!((c.frame, c.tick), (last, last.map(|g| 2 * g + 1)));
        // A gap from each source's first lost seq, and one without a source
        // for sources whose records were all lost.
        let gaps: Vec<_> = read
            .iter()
            .flat_map(|g| g.records())
            .filter_map(|r| match r {
                Record::Annotation(a) => Some((a.source, a.first_seq)),
                _ => None,
            })
            .collect();
        let want = match kept {
            0 => vec![(None, 0)],
            3 => vec![],
            _ => vec![(Some(0), 5 * kept as u32), (None, 0)],
        };
        assert_eq!(gaps, want, "kept {kept}");
    }
}
