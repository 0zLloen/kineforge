//! Canonical JSON-lines dump of a log's committed records: the format the
//! stdlib Python reader (`tools/read_kflog.py --jsonl`) also produces, so
//! the two readers can be compared on golden files. One object per line, in
//! file order; field names are the DATA_SPECIFICATION.md §6 names; sentinels
//! are `null`. The last line describes how the file ended.

use crate::reader::{Chunk, Ending, GroupReader, ReadError};
use crate::record::Record;
use serde_json::{Value, json};
use std::io::{self, Read, Write};

fn ts(t: Option<kf_core::Nanos>) -> Value {
    t.map_or(Value::Null, |t| json!(t.0))
}

/// The JSON object for one record.
pub fn record_json(rec: &Record) -> Value {
    let kind = rec.kind().name();
    match rec {
        Record::Json { value, .. } => json!({"kind": kind, "value": value}),
        Record::Motion(m) => json!({"kind": kind, "t_ns": m.t_ns.0, "src_ts_ns": ts(m.src_ts_ns),
            "dx": m.dx, "dy": m.dy, "seq": m.seq, "source": m.source, "flags": m.flags}),
        Record::Button(b) => json!({"kind": kind, "t_ns": b.t_ns.0, "src_ts_ns": ts(b.src_ts_ns),
            "seq": b.seq, "source": b.source, "button": b.button, "state": b.state}),
        Record::Wheel(w) => json!({"kind": kind, "t_ns": w.t_ns.0, "src_ts_ns": ts(w.src_ts_ns),
            "dv": w.dv, "dh": w.dh, "seq": w.seq, "source": w.source, "flags": w.flags}),
        Record::Frame(f) => json!({"kind": kind, "frame": f.frame, "t_start_ns": f.t_start_ns.0,
            "t_input_cutoff_ns": f.t_input_cutoff_ns.0, "first_tick": f.first_tick,
            "ticks": f.ticks, "flags": f.flags}),
        Record::Drain(d) => {
            json!({"kind": kind, "frame": d.frame, "source": d.source, "last_seq": d.last_seq})
        }
        Record::FrameTiming(t) => {
            json!({"kind": kind, "frame": t.frame, "t_submit_ns": t.t_submit_ns.0,
            "t_present_ns": ts(t.t_present_ns), "cpu_ns": t.cpu_ns, "gpu_ns": t.gpu_ns})
        }
        Record::SimCamera(c) => json!({"kind": kind, "tick": c.tick, "yaw_units": c.yaw_units,
            "pitch_units": c.pitch_units, "yaw_rad": c.yaw_rad, "pitch_rad": c.pitch_rad}),
        Record::SimTarget(t) => {
            json!({"kind": kind, "tick": t.tick, "target": t.target, "event": t.event,
            "shape": t.shape, "pos_m": t.pos_m, "vel_m_s": t.vel_m_s, "radius_m": t.radius_m})
        }
        Record::SimShot(s) => {
            json!({"kind": kind, "tick": s.tick, "t_ns": s.t_ns.0, "target": s.target,
            "seq": s.seq, "ang_err_rad": s.ang_err_rad, "d_yaw_rad": s.d_yaw_rad,
            "d_pitch_rad": s.d_pitch_rad, "hit": s.hit})
        }
        Record::SimControl(c) => {
            json!({"kind": kind, "tick": c.tick, "t_ns": c.t_ns.0, "frame": c.frame,
            "action": c.action, "phase": c.phase, "reason": c.reason, "new_source": c.new_source})
        }
        Record::Commit(c) => {
            json!({"kind": kind, "t_ns": c.t_ns.0, "tick": c.tick, "frame": c.frame,
            "sources": c.sources.iter().map(|&(s, q)| json!([s, q])).collect::<Vec<_>>()})
        }
        Record::ClockSync(c) => json!({"kind": kind, "t_ns": c.t_ns.0, "domain": c.domain,
            "src_ts_ns": c.src_ts_ns.0, "uncertainty_ns": c.uncertainty_ns}),
        Record::Annotation(a) => {
            json!({"kind": kind, "t_ns": a.t_ns.0, "code": a.code, "source": a.source,
            "first_seq": a.first_seq, "lost": a.lost, "text": a.text})
        }
    }
}

/// Stable names for why reading stopped.
pub fn stop_json(e: &ReadError) -> Value {
    let (reason, offset) = match e {
        ReadError::Io(_) => ("io", None),
        ReadError::BadMagic => ("bad_magic", None),
        ReadError::UnsupportedVersion(_) => ("unsupported_version", None),
        ReadError::NonZeroReserved { offset } => ("non_zero_reserved", Some(*offset)),
        ReadError::Truncated { offset } => ("truncated", Some(*offset)),
        ReadError::PayloadTooLarge { offset, .. } => ("payload_too_large", Some(*offset)),
        ReadError::PayloadMisaligned { offset, .. } => ("payload_misaligned", Some(*offset)),
        ReadError::CrcMismatch { offset } => ("crc_mismatch", Some(*offset)),
        ReadError::NonZeroTrailer { offset } => ("non_zero_trailer", Some(*offset)),
    };
    json!({"reason": reason, "offset": offset})
}

fn chunk_lines(chunk: &Chunk, out: &mut impl Write) -> io::Result<()> {
    match chunk {
        Chunk::Records { records, .. } => {
            for rec in records {
                writeln!(out, "{}", record_json(rec))?;
            }
        }
        Chunk::Skipped { offset, header } => writeln!(
            out,
            "{}",
            json!({"kind": "skipped", "offset": offset, "kind_code": header.kind,
                "kind_version": header.kind_version, "flags": header.flags,
                "record_count": header.record_count, "payload_len": header.payload_len})
        )?,
        Chunk::Undecodable { offset, error } => writeln!(
            out,
            "{}",
            json!({"kind": "undecodable", "offset": offset, "kind_code": error.kind as u8})
        )?,
    }
    Ok(())
}

/// The final line: how the file ended.
pub fn ending_json(e: &Ending) -> Value {
    json!({
        "kind": "ending",
        "complete": e.session_end.is_some(),
        "committed_end": e.committed_end,
        "valid_end": e.valid_end,
        "uncommitted_chunks": e.uncommitted.len(),
        "after_end_chunks": e.after_end.len(),
        "stop": e.error.as_ref().map(stop_json),
    })
}

/// Writes the dump. Committed records only, then the terminal
/// `session_end` (if any), then the ending line.
pub fn dump<R: Read, W: Write>(r: R, out: &mut W) -> io::Result<()> {
    let mut groups = match GroupReader::new(r) {
        Ok(g) => g,
        Err(e) => {
            writeln!(
                out,
                "{}",
                json!({"kind": "ending", "complete": false, "stop": stop_json(&e)})
            )?;
            return Ok(());
        }
    };
    for g in groups.by_ref() {
        for c in &g.chunks {
            chunk_lines(c, out)?;
        }
        writeln!(out, "{}", record_json(&Record::Commit(g.commit)))?;
    }
    let (ending, _) = groups.ending();
    if let Some(end) = &ending.session_end {
        writeln!(out, "{}", json!({"kind": "session_end", "value": end}))?;
    }
    writeln!(out, "{}", ending_json(&ending))
}
