//! The validator (DATA_SPECIFICATION.md §10). It reads a log with
//! [`GroupReader`] and checks structure, declarations, watermarks, per-source
//! order and loss, non-finite floats, the terminal `session_end`, and, when the header has
//! a `simulation`, the simulation rules that need no replay (tick
//! continuity, drains of the driving source, source switches, camera
//! radians recomputed bit for bit). Replay hash equality needs `kf-sim` (M5).
//!
//! A recovered session is validated as its recovered byte stream (prefix ‖
//! synthetic end, see [`crate::recover`]).

use crate::format::Kind;
use crate::json::{
    SCHEMA_PREFIX, SessionEnd, SessionHeader, SourceDeclaration, from_value, is_uuid,
};
use crate::reader::{Chunk, GroupReader, ReadError};
use crate::record::Record;
use kf_core::records::{control_action, target_event, target_shape};
use kf_core::{LinearSensitivity, SourceId};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    /// Byte offset of the chunk concerned, when there is one.
    pub offset: Option<u64>,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Warning => "warning",
            Level::Error => "error",
        };
        match self.offset {
            Some(o) => write!(f, "{level} @{o}: {}", self.message),
            None => write!(f, "{level}: {}", self.message),
        }
    }
}

/// A run of missing `seq` values in one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loss {
    pub source: SourceId,
    /// First missing seq (unwrapped to 64 bits).
    pub first_seq: u64,
    pub count: u64,
    /// A gap annotation for this source covers the run.
    pub annotated: bool,
}

/// [`Report::issues`] keeps at most this many; the rest are only counted.
pub const MAX_ISSUES: usize = 1000;

#[derive(Debug, Default)]
pub struct Report {
    /// The first [`MAX_ISSUES`] issues.
    pub issues: Vec<Issue>,
    /// Errors and warnings past [`MAX_ISSUES`], counted but not kept.
    pub suppressed_errors: u64,
    pub suppressed_warnings: u64,
    /// Committed records per kind name (commits excluded).
    pub counts: BTreeMap<String, u64>,
    pub groups: u64,
    /// A terminal `session_end` directly follows a commit.
    pub complete: bool,
    pub losses: Vec<Loss>,
    pub header: Option<SessionHeader>,
    pub sources: BTreeMap<SourceId, SourceDeclaration>,
    pub session_end: Option<SessionEnd>,
}

impl Report {
    pub fn is_valid(&self) -> bool {
        self.suppressed_errors == 0 && self.issues.iter().all(|i| i.level < Level::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Error)
    }

    /// (errors, warnings), counting those past [`MAX_ISSUES`].
    pub fn issue_counts(&self) -> (u64, u64) {
        let errors = self.errors().count() as u64;
        (
            errors + self.suppressed_errors,
            self.issues.len() as u64 - errors + self.suppressed_warnings,
        )
    }
}

/// Per-source running state.
#[derive(Default)]
struct SourceState {
    /// Last seq seen, unwrapped to 64 bits.
    last_seq: Option<u64>,
    last_t_ns: Option<i64>,
}

/// Unwraps a u32 seq that follows `prev` (DATA_SPECIFICATION.md §6.2: seq wraps).
fn unwrap_seq(prev: Option<u64>, seq: u32) -> u64 {
    match prev {
        None => u64::from(seq),
        Some(p) => {
            let base = p & !0xFFFF_FFFF;
            let mut v = base | u64::from(seq);
            // Closest to prev + 1 among v - 2^32, v, v + 2^32.
            let target = p + 1;
            if v + (1 << 31) < target {
                v += 1 << 32;
            } else if v > target + (1 << 31) && v >= 1 << 32 {
                v -= 1 << 32;
            }
            v
        }
    }
}

struct Sim {
    linear: Option<LinearSensitivity>,
    driving: SourceId,
    paused: bool,
    last_frame: Option<u64>,
    next_tick: Option<u64>,
    drained_frames: BTreeSet<u64>,
}

struct V {
    report: Report,
    state: BTreeMap<SourceId, SourceState>,
    /// Sources in `state` with a `last_seq`.
    active: usize,
    /// Gap annotations: (source, first_seq, lost).
    gap_notes: Vec<(Option<SourceId>, u32, Option<u32>)>,
    last_commit: Option<crate::record::Commit>,
    sim: Option<Sim>,
}

impl V {
    fn issue(&mut self, level: Level, offset: Option<u64>, message: String) {
        let r = &mut self.report;
        if r.issues.len() < MAX_ISSUES {
            r.issues.push(Issue {
                level,
                offset,
                message,
            });
        } else if level == Level::Error {
            r.suppressed_errors += 1;
        } else {
            r.suppressed_warnings += 1;
        }
    }
    fn err(&mut self, offset: u64, message: String) {
        self.issue(Level::Error, Some(offset), message);
    }
    fn warn(&mut self, offset: u64, message: String) {
        self.issue(Level::Warning, Some(offset), message);
    }

    fn declared(&self, s: SourceId) -> bool {
        self.report.sources.contains_key(&s)
    }

    fn json(&mut self, offset: u64, kind: Kind, value: &serde_json::Value, first_chunk: bool) {
        match kind {
            Kind::SessionHeader => {
                if !first_chunk {
                    self.err(offset, "session_header is not the first chunk".into());
                    return;
                }
                match from_value::<SessionHeader>(value) {
                    Ok(h) => {
                        if !h.schema.starts_with(SCHEMA_PREFIX) {
                            self.err(offset, format!("unsupported schema {:?}", h.schema));
                        }
                        if !is_uuid(&h.session_id) {
                            self.err(
                                offset,
                                format!("session_id {:?} is not a UUID", h.session_id),
                            );
                        }
                        if let Some(t) = &h.started_at_utc
                            && !is_utc_rfc3339(t)
                        {
                            self.err(offset, format!("started_at_utc {t:?} is not RFC 3339 UTC"));
                        }
                        if let Some(sim) = &h.simulation {
                            if sim.sim_hz == 0 || 1_000_000_000 % sim.sim_hz != 0 {
                                self.err(
                                    offset,
                                    format!("sim_hz {} does not divide 1e9", sim.sim_hz),
                                );
                            }
                            let linear = h.sensitivity.as_ref().and_then(|s| s.linear());
                            if linear.is_none() {
                                self.err(offset, "simulation without a linear sensitivity".into());
                            }
                            self.sim = Some(Sim {
                                linear,
                                driving: sim.input_source,
                                paused: false,
                                last_frame: None,
                                next_tick: None,
                                drained_frames: BTreeSet::new(),
                            });
                        }
                        self.report.header = Some(h);
                    }
                    Err(e) => self.err(offset, format!("session_header: {e}")),
                }
            }
            Kind::SourceDeclare => match from_value::<SourceDeclaration>(value) {
                Ok(d) => {
                    if d.id == 0xFFFF {
                        self.err(offset, "source id 0xFFFF is reserved".into());
                    } else if self.declared(d.id) {
                        self.err(offset, format!("source {} declared twice", d.id));
                    }
                    if d.scale().is_none() {
                        self.err(
                            offset,
                            format!(
                                "source {}: value_scale {} not 1/256/1024",
                                d.id, d.value_scale
                            ),
                        );
                    }
                    let domains = self.report.header.as_ref().map(|h| &h.clock_domains);
                    if !domains.is_some_and(|ds| ds.iter().any(|c| c.id == d.clock_domain)) {
                        self.err(
                            offset,
                            format!(
                                "source {}: clock_domain {} not declared",
                                d.id, d.clock_domain
                            ),
                        );
                    }
                    self.report.sources.insert(d.id, d);
                }
                Err(e) => self.err(offset, format!("source_declare: {e}")),
            },
            _ => {}
        }
    }

    /// Checks one group's input records per source in `seq` order: a
    /// source's motion, button and wheel records sit in separate chunks
    /// (kinds ascending), so chunk order is not seq order (§9.2).
    fn inputs(&mut self, mut list: Vec<(SourceId, u64, i64, u64)>) {
        list.sort_by_key(|&(source, seq, _, _)| (source, seq));
        for (source, s, t, offset) in list {
            let st = self.state.entry(source).or_default();
            let prev_seq = st.last_seq.replace(s.max(st.last_seq.unwrap_or(0)));
            self.active += usize::from(prev_seq.is_none());
            let prev_t = st
                .last_t_ns
                .replace(t.max(st.last_t_ns.unwrap_or(i64::MIN)));
            match prev_seq {
                Some(p) if s <= p => self.err(
                    offset,
                    format!("source {source}: seq {} repeats or goes back", s as u32),
                ),
                Some(p) if s > p + 1 => self.report.losses.push(Loss {
                    source,
                    first_seq: p + 1,
                    count: s - p - 1,
                    annotated: false,
                }),
                _ => {}
            }
            if prev_t.is_some_and(|p| t < p) {
                self.err(
                    offset,
                    format!("source {source}: t_ns {t} goes backwards in seq order"),
                );
            }
        }
    }

    fn sim_record(&mut self, offset: u64, rec: &Record) {
        let Some(sim) = &mut self.sim else { return };
        let (mut errs, mut warns) = (Vec::new(), Vec::new());
        match rec {
            Record::Frame(f) => {
                if sim.last_frame.is_some_and(|p| f.frame <= p) {
                    errs.push(format!("frame {} does not increase", f.frame));
                }
                if sim.next_tick.is_some_and(|t| f.first_tick != t) {
                    errs.push(format!(
                        "frame {}: first_tick {} breaks tick continuity",
                        f.frame, f.first_tick
                    ));
                }
                sim.last_frame = Some(f.frame);
                sim.next_tick = f.first_tick.checked_add(u64::from(f.ticks));
            }
            Record::Drain(d) => {
                if d.source != sim.driving {
                    errs.push(format!(
                        "drain of source {} but the driving source is {}",
                        d.source, sim.driving
                    ));
                }
                if !sim.drained_frames.insert(d.frame) {
                    errs.push(format!("frame {} has two drains", d.frame));
                }
            }
            Record::SimControl(c) => {
                match c.action {
                    control_action::PAUSE => sim.paused = true,
                    control_action::RESUME => sim.paused = false,
                    control_action::SWITCH_SOURCE => {
                        if !sim.paused {
                            errs.push("switch_source outside a pause".into());
                        }
                        match c.new_source {
                            Some(new) => {
                                let (a, b) = (
                                    self.report.sources.get(&sim.driving),
                                    self.report.sources.get(&new),
                                );
                                match (a, b) {
                                (Some(a), Some(b))
                                    if a.input_level == b.input_level
                                        && a.units == b.units
                                        && a.value_scale == b.value_scale => {}
                                (_, None) => errs.push(format!("switch to undeclared source {new}")),
                                _ => errs.push(format!(
                                    "switch from source {} to {new} changes level, units or scale",
                                    sim.driving
                                )),
                            }
                                sim.driving = new;
                            }
                            None => errs.push("switch_source without new_source".into()),
                        }
                    }
                    a if a > control_action::SWITCH_SOURCE => {
                        warns.push(format!("unknown sim_control action {a}"));
                    }
                    _ => {}
                }
            }
            Record::SimCamera(c) => {
                let scale = self
                    .report
                    .sources
                    .get(&sim.driving)
                    .and_then(SourceDeclaration::scale);
                if let (Some(lin), Some(scale)) = (sim.linear, scale) {
                    let (y, p) = (
                        lin.yaw(c.yaw_units, scale).0,
                        lin.pitch(c.pitch_units, scale).0,
                    );
                    if y.to_bits() != c.yaw_rad.to_bits() || p.to_bits() != c.pitch_rad.to_bits() {
                        errs.push(format!(
                            "sim_camera tick {}: radians differ from the recomputation",
                            c.tick
                        ));
                    }
                }
            }
            Record::SimTarget(t) if t.event > target_event::KILL || t.shape > target_shape::BOX => {
                warns.push(format!(
                    "sim_target {}: unknown event {} or shape {}",
                    t.target, t.event, t.shape
                ));
            }
            _ => {}
        }
        for w in warns {
            self.warn(offset, w);
        }
        for e in errs {
            self.err(offset, e);
        }
    }

    fn commit(&mut self, offset: u64, c: &crate::record::Commit) {
        if let Some(p) = self.last_commit.take() {
            if c.t_ns < p.t_ns {
                self.err(offset, "commit t_ns goes backwards".into());
            }
            let back = |a: Option<u64>, b: Option<u64>| {
                matches!((a, b), (Some(a), Some(b)) if b < a) || (a.is_some() && b.is_none())
            };
            if back(p.tick, c.tick) || back(p.frame, c.frame) {
                self.err(offset, "commit tick/frame watermark goes backwards".into());
            }
        }
        for &(source, last) in &c.sources {
            let seen = self.state.get(&source).and_then(|s| s.last_seq);
            if seen.map(|s| s as u32) != Some(last) {
                self.err(
                    offset,
                    format!("commit: source {source} last_seq {last} but {seen:?} written"),
                );
            }
        }
        let listed: BTreeSet<SourceId> = c.sources.iter().map(|s| s.0).collect();
        if listed.len() != c.sources.len() {
            self.err(offset, "commit lists a source twice".into());
        }
        // O(commit size): never walk every source per commit.
        let active = listed
            .iter()
            .filter(|s| self.state.get(s).is_some_and(|st| st.last_seq.is_some()))
            .count();
        if active < self.active {
            self.err(
                offset,
                format!("commit omits {} sources", self.active - active),
            );
        }
        self.last_commit = Some(c.clone());
    }
}

/// Validates a complete log (or a recovered byte stream).
pub fn validate<R: Read>(r: R) -> Report {
    let mut v = V {
        report: Report::default(),
        state: BTreeMap::new(),
        active: 0,
        gap_notes: Vec::new(),
        last_commit: None,
        sim: None,
    };
    let groups = match GroupReader::new(r) {
        Ok(g) => g,
        Err(e) => {
            v.issue(Level::Error, Some(0), e.to_string());
            return v.report;
        }
    };
    let mut first_chunk = true;
    let mut groups = groups;
    for group in groups.by_ref() {
        v.report.groups += 1;
        let mut seen_body = false;
        let mut last_code = 0u8;
        let mut inputs = Vec::new();
        // Drains and sim_control records, checked in replay order at the
        // group's end (§9.1): kinds ascend in a group, so file order puts
        // every drain before a phase-0 switch_source of the same frame.
        let mut sim_order = Vec::new();
        for chunk in &group.chunks {
            let offset = chunk.offset();
            let code = match chunk {
                Chunk::Records { kind, .. }
                | Chunk::Undecodable {
                    error: crate::record::DecodeError { kind, .. },
                    ..
                } => *kind as u8,
                Chunk::Skipped { header, .. } => header.kind,
            };
            let is_decl = Kind::from_u8(code).is_some_and(Kind::is_declaration);
            if is_decl && seen_body {
                v.err(
                    offset,
                    format!(
                        "declaration {} after non-declaration chunks",
                        Kind::from_u8(code).unwrap().name()
                    ),
                );
            }
            if !is_decl {
                if seen_body && code < last_code {
                    v.err(
                        offset,
                        format!("kind 0x{code:02X} after 0x{last_code:02X}: kinds not ascending"),
                    );
                }
                seen_body = true;
                last_code = code;
            }
            if first_chunk && code != Kind::SessionHeader as u8 {
                v.err(offset, "the first chunk is not session_header".into());
            }
            match chunk {
                Chunk::Skipped { header, .. } => {
                    if first_chunk && code == Kind::SessionHeader as u8 {
                        v.err(
                            offset,
                            "session_header unreadable by this reader (kind_version/flags)".into(),
                        );
                    }
                    v.warn(
                        offset,
                        format!(
                            "skipped chunk: kind 0x{:02X} version {} flags 0x{:04X}",
                            header.kind, header.kind_version, header.flags
                        ),
                    )
                }
                Chunk::Undecodable { error, .. } => v.err(offset, error.to_string()),
                Chunk::Records { kind, records, .. } => {
                    for rec in records {
                        *v.report.counts.entry(kind.name().to_owned()).or_insert(0) += 1;
                        if rec.floats().iter().any(|x| !x.is_finite()) {
                            v.err(offset, format!("NaN or infinity in {}", kind.name()));
                        }
                        match rec {
                            Record::Json { kind, value } => {
                                v.json(offset, *kind, value, first_chunk)
                            }
                            Record::Motion(_) | Record::Button(_) | Record::Wheel(_) => {
                                let (source, seq) = rec.source_seq().unwrap();
                                if !v.declared(source) {
                                    v.err(
                                        offset,
                                        format!(
                                            "{} from undeclared source {source}",
                                            rec.kind().name()
                                        ),
                                    );
                                }
                                let prev = v.state.get(&source).and_then(|s| s.last_seq);
                                inputs.push((
                                    source,
                                    unwrap_seq(prev, seq),
                                    rec.input_t_ns().unwrap().0,
                                    offset,
                                ));
                            }
                            Record::Annotation(a) => {
                                if a.gap().is_some() {
                                    v.gap_notes.push((a.source, a.first_seq, a.lost));
                                }
                                if a.source.is_some_and(|s| !v.declared(s)) {
                                    v.err(
                                        offset,
                                        format!(
                                            "annotation names undeclared source {:?}",
                                            a.source
                                        ),
                                    );
                                }
                            }
                            Record::ClockSync(c) => {
                                let known = v.report.header.as_ref().is_some_and(|h| {
                                    h.clock_domains.iter().any(|d| d.id == c.domain)
                                });
                                if !known {
                                    v.err(
                                        offset,
                                        format!("clock_sync for undeclared domain {}", c.domain),
                                    );
                                }
                            }
                            Record::Drain(d) => sim_order.push((d.frame, 1, offset, rec)),
                            Record::SimControl(c) => {
                                let rank = if c.phase == 0 { 0 } else { 2 };
                                sim_order.push((c.frame, rank, offset, rec));
                            }
                            _ => v.sim_record(offset, rec),
                        }
                        if let Record::Drain(d) = rec
                            && !v.declared(d.source)
                        {
                            v.err(offset, format!("drain of undeclared source {}", d.source));
                        }
                    }
                }
            }
            first_chunk = false;
        }
        if first_chunk {
            v.err(group.start, "the first group has no session_header".into());
            first_chunk = false;
        }
        sim_order.sort_by_key(|&(frame, rank, ..)| (frame, rank));
        for (_, _, offset, rec) in sim_order {
            v.sim_record(offset, rec);
        }
        v.inputs(inputs);
        v.commit(group.commit_offset, &group.commit);
    }
    let (ending, _) = groups.ending();

    // Loss annotation coverage (FORMAT.md §5: a gap annotation describes one
    // run from `first_seq`). Known `lost`: per source, notes sorted by first
    // seq with the running maximum of their ends, one binary search per loss.
    // Unknown `lost` covers only the run starting exactly at `first_seq`.
    let mut known: BTreeMap<SourceId, Vec<(u64, u64)>> = BTreeMap::new();
    let mut unknown = BTreeSet::new();
    for (src, first, lost) in std::mem::take(&mut v.gap_notes) {
        let Some(src) = src else { continue };
        match lost {
            Some(l) => known
                .entry(src)
                .or_default()
                .push((u64::from(first), u64::from(first) + u64::from(l))),
            None => drop(unknown.insert((src, first))),
        }
    }
    for notes in known.values_mut() {
        notes.sort_unstable();
        for i in 1..notes.len() {
            notes[i].1 = notes[i].1.max(notes[i - 1].1);
        }
    }
    let mut losses = std::mem::take(&mut v.report.losses);
    for loss in &mut losses {
        let first = loss.first_seq & 0xFFFF_FFFF;
        let notes = known.get(&loss.source).map_or(&[][..], Vec::as_slice);
        let i = notes.partition_point(|n| n.0 <= first);
        loss.annotated = (i > 0 && notes[i - 1].1 >= first + loss.count)
            || unknown.contains(&(loss.source, first as u32));
        if !loss.annotated {
            v.issue(
                Level::Warning,
                None,
                format!(
                    "source {}: {} seqs missing from {} without a gap annotation (capture bug)",
                    loss.source, loss.count, loss.first_seq
                ),
            );
        }
    }
    v.report.losses = losses;

    if v.report.header.is_none() && v.report.is_valid() {
        v.issue(Level::Error, None, "no committed session_header".into());
    }
    if let Some(e) = &ending.error {
        let off = match e {
            ReadError::Truncated { offset }
            | ReadError::PayloadTooLarge { offset, .. }
            | ReadError::PayloadMisaligned { offset, .. }
            | ReadError::CrcMismatch { offset }
            | ReadError::NonZeroTrailer { offset }
            | ReadError::NonZeroReserved { offset } => Some(*offset),
            _ => None,
        };
        v.issue(Level::Error, off, format!("reading stopped: {e}"));
    }
    for &o in &ending.uncommitted {
        v.warn(
            o,
            "chunk after the last commit is uncommitted and ignored".into(),
        );
    }
    for &o in &ending.after_end {
        v.err(o, "chunk after the terminal session_end".into());
    }
    match &ending.session_end {
        Some(value) => match from_value::<SessionEnd>(value) {
            Ok(end) => {
                let mut expected = v.report.counts.clone();
                expected.retain(|_, n| *n > 0);
                if end.counts != expected {
                    v.issue(
                        Level::Error,
                        None,
                        format!(
                            "session_end counts {:?} differ from the committed records {:?}",
                            end.counts, expected
                        ),
                    );
                }
                if v.last_commit
                    .as_ref()
                    .is_some_and(|c| end.t_end_ns < c.t_ns.0)
                {
                    v.issue(
                        Level::Error,
                        None,
                        "session_end t_end_ns precedes the last commit".into(),
                    );
                }
                v.report.complete = true;
                v.report.session_end = Some(end);
            }
            Err(e) => v.issue(Level::Error, None, format!("session_end: {e}")),
        },
        None => v.issue(
            Level::Error,
            None,
            "no terminal session_end: the session is incomplete (recover it)".into(),
        ),
    }
    v.report
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]Z`: RFC 3339 in UTC, the shape
/// `kf_store::utc_now_rfc3339` writes (second 60 is a leap second).
fn is_utc_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    let num = |at: usize, len: usize| {
        b.get(at..at + len)?.iter().try_fold(0u32, |n, &c| {
            c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0'))
        })
    };
    let fields = [(0, 4), (5, 2), (8, 2), (11, 2), (14, 2), (17, 2)].map(|(at, len)| num(at, len));
    let [Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)] = fields else {
        return false;
    };
    let days = match mo {
        2 => 28 + u32::from(y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)),
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]
        .iter()
        .all(|&(i, c)| b[i] == c)
        && (1..=12).contains(&mo)
        && (1..=days).contains(&d)
        && h < 24
        && mi < 60
        && sec <= 60
        && match &b[19..] {
            [b'Z'] => true,
            [b'.', frac @ .., b'Z'] => !frac.is_empty() && frac.iter().all(u8::is_ascii_digit),
            _ => false,
        }
}
