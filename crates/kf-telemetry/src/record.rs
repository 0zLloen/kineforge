//! Record payload codec (DATA_SPECIFICATION.md §6). Every fixed layout is
//! written field by field, little-endian, with zero padding; on-disk
//! sentinels (`i64::MIN`, `u32::MAX`, `u64::MAX`, `0xFFFF`) map to `None`.

use crate::format::{Kind, Layout, MAX_PAYLOAD};
use kf_core::{
    Annotation, Button, ClockSync, Drain, Frame, FrameTiming, Motion, Nanos, SimCamera, SimControl,
    SimShot, SimTarget, SourceId, Wheel,
};
use std::fmt;

/// Commit watermark (DATA_SPECIFICATION.md §5.2): written last in every group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// Session time when the group was cut.
    pub t_ns: Nanos,
    /// Last tick of the last complete frame so far; `None` before any.
    pub tick: Option<u64>,
    /// That frame; `None` before any.
    pub frame: Option<u64>,
    /// Per source, the last `seq` written so far, ascending by source id.
    pub sources: Vec<(SourceId, u32)>,
}

/// One decoded record of any kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// A JSON kind (`session_header`, `source_declare`, `device_info`,
    /// `render_state`, `app_result`, `session_end`); always an object.
    Json {
        kind: Kind,
        value: serde_json::Value,
    },
    Motion(Motion),
    Button(Button),
    Wheel(Wheel),
    Frame(Frame),
    Drain(Drain),
    FrameTiming(FrameTiming),
    SimCamera(SimCamera),
    SimTarget(SimTarget),
    SimShot(SimShot),
    SimControl(SimControl),
    Commit(Commit),
    ClockSync(ClockSync),
    Annotation(Annotation),
}

impl Record {
    pub fn kind(&self) -> Kind {
        match self {
            Record::Json { kind, .. } => *kind,
            Record::Motion(_) => Kind::InputMotion,
            Record::Button(_) => Kind::InputButton,
            Record::Wheel(_) => Kind::InputWheel,
            Record::Frame(_) => Kind::Frame,
            Record::Drain(_) => Kind::Drain,
            Record::FrameTiming(_) => Kind::FrameTiming,
            Record::SimCamera(_) => Kind::SimCamera,
            Record::SimTarget(_) => Kind::SimTarget,
            Record::SimShot(_) => Kind::SimShot,
            Record::SimControl(_) => Kind::SimControl,
            Record::Commit(_) => Kind::Commit,
            Record::ClockSync(_) => Kind::ClockSync,
            Record::Annotation(_) => Kind::Annotation,
        }
    }

    /// `(source, seq)` for input records, which share one counter per source.
    pub fn source_seq(&self) -> Option<(SourceId, u32)> {
        match self {
            Record::Motion(m) => Some((m.source, m.seq)),
            Record::Button(b) => Some((b.source, b.seq)),
            Record::Wheel(w) => Some((w.source, w.seq)),
            _ => None,
        }
    }

    /// Receipt time of input records.
    pub fn input_t_ns(&self) -> Option<Nanos> {
        match self {
            Record::Motion(m) => Some(m.t_ns),
            Record::Button(b) => Some(b.t_ns),
            Record::Wheel(w) => Some(w.t_ns),
            _ => None,
        }
    }

    /// Every f64 field, for the "finite floats only" rule.
    pub fn floats(&self) -> Vec<f64> {
        match self {
            Record::SimCamera(c) => vec![c.yaw_rad, c.pitch_rad],
            Record::SimTarget(t) => {
                let mut v = t.pos_m.to_vec();
                v.extend_from_slice(&t.vel_m_s);
                v.push(t.radius_m);
                v
            }
            Record::SimShot(s) => vec![s.ang_err_rad, s.d_yaw_rad, s.d_pitch_rad],
            _ => Vec::new(),
        }
    }
}

/// A record that cannot be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodeError {
    pub kind: Kind,
    pub what: &'static str,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot encode {}: {}", self.kind.name(), self.what)
    }
}

impl std::error::Error for EncodeError {}

/// A CRC-valid chunk whose payload does not decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub kind: Kind,
    pub what: String,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad {} payload: {}", self.kind.name(), self.what)
    }
}

impl std::error::Error for DecodeError {}

fn some_or<T: PartialEq>(
    v: Option<T>,
    sentinel: T,
    kind: Kind,
    what: &'static str,
) -> Result<T, EncodeError> {
    match v {
        Some(x) if x == sentinel => Err(EncodeError { kind, what }),
        Some(x) => Ok(x),
        None => Ok(sentinel),
    }
}

fn finite(kind: Kind, xs: &[f64]) -> Result<(), EncodeError> {
    if xs.iter().any(|x| !x.is_finite()) {
        Err(EncodeError {
            kind,
            what: "NaN and infinities are invalid",
        })
    } else {
        Ok(())
    }
}

/// Appends the fixed-layout bytes of a fixed-size record.
pub(crate) fn encode_fixed(rec: &Record, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let kind = rec.kind();
    let start = out.len();
    let mut put = |b: &[u8]| out.extend_from_slice(b);
    const NO_TS: &str = "Some(i64::MIN) timestamp";
    match rec {
        Record::Motion(m) => {
            put(&m.t_ns.0.to_le_bytes());
            put(&some_or(m.src_ts_ns.map(|t| t.0), i64::MIN, kind, NO_TS)?.to_le_bytes());
            put(&m.dx.to_le_bytes());
            put(&m.dy.to_le_bytes());
            put(&m.seq.to_le_bytes());
            put(&m.source.to_le_bytes());
            put(&m.flags.to_le_bytes());
        }
        Record::Button(b) => {
            put(&b.t_ns.0.to_le_bytes());
            put(&some_or(b.src_ts_ns.map(|t| t.0), i64::MIN, kind, NO_TS)?.to_le_bytes());
            put(&b.seq.to_le_bytes());
            put(&b.source.to_le_bytes());
            put(&[b.button, b.state]);
        }
        Record::Wheel(w) => {
            put(&w.t_ns.0.to_le_bytes());
            put(&some_or(w.src_ts_ns.map(|t| t.0), i64::MIN, kind, NO_TS)?.to_le_bytes());
            put(&w.dv.to_le_bytes());
            put(&w.dh.to_le_bytes());
            put(&w.seq.to_le_bytes());
            put(&w.source.to_le_bytes());
            put(&w.flags.to_le_bytes());
        }
        Record::Frame(f) => {
            put(&f.frame.to_le_bytes());
            put(&f.t_start_ns.0.to_le_bytes());
            put(&f.t_input_cutoff_ns.0.to_le_bytes());
            put(&f.first_tick.to_le_bytes());
            put(&f.ticks.to_le_bytes());
            put(&f.flags.to_le_bytes());
            put(&[0; 2]);
        }
        Record::Drain(d) => {
            put(&d.frame.to_le_bytes());
            put(&d.source.to_le_bytes());
            put(&[0; 2]);
            put(&d.last_seq.to_le_bytes());
        }
        Record::FrameTiming(t) => {
            put(&t.frame.to_le_bytes());
            put(&t.t_submit_ns.0.to_le_bytes());
            put(&some_or(t.t_present_ns.map(|t| t.0), i64::MIN, kind, NO_TS)?.to_le_bytes());
            put(&t.cpu_ns.to_le_bytes());
            put(&some_or(t.gpu_ns, u32::MAX, kind, "Some(u32::MAX) gpu_ns")?.to_le_bytes());
        }
        Record::SimCamera(c) => {
            finite(kind, &[c.yaw_rad, c.pitch_rad])?;
            put(&c.tick.to_le_bytes());
            put(&c.yaw_units.to_le_bytes());
            put(&c.pitch_units.to_le_bytes());
            put(&c.yaw_rad.to_le_bytes());
            put(&c.pitch_rad.to_le_bytes());
        }
        Record::SimTarget(t) => {
            finite(kind, &rec.floats())?;
            put(&t.tick.to_le_bytes());
            put(&t.target.to_le_bytes());
            put(&[t.event, t.shape, 0, 0]);
            for x in t.pos_m.iter().chain(&t.vel_m_s) {
                put(&x.to_le_bytes());
            }
            put(&t.radius_m.to_le_bytes());
        }
        Record::SimShot(s) => {
            finite(kind, &rec.floats())?;
            put(&s.tick.to_le_bytes());
            put(&s.t_ns.0.to_le_bytes());
            put(&some_or(s.target, u32::MAX, kind, "Some(u32::MAX) target")?.to_le_bytes());
            put(&s.seq.to_le_bytes());
            put(&s.ang_err_rad.to_le_bytes());
            put(&s.d_yaw_rad.to_le_bytes());
            put(&s.d_pitch_rad.to_le_bytes());
            put(&[u8::from(s.hit), 0, 0, 0, 0, 0, 0, 0]);
        }
        Record::SimControl(c) => {
            put(&c.tick.to_le_bytes());
            put(&c.t_ns.0.to_le_bytes());
            put(&c.frame.to_le_bytes());
            put(&[c.action, c.phase]);
            put(&c.reason.to_le_bytes());
            put(&some_or(c.new_source, 0xFFFF, kind, "Some(0xFFFF) new_source")?.to_le_bytes());
            put(&[0; 2]);
        }
        Record::ClockSync(c) => {
            put(&c.t_ns.0.to_le_bytes());
            put(&c.domain.to_le_bytes());
            put(&[0; 6]);
            put(&c.src_ts_ns.0.to_le_bytes());
            put(&c.uncertainty_ns.to_le_bytes());
        }
        Record::Json { .. } | Record::Commit(_) | Record::Annotation(_) => {
            return Err(EncodeError {
                kind,
                what: "not a fixed-size kind",
            });
        }
    }
    debug_assert_eq!(Layout::Fixed(out.len() - start), kind.layout());
    Ok(())
}

/// Appends one self-framed annotation record (24-byte header, UTF-8 text,
/// zero padding to 8).
pub(crate) fn encode_annotation(a: &Annotation, out: &mut Vec<u8>) -> Result<(), EncodeError> {
    let kind = Kind::Annotation;
    if a.text.len() > MAX_PAYLOAD as usize - 24 {
        return Err(EncodeError {
            kind,
            what: "text longer than a chunk",
        });
    }
    out.extend_from_slice(&a.t_ns.0.to_le_bytes());
    out.extend_from_slice(&a.code.to_le_bytes());
    out.extend_from_slice(&some_or(a.source, 0xFFFF, kind, "Some(0xFFFF) source")?.to_le_bytes());
    out.extend_from_slice(&a.first_seq.to_le_bytes());
    out.extend_from_slice(&some_or(a.lost, u32::MAX, kind, "Some(u32::MAX) lost")?.to_le_bytes());
    out.extend_from_slice(&(a.text.len() as u32).to_le_bytes());
    out.extend_from_slice(a.text.as_bytes());
    out.resize(out.len().next_multiple_of(8), 0);
    Ok(())
}

/// Bytes of one encoded annotation record.
pub(crate) fn annotation_len(a: &Annotation) -> usize {
    (24 + a.text.len()).next_multiple_of(8)
}

/// The commit payload: 32 + 8n bytes.
pub(crate) fn encode_commit(c: &Commit) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 8 * c.sources.len());
    out.extend_from_slice(&c.t_ns.0.to_le_bytes());
    out.extend_from_slice(&c.tick.unwrap_or(u64::MAX).to_le_bytes());
    out.extend_from_slice(&c.frame.unwrap_or(u64::MAX).to_le_bytes());
    out.extend_from_slice(&(c.sources.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    for &(source, last_seq) in &c.sources {
        out.extend_from_slice(&source.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&last_seq.to_le_bytes());
    }
    out
}

/// A JSON object, padded with spaces to a multiple of 8.
pub(crate) fn encode_json(kind: Kind, value: &serde_json::Value) -> Result<Vec<u8>, EncodeError> {
    if !value.is_object() {
        return Err(EncodeError {
            kind,
            what: "JSON record is not an object",
        });
    }
    let mut out = serde_json::to_vec(value).map_err(|_| EncodeError {
        kind,
        what: "JSON serialisation failed",
    })?;
    out.resize(out.len().next_multiple_of(8), b' ');
    if out.len() > MAX_PAYLOAD as usize {
        return Err(EncodeError {
            kind,
            what: "JSON document larger than a chunk",
        });
    }
    Ok(out)
}

/// Little-endian cursor over a payload whose length was already checked.
struct Le<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Le<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        s
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().unwrap())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn ts(&mut self) -> Option<Nanos> {
        Some(self.i64()).filter(|&t| t != i64::MIN).map(Nanos)
    }
    /// `n` padding bytes that must be zero.
    fn pad(&mut self, n: usize) -> bool {
        self.take(n).iter().all(|&b| b == 0)
    }
}

fn opt_u32(v: u32) -> Option<u32> {
    Some(v).filter(|&v| v != u32::MAX)
}

fn opt_source(v: u16) -> Option<SourceId> {
    Some(v).filter(|&v| v != 0xFFFF)
}

/// Decodes one fixed-size record (`b.len()` is the kind's size).
fn decode_fixed(kind: Kind, b: &[u8]) -> Result<Record, &'static str> {
    let mut r = Le { b, i: 0 };
    let rec = match kind {
        Kind::InputMotion => Record::Motion(Motion {
            t_ns: Nanos(r.i64()),
            src_ts_ns: r.ts(),
            dx: r.i32(),
            dy: r.i32(),
            seq: r.u32(),
            source: r.u16(),
            flags: r.u16(),
        }),
        Kind::InputButton => Record::Button(Button {
            t_ns: Nanos(r.i64()),
            src_ts_ns: r.ts(),
            seq: r.u32(),
            source: r.u16(),
            button: r.u8(),
            state: r.u8(),
        }),
        Kind::InputWheel => Record::Wheel(Wheel {
            t_ns: Nanos(r.i64()),
            src_ts_ns: r.ts(),
            dv: r.i32(),
            dh: r.i32(),
            seq: r.u32(),
            source: r.u16(),
            flags: r.u16(),
        }),
        Kind::Frame => {
            let f = Frame {
                frame: r.u64(),
                t_start_ns: Nanos(r.i64()),
                t_input_cutoff_ns: Nanos(r.i64()),
                first_tick: r.u64(),
                ticks: r.u32(),
                flags: r.u16(),
            };
            if !r.pad(2) {
                return Err("non-zero padding");
            }
            Record::Frame(f)
        }
        Kind::Drain => {
            let frame = r.u64();
            let source = r.u16();
            if !r.pad(2) {
                return Err("non-zero padding");
            }
            Record::Drain(Drain {
                frame,
                source,
                last_seq: r.u32(),
            })
        }
        Kind::FrameTiming => Record::FrameTiming(FrameTiming {
            frame: r.u64(),
            t_submit_ns: Nanos(r.i64()),
            t_present_ns: r.ts(),
            cpu_ns: r.u32(),
            gpu_ns: opt_u32(r.u32()),
        }),
        Kind::SimCamera => Record::SimCamera(SimCamera {
            tick: r.u64(),
            yaw_units: r.i64(),
            pitch_units: r.i64(),
            yaw_rad: r.f64(),
            pitch_rad: r.f64(),
        }),
        Kind::SimTarget => {
            let tick = r.u64();
            let target = r.u32();
            let (event, shape) = (r.u8(), r.u8());
            if !r.pad(2) {
                return Err("non-zero padding");
            }
            Record::SimTarget(SimTarget {
                tick,
                target,
                event,
                shape,
                pos_m: [r.f64(), r.f64(), r.f64()],
                vel_m_s: [r.f64(), r.f64(), r.f64()],
                radius_m: r.f64(),
            })
        }
        Kind::SimShot => {
            let s = SimShot {
                tick: r.u64(),
                t_ns: Nanos(r.i64()),
                target: opt_u32(r.u32()),
                seq: r.u32(),
                ang_err_rad: r.f64(),
                d_yaw_rad: r.f64(),
                d_pitch_rad: r.f64(),
                hit: match r.u8() {
                    0 => false,
                    1 => true,
                    _ => return Err("hit is neither 0 nor 1"),
                },
            };
            if !r.pad(7) {
                return Err("non-zero padding");
            }
            Record::SimShot(s)
        }
        Kind::SimControl => {
            let c = SimControl {
                tick: r.u64(),
                t_ns: Nanos(r.i64()),
                frame: r.u64(),
                action: r.u8(),
                phase: r.u8(),
                reason: r.u16(),
                new_source: opt_source(r.u16()),
            };
            if !r.pad(2) {
                return Err("non-zero padding");
            }
            Record::SimControl(c)
        }
        Kind::ClockSync => {
            let t_ns = Nanos(r.i64());
            let domain = r.u16();
            if !r.pad(6) {
                return Err("non-zero padding");
            }
            Record::ClockSync(ClockSync {
                t_ns,
                domain,
                src_ts_ns: Nanos(r.i64()),
                uncertainty_ns: r.i64(),
            })
        }
        _ => unreachable!("not a fixed kind"),
    };
    Ok(rec)
}

/// Decodes a CRC-valid v1 payload of a known kind into its records.
pub fn decode_payload(
    kind: Kind,
    record_count: u32,
    payload: &[u8],
) -> Result<Vec<Record>, DecodeError> {
    let err = |what: String| DecodeError { kind, what };
    let n = record_count as usize;
    match kind.layout() {
        Layout::Fixed(size) => {
            if n.checked_mul(size) != Some(payload.len()) {
                return Err(err(format!(
                    "payload_len {} != record_count {n} × {size}",
                    payload.len()
                )));
            }
            payload
                .chunks_exact(size)
                .enumerate()
                .map(|(i, b)| decode_fixed(kind, b).map_err(|e| err(format!("record {i}: {e}"))))
                .collect()
        }
        Layout::Json => {
            if n != 1 {
                return Err(err(format!("record_count {n} != 1")));
            }
            match serde_json::from_slice::<serde_json::Value>(payload) {
                Ok(value) if value.is_object() => Ok(vec![Record::Json { kind, value }]),
                Ok(_) => Err(err("JSON is not an object".into())),
                Err(e) => Err(err(format!("invalid JSON: {e}"))),
            }
        }
        Layout::Commit => {
            if n != 1 || payload.len() < 32 {
                return Err(err("commit needs record_count 1 and ≥ 32 bytes".into()));
            }
            let mut r = Le { b: payload, i: 0 };
            let t_ns = Nanos(r.i64());
            let tick = Some(r.u64()).filter(|&t| t != u64::MAX);
            let frame = Some(r.u64()).filter(|&f| f != u64::MAX);
            // u64: on 32-bit targets 8 × count overflows usize.
            let count = u64::from(r.u32());
            if !r.pad(4) {
                return Err(err("non-zero padding".into()));
            }
            if payload.len() as u64 != 32 + 8 * count {
                return Err(err(format!(
                    "payload_len {} != 32 + 8 × {count}",
                    payload.len()
                )));
            }
            // Bounded: count ≤ payload_len / 8 ≤ 1 MiB / 8.
            let mut sources = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let source = r.u16();
                if !r.pad(2) {
                    return Err(err("non-zero padding".into()));
                }
                sources.push((source, r.u32()));
            }
            Ok(vec![Record::Commit(Commit {
                t_ns,
                tick,
                frame,
                sources,
            })])
        }
        Layout::Annotation => {
            let mut r = Le { b: payload, i: 0 };
            let mut out = Vec::new();
            while r.i < payload.len() {
                if payload.len() - r.i < 24 {
                    return Err(err("truncated annotation header".into()));
                }
                let t_ns = Nanos(r.i64());
                let code = r.u16();
                let source = opt_source(r.u16());
                let first_seq = r.u32();
                let lost = opt_u32(r.u32());
                // u64: on 32-bit targets rounding u32::MAX up overflows usize.
                let len = u64::from(r.u32());
                let padded = len.next_multiple_of(8);
                if padded > (payload.len() - r.i) as u64 {
                    return Err(err("annotation text runs past the payload".into()));
                }
                let (len, padded) = (len as usize, padded as usize);
                let text = std::str::from_utf8(r.take(len))
                    .map_err(|_| err("annotation text is not UTF-8".into()))?
                    .to_owned();
                if !r.pad(padded - len) {
                    return Err(err("non-zero padding".into()));
                }
                out.push(Record::Annotation(Annotation {
                    t_ns,
                    code,
                    source,
                    first_seq,
                    lost,
                    text,
                }));
            }
            if out.len() != n {
                return Err(err(format!(
                    "{} annotations but record_count {n}",
                    out.len()
                )));
            }
            Ok(out)
        }
    }
}
