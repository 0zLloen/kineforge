//! The pure group encoder (DATA_SPECIFICATION.md §5.2). The recorder thread
//! (or, on the web, the frame loop) pushes records as they arrive and cuts a
//! group between messages; each cut returns the bytes to append to
//! `raw.kflog`. The concatenation of every returned buffer is the file.
//!
//! Group order: declaration chunks (`session_header` first, then
//! `source_declare`/`device_info` in arrival order), then every other kind in
//! ascending kind number (a kind may span several chunks, each ≤ 1 MiB, in
//! arrival order), then the `commit` chunk. `session_end` follows the final
//! commit as the terminal chunk.
//!
//! Time is never read from a clock: the caller passes session time.
//!
//! **Recorder-thread protocol** (with the native `SessionWriter`):
//! 1. [`Recorder::push`] each record as it arrives; between frame messages,
//!    when [`Recorder::due`], submit [`Recorder::cut`]'s bytes to the writer.
//! 2. Periodically pass the writer's `status().written_bytes` to
//!    [`Recorder::written`]; it forgets confirmed group boundaries, so their
//!    marks stay bounded instead of growing by one per cut.
//! 3. After any writer drop: stop submitting, `drain` the writer and pass its
//!    final `written_bytes` to [`Recorder::writer_lost`].
//! 4. Either way, close the writer with [`Recorder::finish`]'s bytes
//!    (`end_reason` `storage_error` after a drop).
//!
//! **Writer losses.** If the storage side drops groups (memory cap, write
//! error), the file ends at an earlier group boundary than the recorder
//! believes. [`Recorder::writer_lost`] rolls the recorder back to that
//! boundary (counts, commit watermarks), records a `storage_error` gap per
//! source with written input plus one without a source (for sources whose
//! records were all lost) and counts the dropped chunks, so the final
//! group, its commit and `session_end` describe exactly what is in the
//! file. The session then ends with `end_reason` `storage_error`.

use crate::format::{FILE_HEADER, Kind, Layout, MAX_PAYLOAD, write_chunk};
use crate::json::{SessionEnd, SessionHeader, to_value};
use crate::record::{
    Commit, EncodeError, Record, annotation_len, encode_annotation, encode_commit, encode_fixed,
    encode_json,
};
use kf_core::{Annotation, GapReason, Nanos, SourceId};
use std::collections::{BTreeMap, VecDeque};

/// Flush policy (DATA_SPECIFICATION.md §5.2 defaults).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlushPolicy {
    /// Cut a group when this much session time has passed since the last cut.
    pub chunk_flush_ns: i64,
    /// … or when any kind has buffered this many records.
    pub max_records: u32,
}

impl Default for FlushPolicy {
    fn default() -> FlushPolicy {
        FlushPolicy {
            chunk_flush_ns: 100_000_000,
            max_records: 4096,
        }
    }
}

/// Buffered chunks of one kind: (payload, record_count), last one open.
#[derive(Default)]
struct KindBuf {
    chunks: Vec<(Vec<u8>, u32)>,
    records: u32,
}

impl KindBuf {
    /// The open chunk with room for `len` more bytes.
    fn room(&mut self, len: usize) -> &mut (Vec<u8>, u32) {
        let full = self
            .chunks
            .last()
            .is_none_or(|(p, _)| p.len() + len > MAX_PAYLOAD as usize);
        if full {
            self.chunks.push((Vec::new(), 0));
        }
        self.chunks.last_mut().unwrap()
    }
}

/// Recorder state at a group boundary.
#[derive(Clone)]
struct Mark {
    /// File offset just after the group.
    end: u64,
    /// Chunks emitted up to it.
    chunks: u64,
    counts: BTreeMap<Kind, u64>,
    last_seq: BTreeMap<SourceId, u32>,
    last_frame: Option<u64>,
    last_tick: Option<u64>,
}

/// [`Recorder::writer_lost`] was given a length that is no group boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotABoundary(pub u64);

impl std::fmt::Display for NotABoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "file length {} is not a group boundary", self.0)
    }
}

impl std::error::Error for NotABoundary {}

/// Encodes records into groups.
pub struct Recorder {
    policy: FlushPolicy,
    /// File header not yet emitted.
    first: bool,
    /// Declaration JSON payloads in arrival order (header first).
    declarations: Vec<(Kind, Vec<u8>)>,
    /// Non-declaration kinds, ascending by kind code.
    bufs: BTreeMap<Kind, KindBuf>,
    last_seq: BTreeMap<SourceId, u32>,
    last_frame: Option<u64>,
    last_tick: Option<u64>,
    last_cut: Nanos,
    counts: BTreeMap<Kind, u64>,
    bytes_written: u64,
    /// The header chunk payload, re-emitted if the writer lost the first group.
    header: Vec<u8>,
    chunks_emitted: u64,
    dropped_chunks: u64,
    /// Group boundaries not yet confirmed written (see [`Recorder::written`]).
    marks: VecDeque<Mark>,
}

impl Recorder {
    /// Starts a session; the header goes first into the first group.
    pub fn new(header: &SessionHeader, policy: FlushPolicy) -> Result<Recorder, EncodeError> {
        let mut r = Recorder {
            policy,
            first: true,
            declarations: Vec::new(),
            bufs: BTreeMap::new(),
            last_seq: BTreeMap::new(),
            last_frame: None,
            last_tick: None,
            last_cut: Nanos(0),
            counts: BTreeMap::new(),
            bytes_written: 0,
            header: Vec::new(),
            chunks_emitted: 0,
            dropped_chunks: 0,
            marks: VecDeque::new(),
        };
        r.marks.push_back(r.mark());
        r.push(Record::Json {
            kind: Kind::SessionHeader,
            value: to_value(header),
        })?;
        r.header = r.declarations[0].1.clone();
        Ok(r)
    }

    fn mark(&self) -> Mark {
        Mark {
            end: self.bytes_written,
            chunks: self.chunks_emitted,
            counts: self.counts.clone(),
            last_seq: self.last_seq.clone(),
            last_frame: self.last_frame,
            last_tick: self.last_tick,
        }
    }

    /// Buffers one record. `commit` and `session_end` are written by
    /// [`Recorder::cut`] and [`Recorder::finish`], and a second
    /// `session_header` is refused.
    pub fn push(&mut self, rec: Record) -> Result<(), EncodeError> {
        let kind = rec.kind();
        let refuse = |what| Err(EncodeError { kind, what });
        match kind {
            Kind::Commit | Kind::SessionEnd => return refuse("written by the recorder itself"),
            Kind::SessionHeader if self.counts.contains_key(&kind) => {
                return refuse("only one session_header");
            }
            _ => {}
        }
        match (&rec, kind.layout()) {
            (Record::Json { value, .. }, Layout::Json) => {
                let payload = encode_json(kind, value)?;
                if kind.is_declaration() {
                    self.declarations.push((kind, payload));
                } else {
                    self.bufs.entry(kind).or_default().chunks.push((payload, 1));
                }
            }
            (Record::Annotation(a), _) => {
                let buf = self.bufs.entry(kind).or_default();
                let (payload, n) = buf.room(annotation_len(a));
                let start = payload.len();
                if let Err(e) = encode_annotation(a, payload) {
                    payload.truncate(start);
                    return Err(e);
                }
                *n += 1;
            }
            (_, Layout::Fixed(size)) => {
                let buf = self.bufs.entry(kind).or_default();
                let (payload, n) = buf.room(size);
                let start = payload.len();
                if let Err(e) = encode_fixed(&rec, payload) {
                    payload.truncate(start);
                    return Err(e);
                }
                *n += 1;
            }
            _ => return refuse("record does not match its kind's layout"),
        }
        if let Some(b) = self.bufs.get_mut(&kind) {
            b.records += 1;
        }
        if let Some((source, seq)) = rec.source_seq() {
            self.last_seq.insert(source, seq);
        }
        if let Record::Frame(f) = &rec {
            self.last_frame = Some(f.frame);
            if f.ticks > 0 {
                self.last_tick = Some(f.first_tick + u64::from(f.ticks) - 1);
            }
        }
        *self.counts.entry(kind).or_insert(0) += 1;
        Ok(())
    }

    /// Whether a cut is due at session time `now` (flush interval elapsed,
    /// or a kind reached `max_records`). Cut only between frame messages.
    pub fn due(&self, now: Nanos) -> bool {
        self.has_records()
            && (now.0.saturating_sub(self.last_cut.0) >= self.policy.chunk_flush_ns
                || self
                    .bufs
                    .values()
                    .any(|b| b.records >= self.policy.max_records))
    }

    fn has_records(&self) -> bool {
        self.first || !self.declarations.is_empty() || self.bufs.values().any(|b| b.records > 0)
    }

    /// Closes the current group with its commit and returns its bytes
    /// (preceded by the file header in the first group). Empty if nothing
    /// was buffered since the last cut.
    pub fn cut(&mut self, now: Nanos) -> Vec<u8> {
        self.group(now, false)
    }

    fn group(&mut self, now: Nanos, force: bool) -> Vec<u8> {
        self.last_cut = now;
        if !force && !self.has_records() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.first {
            out.extend_from_slice(&FILE_HEADER);
            self.first = false;
        }
        for (kind, payload) in self.declarations.drain(..) {
            write_chunk(&mut out, kind, 1, &payload);
            self.chunks_emitted += 1;
        }
        for (&kind, buf) in &mut self.bufs {
            for (payload, n) in buf.chunks.drain(..) {
                // A chunk opened for a record that then failed to encode.
                if n > 0 {
                    write_chunk(&mut out, kind, n, &payload);
                    self.chunks_emitted += 1;
                }
            }
            buf.records = 0;
        }
        let commit = Commit {
            t_ns: now,
            tick: self.last_tick,
            frame: self.last_frame,
            sources: self.last_seq.iter().map(|(&s, &q)| (s, q)).collect(),
        };
        write_chunk(&mut out, Kind::Commit, 1, &encode_commit(&commit));
        self.chunks_emitted += 1;
        self.bytes_written += out.len() as u64;
        let mark = self.mark();
        self.marks.push_back(mark);
        out
    }

    /// The storage side has appended everything up to `file_len` (the
    /// writer's `status().written_bytes`, which never decreases): forgets
    /// older group boundaries (keeps memory bounded). The boundary at
    /// `file_len` is kept, since [`Recorder::writer_lost`] may still need it.
    pub fn written(&mut self, file_len: u64) {
        while self.marks.len() > 1 && self.marks[1].end <= file_len {
            self.marks.pop_front();
        }
    }

    /// The storage side dropped every group after `file_len` (the writer's
    /// final length once its queue has drained). Rolls back to that group
    /// boundary, discards buffered records, counts the dropped chunks for
    /// `session_end.dropped_chunks`, and buffers a `storage_error` gap
    /// annotation per source with written input (from its first lost seq,
    /// `lost` unknown) plus one without a source, which covers sources whose
    /// records were all in the lost groups. Then end the session with
    /// [`Recorder::finish`] and `end_reason` `storage_error`; push no more
    /// frames, since they would break tick continuity.
    pub fn writer_lost(&mut self, file_len: u64, now: Nanos) -> Result<(), NotABoundary> {
        let at = self
            .marks
            .iter()
            .rposition(|m| m.end == file_len)
            .ok_or(NotABoundary(file_len))?;
        self.marks.truncate(at + 1);
        let m = self.marks[at].clone();
        self.dropped_chunks += self.chunks_emitted - m.chunks;
        self.chunks_emitted = m.chunks;
        self.bytes_written = m.end;
        self.counts = m.counts;
        self.last_seq = m.last_seq;
        self.last_frame = m.last_frame;
        self.last_tick = m.last_tick;
        self.declarations.clear();
        self.bufs.clear();
        if m.end == 0 {
            // Even the first group is gone: start the file again.
            self.first = true;
            self.declarations
                .push((Kind::SessionHeader, self.header.clone()));
            self.counts.insert(Kind::SessionHeader, 1);
        }
        let gap = |source, first_seq| {
            Record::Annotation(Annotation {
                t_ns: now,
                code: GapReason::StorageError.code(),
                source,
                first_seq,
                lost: None,
                text: "dropped by the storage writer".into(),
            })
        };
        let gaps: Vec<Record> = self
            .last_seq
            .iter()
            .map(|(&s, &q)| gap(Some(s), q.wrapping_add(1)))
            .chain([gap(None, 0)])
            .collect();
        for g in gaps {
            self.push(g).expect("gap annotations encode");
        }
        Ok(())
    }

    /// Writes the final group with its commit, then `session_end` as the
    /// terminal chunk. `end.counts` and `end.dropped_chunks` are filled in by
    /// the recorder (records written; chunks lost by the writer).
    pub fn finish(mut self, mut end: SessionEnd, now: Nanos) -> Result<Vec<u8>, EncodeError> {
        end.counts = self.counts();
        end.dropped_chunks = self.dropped_chunks;
        let payload = encode_json(Kind::SessionEnd, &to_value(&end))?;
        // The final group always carries a commit, even when empty.
        let mut out = self.group(now, true);
        write_chunk(&mut out, Kind::SessionEnd, 1, &payload);
        Ok(out)
    }

    /// Records written so far per kind name (excluding commits).
    pub fn counts(&self) -> BTreeMap<String, u64> {
        self.counts
            .iter()
            .map(|(k, &n)| (k.name().to_owned(), n))
            .collect()
    }

    /// Bytes returned by cuts so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}
