//! Typed views of the JSON kinds (DATA_SPECIFICATION.md §3.1, §6.1, §7).
//! Fields that the validator or a consumer needs are typed; everything else
//! stays in `rest`, so unknown fields round-trip (§11: readers ignore them).

use kf_core::{
    ClockKind, ClockMeaning, EvidenceClass, InputLevel, LinearSensitivity, ReceiptMeaning,
    SourceId, Units, ValueScale,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;

/// Schema id written by this version.
pub const SCHEMA: &str = "kineforge.session/0.1";
/// Schema major this reader accepts (`kineforge.session/0.*`).
pub const SCHEMA_PREFIX: &str = "kineforge.session/0.";

/// `session_end.end_reason` values used by KineForge.
pub mod end_reason {
    /// The session ran to its normal end.
    pub const COMPLETED: &str = "completed";
    /// A re-lock or device change would have changed level/units/scale.
    pub const INPUT_LEVEL_CHANGED: &str = "input_level_changed";
    /// Storage ran out or failed; the final group was still written.
    pub const STORAGE_ERROR: &str = "storage_error";
    /// Synthetic end built by recovery (DATA_SPECIFICATION.md §5.4).
    pub const RECOVERED: &str = "recovered";
}

pub type Rest = BTreeMap<String, Value>;

/// `session_header` (kind 0x00).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub schema: String,
    /// UUID ([`is_uuid`]; KineForge writes v4).
    pub session_id: String,
    /// RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SS[.f]Z`); omitted in privacy exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_utc: Option<String>,
    pub clock_domains: Vec<ClockDomain>,
    /// Absent for third-party sessions (§7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulation: Option<Simulation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Sensitivity>,
    #[serde(flatten)]
    pub rest: Rest,
}

/// One clock domain (DATA_SPECIFICATION.md §2.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockDomain {
    pub id: u16,
    pub kind: ClockKind,
    /// `browser_perf` only: cross-origin isolated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coi: Option<bool>,
    pub meaning: ClockMeaning,
    pub resolution_ns: i64,
    pub offset_to_session_ns: i64,
    pub offset_uncertainty_ns: i64,
    pub drift_ppm: f64,
}

/// Header `simulation` object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Simulation {
    /// Must divide 1e9.
    pub sim_hz: u32,
    /// The initial driving source.
    pub input_source: SourceId,
    #[serde(flatten)]
    pub rest: Rest,
}

/// Header `sensitivity` object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sensitivity {
    /// `"linear"` is the only model defined so far.
    pub model: String,
    pub yaw_rad_per_unit: f64,
    pub pitch_rad_per_unit: f64,
    pub sens_x: f64,
    pub sens_y: f64,
    pub invert_y: bool,
    #[serde(flatten)]
    pub rest: Rest,
}

impl Sensitivity {
    /// The camera mapping, if the model is `linear`.
    pub fn linear(&self) -> Option<LinearSensitivity> {
        (self.model == "linear").then_some(LinearSensitivity {
            yaw_rad_per_unit: self.yaw_rad_per_unit,
            pitch_rad_per_unit: self.pitch_rad_per_unit,
            sens_x: self.sens_x,
            sens_y: self.sens_y,
            invert_y: self.invert_y,
        })
    }
}

/// `source_declare` (kind 0x04, DATA_SPECIFICATION.md §3.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDeclaration {
    /// 0..=0xFFFE.
    pub id: SourceId,
    /// Cross-source ordering for analysis; defaults to `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_rank: Option<u32>,
    pub backend: String,
    pub input_level: InputLevel,
    pub units: Units,
    /// 1, 256 or 1024.
    pub value_scale: u16,
    pub clock_domain: u16,
    pub receipt_meaning: ReceiptMeaning,
    pub evidence_class: EvidenceClass,
    #[serde(default)]
    pub processing: Vec<String>,
    #[serde(flatten)]
    pub rest: Rest,
}

impl SourceDeclaration {
    pub fn rank(&self) -> u32 {
        self.source_rank.unwrap_or(u32::from(self.id))
    }

    pub fn scale(&self) -> Option<ValueScale> {
        ValueScale::from_u16(self.value_scale)
    }
}

/// `session_end` (kind 0x7F); the terminal chunk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEnd {
    pub completed: bool,
    /// One of [`end_reason`] or an app-specific reason.
    pub end_reason: String,
    pub t_end_ns: i64,
    /// Records written per kind name, excluding `commit` and `session_end`.
    pub counts: BTreeMap<String, u64>,
    #[serde(default)]
    pub dropped_chunks: u64,
    #[serde(default)]
    pub max_writer_backlog_bytes: u64,
    #[serde(default)]
    pub interruptions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_hz_measured: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raf_hz_measured: Option<f64>,
    #[serde(flatten)]
    pub rest: Rest,
}

/// Serialises a typed JSON record to the `Value` stored in [`crate::Record::Json`].
pub fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("JSON record types always serialise")
}

/// Parses a stored JSON record into its typed view.
pub fn from_value<T: DeserializeOwned>(v: &Value) -> Result<T, serde_json::Error> {
    T::deserialize(v)
}

/// `8-4-4-4-12` hex digits (any UUID version; KineForge writes v4): the
/// `session_id` shape, safe as a directory name.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}
