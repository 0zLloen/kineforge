#![cfg(feature = "parquet")]
// The Arrow reader reads a `std::fs::File` (or `bytes::Bytes`, which is not a
// dependency), so each exported file goes through a temp file.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
//! Parquet export of the golden logs, read back with the Arrow reader and
//! compared with the committed dumps (tests/golden/*.jsonl): same values,
//! sentinels as nulls, §8 metadata, identical bytes on a second export.

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Float64Type, Int32Type, Int64Type, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, ArrayRef};
use arrow_schema::{DataType, Schema};
use kf_core::{Motion, Nanos};
use kf_telemetry::json::{SessionHeader, from_value};
use kf_telemetry::kfz::DERIVED_TABLES;
use kf_telemetry::parquet::{ALGORITHM, Exported, ROW_GROUP_ROWS, export};
use kf_telemetry::recover::{Outcome, hex, recover, recovered_stream};
use kf_telemetry::{FlushPolicy, GroupReader, Record, Recorder, golden};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The column contract: `name:type`, `?` = nullable.
const SCHEMAS: [(&str, &str); 12] = [
    (
        "input_motion",
        "t_ns:Int64 src_ts_ns:Int64? dx:Int32 dy:Int32 seq:UInt32 source:UInt16 flags:UInt16",
    ),
    (
        "input_events",
        "type:Utf8 t_ns:Int64 src_ts_ns:Int64? seq:UInt32 source:UInt16 button:UInt8? \
         state:UInt8? dv:Int32? dh:Int32? flags:UInt16?",
    ),
    (
        "frames",
        "frame:UInt64 t_start_ns:Int64 t_input_cutoff_ns:Int64 first_tick:UInt64 ticks:UInt32 \
         flags:UInt16",
    ),
    (
        "frame_timing",
        "frame:UInt64 t_submit_ns:Int64 t_present_ns:Int64? cpu_ns:UInt32 gpu_ns:UInt32?",
    ),
    ("drains", "frame:UInt64 source:UInt16 last_seq:UInt32"),
    (
        "sim_camera",
        "tick:UInt64 yaw_units:Int64 pitch_units:Int64 yaw_rad:Float64 pitch_rad:Float64",
    ),
    (
        "sim_targets",
        "tick:UInt64 target:UInt32 event:UInt8 shape:UInt8 pos_m_x:Float64 pos_m_y:Float64 \
         pos_m_z:Float64 vel_m_s_x:Float64 vel_m_s_y:Float64 vel_m_s_z:Float64 radius_m:Float64",
    ),
    (
        "sim_shots",
        "tick:UInt64 t_ns:Int64 target:UInt32? seq:UInt32 ang_err_rad:Float64 d_yaw_rad:Float64 \
         d_pitch_rad:Float64 hit:Boolean",
    ),
    (
        "sim_control",
        "tick:UInt64 t_ns:Int64 frame:UInt64 action:UInt8 phase:UInt8 reason:UInt16 \
         new_source:UInt16?",
    ),
    (
        "annotations",
        "t_ns:Int64 code:UInt16 source:UInt16? first_seq:UInt32 lost:UInt32? text:Utf8",
    ),
    (
        "app_results",
        "t_ns:Int64? metric:Utf8? value:Float64? unit:Utf8? method:Utf8? transcribed_by:Utf8? \
         json:Utf8",
    ),
    (
        "clock_sync",
        "t_ns:Int64 domain:UInt16 src_ts_ns:Int64 uncertainty_ns:Int64",
    ),
];

type Row = Map<String, Value>;

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn export_mem(log: &[u8], identity: &str) -> Vec<Exported<Vec<u8>>> {
    export(log, identity, |_| Ok(Vec::new())).unwrap()
}

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kf-parquet-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn signature(schema: &Schema) -> String {
    let cols: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| {
            let null = if f.is_nullable() { "?" } else { "" };
            format!("{}:{}{null}", f.name(), f.data_type())
        })
        .collect();
    cols.join(" ")
}

fn cell(c: &ArrayRef, i: usize) -> Value {
    if c.is_null(i) {
        return Value::Null;
    }
    match c.data_type() {
        DataType::Int64 => json!(c.as_primitive::<Int64Type>().value(i)),
        DataType::Int32 => json!(c.as_primitive::<Int32Type>().value(i)),
        DataType::UInt64 => json!(c.as_primitive::<UInt64Type>().value(i)),
        DataType::UInt32 => json!(c.as_primitive::<UInt32Type>().value(i)),
        DataType::UInt16 => json!(c.as_primitive::<UInt16Type>().value(i)),
        DataType::UInt8 => json!(c.as_primitive::<UInt8Type>().value(i)),
        DataType::Float64 => json!(c.as_primitive::<Float64Type>().value(i)),
        DataType::Boolean => json!(c.as_boolean().value(i)),
        DataType::Utf8 => json!(c.as_string::<i32>().value(i)),
        t => panic!("unexpected column type {t}"),
    }
}

struct ReadBack {
    schema: String,
    metadata: BTreeMap<String, String>,
    row_groups: usize,
    rows: Vec<Row>,
}

fn read_back(dir: &Path, t: &Exported<Vec<u8>>) -> ReadBack {
    let path = dir.join(format!("{}.parquet", t.table));
    std::fs::write(&path, &t.out).unwrap();
    let reader =
        ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap()).unwrap();
    let meta = reader.metadata().clone();
    let schema = signature(reader.schema());
    let mut rows = Vec::new();
    for batch in reader.build().unwrap() {
        let batch = batch.unwrap();
        for i in 0..batch.num_rows() {
            let fields = batch.schema_ref().fields().iter();
            rows.push(
                fields
                    .zip(batch.columns())
                    .map(|(f, c)| (f.name().clone(), cell(c, i)))
                    .collect(),
            );
        }
    }
    ReadBack {
        schema,
        metadata: meta
            .file_metadata()
            .key_value_metadata()
            .unwrap()
            .iter()
            .map(|kv| (kv.key.clone(), kv.value.clone().unwrap()))
            .collect(),
        row_groups: meta.num_row_groups(),
        rows,
    }
}

/// The rows each table must hold, from a dump: §6 records renamed and
/// flattened the way the export documents.
fn expected(jsonl: &str) -> BTreeMap<&'static str, Vec<Row>> {
    let mut tables: BTreeMap<_, Vec<Row>> = BTreeMap::new();
    for line in jsonl.lines() {
        let Value::Object(mut o) = serde_json::from_str(line).unwrap() else {
            panic!("dump lines are objects")
        };
        let kind = o.remove("kind").unwrap();
        let table = match kind.as_str().unwrap() {
            "input_motion" => "input_motion",
            k @ ("input_button" | "input_wheel") => {
                o.insert("type".into(), json!(k.trim_start_matches("input_")));
                "input_events"
            }
            "frame" => "frames",
            "frame_timing" => "frame_timing",
            "drain" => "drains",
            "sim_camera" => "sim_camera",
            "sim_target" => {
                for v in ["pos_m", "vel_m_s"] {
                    let xyz = o.remove(v).unwrap();
                    for (i, axis) in ["x", "y", "z"].into_iter().enumerate() {
                        o.insert(format!("{v}_{axis}"), xyz[i].clone());
                    }
                }
                "sim_targets"
            }
            "sim_shot" => "sim_shots",
            "sim_control" => "sim_control",
            "annotation" => "annotations",
            "clock_sync" => "clock_sync",
            "app_result" => {
                let v = o.remove("value").unwrap();
                for k in [
                    "t_ns",
                    "metric",
                    "value",
                    "unit",
                    "method",
                    "transcribed_by",
                ] {
                    o.insert(k.into(), v.get(k).cloned().unwrap_or(Value::Null));
                }
                o.insert("json".into(), json!(v.to_string()));
                "app_results"
            }
            _ => continue,
        };
        tables.entry(table).or_default().push(o);
    }
    tables
}

/// Declared evidence class per source id, from a dump.
fn declared(jsonl: &str) -> BTreeMap<u64, Value> {
    jsonl
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|o| o["kind"] == "source_declare")
        .map(|o| {
            let v = &o["value"];
            (v["id"].as_u64().unwrap(), v["evidence_class"].clone())
        })
        .collect()
}

fn check(name: &str, log: &[u8], jsonl: &str, identity: &str) -> Vec<&'static str> {
    let exported = export_mem(log, identity);
    let bytes = |e: &[Exported<Vec<u8>>]| -> Vec<(&str, u64, Vec<u8>)> {
        e.iter().map(|t| (t.table, t.rows, t.out.clone())).collect()
    };
    assert!(
        bytes(&exported) == bytes(&export_mem(log, identity)),
        "{name}: a second export differs"
    );

    let want = expected(jsonl);
    let names: Vec<_> = exported.iter().map(|t| t.table).collect();
    let want_names: Vec<_> = DERIVED_TABLES
        .into_iter()
        .filter(|t| want.contains_key(t))
        .collect();
    assert_eq!(names, want_names, "{name}: tables, in DERIVED_TABLES order");

    let classes = declared(jsonl);
    let dir = tmp_dir(name);
    for t in &exported {
        let ctx = format!("{name}/{}", t.table);
        let got = read_back(&dir, t);
        let want_rows = &want[t.table];
        assert_eq!(t.rows, want_rows.len() as u64, "{ctx}");
        assert_eq!(got.row_groups, 1, "{ctx}");
        let schema = SCHEMAS.iter().find(|(n, _)| *n == t.table).unwrap().1;
        assert_eq!(got.schema, schema, "{ctx}");

        assert_eq!(got.rows.len(), want_rows.len(), "{ctx}");
        for (i, (row, want_row)) in got.rows.iter().zip(want_rows).enumerate() {
            let mut want_row = want_row.clone();
            for (col, v) in row {
                let w = want_row.remove(col).unwrap_or(Value::Null);
                assert_eq!(*v, w, "{ctx} row {i} column {col}");
            }
            assert!(
                want_row.is_empty(),
                "{ctx} row {i}: fields not exported {want_row:?}"
            );
        }

        let evidence = match t.table {
            "input_motion" | "input_events" => {
                let ids: BTreeSet<u64> = want_rows
                    .iter()
                    .map(|r| r["source"].as_u64().unwrap())
                    .collect();
                Value::Object(
                    ids.into_iter()
                        .map(|id| {
                            (
                                id.to_string(),
                                classes.get(&id).cloned().unwrap_or_default(),
                            )
                        })
                        .collect(),
                )
            }
            "app_results" => json!("app_reported"),
            n if n.starts_with("sim_") => json!("simulated_truth"),
            _ => json!("observed"),
        };
        let m = &got.metadata;
        let got_evidence = &m["evidence_class"];
        assert_eq!(
            serde_json::from_str(got_evidence).unwrap_or_else(|_| json!(got_evidence)),
            evidence,
            "{ctx}"
        );
        for (k, v) in [
            ("kineforge.format_version", "1"),
            ("session_identity_sha256", identity),
            ("algorithm", ALGORITHM),
            ("algorithm_version", "1"),
            ("params", "{}"),
        ] {
            assert_eq!(m[k], v, "{ctx} {k}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
    names
}

#[test]
fn schema_names_are_the_derived_tables() {
    assert_eq!(SCHEMAS.map(|(n, _)| n), DERIVED_TABLES);
}

#[test]
fn full_golden() {
    let log = include_bytes!("golden/full.kflog");
    let tables = check("full", log, include_str!("golden/full.jsonl"), &sha(log));
    assert_eq!(tables, DERIVED_TABLES, "the full golden fills every table");
}

#[test]
fn minimal_golden() {
    let log = include_bytes!("golden/minimal.kflog");
    let tables = check(
        "minimal",
        log,
        include_str!("golden/minimal.jsonl"),
        &sha(log),
    );
    assert_eq!(tables, ["input_motion"]);
}

/// The recovered stream holds the torn log's committed records, so its
/// export matches the torn log's dump.
#[test]
fn torn_golden_recovered() {
    let torn = include_bytes!("golden/torn.kflog");
    let Outcome::Recovered(sidecar) = recover(&torn[..]) else {
        panic!("torn is recoverable")
    };
    let mut stream = Vec::new();
    std::io::copy(
        &mut recovered_stream(&torn[..], &sidecar).unwrap(),
        &mut stream,
    )
    .unwrap();
    let tables = check(
        "torn",
        &stream,
        include_str!("golden/torn.jsonl"),
        &sidecar.recovered_sha256,
    );
    assert!(tables.contains(&"input_motion"));
}

#[test]
fn row_groups_and_undeclared_source() {
    let full = golden::full();
    let first = GroupReader::new(full.as_slice()).unwrap().next().unwrap();
    let Some(Record::Json { value, .. }) = first.records().next() else {
        panic!("the first record is the header")
    };
    let header: SessionHeader = from_value(value).unwrap();
    let mut r = Recorder::new(&header, FlushPolicy::default()).unwrap();
    let n = ROW_GROUP_ROWS as u32 + 10;
    for i in 0..n {
        r.push(Record::Motion(Motion {
            t_ns: Nanos(i64::from(i)),
            src_ts_ns: None,
            dx: 1,
            dy: -1,
            seq: i,
            source: 9,
            flags: 0,
        }))
        .unwrap();
    }
    let end = json!({"completed": true, "end_reason": "completed", "t_end_ns": 0, "counts": {}});
    let log = r
        .finish(from_value(&end).unwrap(), Nanos(i64::from(n)))
        .unwrap();

    let exported = export_mem(&log, &sha(&log));
    assert_eq!(exported.len(), 1);
    let dir = tmp_dir("row-groups");
    let got = read_back(&dir, &exported[0]);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        (exported[0].rows, got.rows.len()),
        (u64::from(n), n as usize)
    );
    assert_eq!(got.row_groups, 2);
    assert_eq!(got.rows[n as usize - 1]["seq"], json!(n - 1));
    assert_eq!(got.metadata["evidence_class"], r#"{"9":null}"#);
}

#[test]
fn not_kflog_is_an_error() {
    let err = export(&b"not a log at all"[..], "", |_| Ok(Vec::new())).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
