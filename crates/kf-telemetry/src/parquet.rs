//! Parquet export of the derived tables (DATA_SPECIFICATION.md §4, §8): the
//! committed records of a log re-encoded, one zstd file per table that has
//! rows, named by [`DERIVED_TABLES`]. Pure: the caller supplies the writers.
//!
//! Columns carry the §6 field names and exact types (`u64` → UInt64, …,
//! `f64` → Float64); on-disk sentinels are nulls, never the sentinel value.
//! `sim_targets` flattens its vectors to `pos_m_x/_y/_z`, `vel_m_s_x/_y/_z`.
//! `input_events` holds button and wheel records, told apart by `type`
//! (`"button"`/`"wheel"`); the other kind's columns are null. `app_results`
//! has typed columns for the §6.5 fields (null when absent or of another JSON
//! type) plus `json`, the whole object.
//!
//! Key-value metadata on every file (§8): `kineforge.format_version` `1`,
//! `session_identity_sha256`, `algorithm` `kf-telemetry/raw-reencode`,
//! `algorithm_version` `1`, `params` `{}` and `evidence_class`, the raw
//! class kept (§3.2, §6.2–§6.5): `simulated_truth` for `sim_*`,
//! `app_reported` for `app_results`, `observed` for frames, frame timing,
//! drains, annotations and clock sync, and for the two input tables a JSON
//! object mapping each source id with rows in the table to its declared
//! class (null if undeclared), e.g. `{"0":"observed","2":"simulated_truth"}`,
//! since input can mix sources.
//!
//! Memory stays bounded: each table buffers at most [`ROW_GROUP_ROWS`] rows
//! (or 64 MiB of strings) before writing them as one row group.

use crate::Kind;
use crate::kfz::DERIVED_TABLES;
use crate::reader::{GroupReader, ReadError};
use crate::record::Record;
use arrow_array::RecordBatch;
use arrow_array::builder::{
    ArrayBuilder, BooleanBuilder, Float64Builder, Int32Builder, Int64Builder, StringBuilder,
    UInt8Builder, UInt16Builder, UInt32Builder, UInt64Builder, make_builder,
};
use arrow_schema::DataType::{
    self, Boolean, Float64, Int32, Int64, UInt8, UInt16, UInt32, UInt64, Utf8,
};
use arrow_schema::{Field, Schema, SchemaRef};
use kf_core::{Nanos, SourceId};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Rows per row group.
pub const ROW_GROUP_ROWS: usize = 65_536;
/// A row group is also cut once its strings reach this size.
const ROW_GROUP_TEXT_BYTES: usize = 64 << 20;

/// `algorithm` metadata value.
pub const ALGORITHM: &str = "kf-telemetry/raw-reencode";

type Columns = &'static [(&'static str, DataType, bool)];

/// Name, type and nullability of every column, in [`DERIVED_TABLES`] order.
const COLUMNS: [Columns; 12] = [
    // input_motion
    &[
        ("t_ns", Int64, false),
        ("src_ts_ns", Int64, true),
        ("dx", Int32, false),
        ("dy", Int32, false),
        ("seq", UInt32, false),
        ("source", UInt16, false),
        ("flags", UInt16, false),
    ],
    // input_events
    &[
        ("type", Utf8, false),
        ("t_ns", Int64, false),
        ("src_ts_ns", Int64, true),
        ("seq", UInt32, false),
        ("source", UInt16, false),
        ("button", UInt8, true),
        ("state", UInt8, true),
        ("dv", Int32, true),
        ("dh", Int32, true),
        ("flags", UInt16, true),
    ],
    // frames
    &[
        ("frame", UInt64, false),
        ("t_start_ns", Int64, false),
        ("t_input_cutoff_ns", Int64, false),
        ("first_tick", UInt64, false),
        ("ticks", UInt32, false),
        ("flags", UInt16, false),
    ],
    // frame_timing
    &[
        ("frame", UInt64, false),
        ("t_submit_ns", Int64, false),
        ("t_present_ns", Int64, true),
        ("cpu_ns", UInt32, false),
        ("gpu_ns", UInt32, true),
    ],
    // drains
    &[
        ("frame", UInt64, false),
        ("source", UInt16, false),
        ("last_seq", UInt32, false),
    ],
    // sim_camera
    &[
        ("tick", UInt64, false),
        ("yaw_units", Int64, false),
        ("pitch_units", Int64, false),
        ("yaw_rad", Float64, false),
        ("pitch_rad", Float64, false),
    ],
    // sim_targets
    &[
        ("tick", UInt64, false),
        ("target", UInt32, false),
        ("event", UInt8, false),
        ("shape", UInt8, false),
        ("pos_m_x", Float64, false),
        ("pos_m_y", Float64, false),
        ("pos_m_z", Float64, false),
        ("vel_m_s_x", Float64, false),
        ("vel_m_s_y", Float64, false),
        ("vel_m_s_z", Float64, false),
        ("radius_m", Float64, false),
    ],
    // sim_shots
    &[
        ("tick", UInt64, false),
        ("t_ns", Int64, false),
        ("target", UInt32, true),
        ("seq", UInt32, false),
        ("ang_err_rad", Float64, false),
        ("d_yaw_rad", Float64, false),
        ("d_pitch_rad", Float64, false),
        ("hit", Boolean, false),
    ],
    // sim_control
    &[
        ("tick", UInt64, false),
        ("t_ns", Int64, false),
        ("frame", UInt64, false),
        ("action", UInt8, false),
        ("phase", UInt8, false),
        ("reason", UInt16, false),
        ("new_source", UInt16, true),
    ],
    // annotations
    &[
        ("t_ns", Int64, false),
        ("code", UInt16, false),
        ("source", UInt16, true),
        ("first_seq", UInt32, false),
        ("lost", UInt32, true),
        ("text", Utf8, false),
    ],
    // app_results
    &[
        ("t_ns", Int64, true),
        ("metric", Utf8, true),
        ("value", Float64, true),
        ("unit", Utf8, true),
        ("method", Utf8, true),
        ("transcribed_by", Utf8, true),
        ("json", Utf8, false),
    ],
    // clock_sync
    &[
        ("t_ns", Int64, false),
        ("domain", UInt16, false),
        ("src_ts_ns", Int64, false),
        ("uncertainty_ns", Int64, false),
    ],
];

/// One cell; `None` is null.
enum V {
    I64(Option<i64>),
    U64(Option<u64>),
    I32(Option<i32>),
    U32(Option<u32>),
    U16(Option<u16>),
    U8(Option<u8>),
    F64(Option<f64>),
    Bool(Option<bool>),
    Str(Option<String>),
}

macro_rules! cell_from {
    ($($t:ty => $v:ident),*) => {$(
        impl From<$t> for V {
            fn from(x: $t) -> V { V::$v(Some(x)) }
        }
        impl From<Option<$t>> for V {
            fn from(x: Option<$t>) -> V { V::$v(x) }
        }
    )*};
}
cell_from!(i64 => I64, u64 => U64, i32 => I32, u32 => U32, u16 => U16, u8 => U8, f64 => F64,
    bool => Bool, String => Str);

impl From<Nanos> for V {
    fn from(t: Nanos) -> V {
        V::I64(Some(t.0))
    }
}

impl From<Option<Nanos>> for V {
    fn from(t: Option<Nanos>) -> V {
        V::I64(t.map(|t| t.0))
    }
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        V::Str(Some(s.to_owned()))
    }
}

macro_rules! cells {
    ($($x:expr),* $(,)?) => { vec![$(V::from($x)),*] };
}

/// The table and cells of a record that has a derived row.
// ponytail: one Vec per row; reuse a row buffer if export time ever matters.
fn row(rec: &Record) -> Option<(&'static str, Vec<V>)> {
    Some(match rec {
        Record::Motion(m) => (
            "input_motion",
            cells![m.t_ns, m.src_ts_ns, m.dx, m.dy, m.seq, m.source, m.flags],
        ),
        Record::Button(b) => (
            "input_events",
            cells![
                "button",
                b.t_ns,
                b.src_ts_ns,
                b.seq,
                b.source,
                b.button,
                b.state,
                V::I32(None),
                V::I32(None),
                V::U16(None)
            ],
        ),
        Record::Wheel(w) => (
            "input_events",
            cells![
                "wheel",
                w.t_ns,
                w.src_ts_ns,
                w.seq,
                w.source,
                V::U8(None),
                V::U8(None),
                w.dv,
                w.dh,
                w.flags
            ],
        ),
        Record::Frame(f) => (
            "frames",
            cells![
                f.frame,
                f.t_start_ns,
                f.t_input_cutoff_ns,
                f.first_tick,
                f.ticks,
                f.flags
            ],
        ),
        Record::FrameTiming(t) => (
            "frame_timing",
            cells![t.frame, t.t_submit_ns, t.t_present_ns, t.cpu_ns, t.gpu_ns],
        ),
        Record::Drain(d) => ("drains", cells![d.frame, d.source, d.last_seq]),
        Record::SimCamera(c) => (
            "sim_camera",
            cells![c.tick, c.yaw_units, c.pitch_units, c.yaw_rad, c.pitch_rad],
        ),
        Record::SimTarget(t) => {
            let ([px, py, pz], [vx, vy, vz]) = (t.pos_m, t.vel_m_s);
            (
                "sim_targets",
                cells![
                    t.tick, t.target, t.event, t.shape, px, py, pz, vx, vy, vz, t.radius_m
                ],
            )
        }
        Record::SimShot(s) => (
            "sim_shots",
            cells![
                s.tick,
                s.t_ns,
                s.target,
                s.seq,
                s.ang_err_rad,
                s.d_yaw_rad,
                s.d_pitch_rad,
                s.hit
            ],
        ),
        Record::SimControl(c) => (
            "sim_control",
            cells![
                c.tick,
                c.t_ns,
                c.frame,
                c.action,
                c.phase,
                c.reason,
                c.new_source
            ],
        ),
        Record::Annotation(a) => (
            "annotations",
            cells![
                a.t_ns,
                a.code,
                a.source,
                a.first_seq,
                a.lost,
                a.text.clone()
            ],
        ),
        Record::Json {
            kind: Kind::AppResult,
            value,
        } => {
            let s = |k| value.get(k).and_then(Value::as_str).map(str::to_owned);
            (
                "app_results",
                cells![
                    value.get("t_ns").and_then(Value::as_i64),
                    s("metric"),
                    value.get("value").and_then(Value::as_f64),
                    s("unit"),
                    s("method"),
                    s("transcribed_by"),
                    value.to_string(),
                ],
            )
        }
        Record::ClockSync(c) => (
            "clock_sync",
            cells![c.t_ns, c.domain, c.src_ts_ns, c.uncertainty_ns],
        ),
        _ => return None,
    })
}

/// Appends a cell to the builder of its column.
fn append(b: &mut dyn ArrayBuilder, v: V) {
    fn get<B: 'static>(b: &mut dyn ArrayBuilder) -> &mut B {
        b.as_any_mut()
            .downcast_mut()
            .expect("cell type matches the column type")
    }
    match v {
        V::I64(x) => get::<Int64Builder>(b).append_option(x),
        V::U64(x) => get::<UInt64Builder>(b).append_option(x),
        V::I32(x) => get::<Int32Builder>(b).append_option(x),
        V::U32(x) => get::<UInt32Builder>(b).append_option(x),
        V::U16(x) => get::<UInt16Builder>(b).append_option(x),
        V::U8(x) => get::<UInt8Builder>(b).append_option(x),
        V::F64(x) => get::<Float64Builder>(b).append_option(x),
        V::Bool(x) => get::<BooleanBuilder>(b).append_option(x),
        V::Str(x) => get::<StringBuilder>(b).append_option(x),
    }
}

/// One exported table.
#[derive(Debug)]
pub struct Exported<W> {
    /// Its [`DERIVED_TABLES`] name; the file is `<name>.parquet`.
    pub table: &'static str,
    pub rows: u64,
    /// The writer the factory returned, holding the complete file.
    pub out: W,
}

struct Table<W: Write + Send> {
    name: &'static str,
    schema: SchemaRef,
    cols: Vec<Box<dyn ArrayBuilder>>,
    pending: usize,
    pending_text: usize,
    rows: u64,
    /// Sources with rows here (input tables).
    sources: BTreeSet<SourceId>,
    out: Option<ArrowWriter<W>>,
}

impl<W: Write + Send> Table<W> {
    fn new(name: &'static str, columns: Columns) -> Table<W> {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(n, t, nullable)| Field::new(*n, t.clone(), *nullable))
            .collect();
        Table {
            name,
            schema: Arc::new(Schema::new(fields)),
            cols: columns.iter().map(|(_, t, _)| make_builder(t, 0)).collect(),
            pending: 0,
            pending_text: 0,
            rows: 0,
            sources: BTreeSet::new(),
            out: None,
        }
    }

    /// Appends a row; true when the row group is full.
    fn push(&mut self, cells: Vec<V>) -> bool {
        debug_assert_eq!(cells.len(), self.cols.len(), "{}", self.name);
        for (b, v) in self.cols.iter_mut().zip(cells) {
            if let V::Str(Some(s)) = &v {
                self.pending_text += s.len();
            }
            append(b.as_mut(), v);
        }
        self.pending += 1;
        self.pending == ROW_GROUP_ROWS || self.pending_text >= ROW_GROUP_TEXT_BYTES
    }

    /// Writes the buffered rows as one row group, creating the file first.
    fn flush<F>(&mut self, create: &mut F, props: &WriterProperties) -> io::Result<()>
    where
        F: FnMut(&str) -> io::Result<W>,
    {
        if self.pending == 0 {
            return Ok(());
        }
        let arrays = self.cols.iter_mut().map(|b| b.finish()).collect();
        let batch = RecordBatch::try_new(self.schema.clone(), arrays).map_err(io::Error::other)?;
        let mut out = match self.out.take() {
            Some(out) => out,
            None => {
                ArrowWriter::try_new(create(self.name)?, self.schema.clone(), Some(props.clone()))
                    .map_err(io::Error::other)?
            }
        };
        out.write(&batch).map_err(io::Error::other)?;
        out.flush().map_err(io::Error::other)?;
        self.out = Some(out);
        self.rows += self.pending as u64;
        self.pending = 0;
        self.pending_text = 0;
        Ok(())
    }

    fn evidence_class(&self, declared: &BTreeMap<SourceId, String>) -> String {
        match self.name {
            "input_motion" | "input_events" => {
                let classes: BTreeMap<_, _> =
                    self.sources.iter().map(|s| (s, declared.get(s))).collect();
                serde_json::to_string(&classes).expect("a map of strings serialises")
            }
            "app_results" => "app_reported".into(),
            n if n.starts_with("sim_") => "simulated_truth".into(),
            _ => "observed".into(),
        }
    }
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue::new(key.to_owned(), value.to_owned())
}

/// Exports the committed records of `log` (DATA_SPECIFICATION.md §5.4:
/// records after the last commit are never used). `create(table)` is called
/// once per table that has rows, the first time it has rows to write, and
/// must return the writer for `<table>.parquet`. Returns those tables in
/// [`DERIVED_TABLES`] order.
///
/// `identity_sha256` is the session identity hash (§4), hex; for a
/// recovered session pass the recovered stream and its hash. Reading stops
/// quietly at a torn or corrupt tail like every reader (run
/// [`crate::validate`] / [`crate::recover`] first); only an I/O error or a
/// file that is not KFLOG v1 fails the export.
pub fn export<R, W, F>(log: R, identity_sha256: &str, mut create: F) -> io::Result<Vec<Exported<W>>>
where
    R: Read,
    W: Write + Send,
    F: FnMut(&str) -> io::Result<W>,
{
    let as_io = |e: ReadError| match e {
        ReadError::Io(e) => e,
        e => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
    };
    let mut groups = GroupReader::new(log).map_err(as_io)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .set_key_value_metadata(Some(vec![
            kv("kineforge.format_version", "1"),
            kv("session_identity_sha256", identity_sha256),
            kv("algorithm", ALGORITHM),
            kv("algorithm_version", "1"),
            kv("params", "{}"),
        ]))
        .build();
    let mut tables: Vec<Table<W>> = DERIVED_TABLES
        .iter()
        .zip(COLUMNS)
        .map(|(name, columns)| Table::new(name, columns))
        .collect();
    let mut declared = BTreeMap::new();
    for group in groups.by_ref() {
        for rec in group.records() {
            if let Record::Json {
                kind: Kind::SourceDeclare,
                value,
            } = rec
            {
                let id = value.get("id").and_then(Value::as_u64);
                let class = value.get("evidence_class").and_then(Value::as_str);
                if let (Some(id), Some(class)) = (id.and_then(|i| u16::try_from(i).ok()), class) {
                    declared.insert(id, class.to_owned());
                }
                continue;
            }
            let Some((name, cells)) = row(rec) else {
                continue;
            };
            let t = tables
                .iter_mut()
                .find(|t| t.name == name)
                .expect("row() names a derived table");
            if let Some((source, _)) = rec.source_seq() {
                t.sources.insert(source);
            }
            if t.push(cells) {
                t.flush(&mut create, &props)?;
            }
        }
    }
    if let Some(ReadError::Io(e)) = groups.ending().0.error {
        return Err(e);
    }
    let mut done = Vec::new();
    for mut t in tables {
        t.flush(&mut create, &props)?;
        let evidence = t.evidence_class(&declared);
        if let Some(mut out) = t.out {
            out.append_key_value_metadata(kv("evidence_class", &evidence));
            done.push(Exported {
                table: t.name,
                rows: t.rows,
                out: out.into_inner().map_err(io::Error::other)?,
            });
        }
    }
    Ok(done)
}
