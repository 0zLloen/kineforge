//! `manifest.json` of a session bundle (DATA_SPECIFICATION.md §4): a
//! regenerable convenience copy of the header, source declarations,
//! `device_info` records and `session_end` (recovery's synthetic one for a
//! recovered session), the session identity hash and the bundle's file
//! hashes. Built from the session's log stream — `raw.kflog`, or the
//! recovered stream (prefix ‖ synthetic end) — with only committed records.
//! The JSON records are copied as stored, unknown fields included, up to
//! [`DECLARATION_BUDGET`].

use crate::format::Kind;
use crate::json::end_reason;
use crate::reader::{GroupReader, ReadError};
use crate::record::Record;
use crate::recover::hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Read};

/// Schema id written by this version.
pub const SCHEMA: &str = "kineforge.manifest/0.1";

/// Total JSON bytes of source declarations and `device_info` records copied
/// into a manifest; the log is untrusted on import and may hold any number.
pub const DECLARATION_BUDGET: usize = 256 << 10;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    /// SHA-256 of the log stream the manifest was built from: `raw.kflog`,
    /// or the recovered stream (DATA_SPECIFICATION.md §4, §5.4).
    pub session_identity_sha256: String,
    /// The `session_end` is recovery's synthetic one.
    pub recovered: bool,
    pub header: Value,
    pub sources: Vec<Value>,
    pub device_info: Vec<Value>,
    /// Source declarations and `device_info` records left out of the two
    /// lists above because they did not fit [`DECLARATION_BUDGET`].
    /// Serialised only when non-zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub declarations_omitted: u64,
    /// `None` while the log has no terminal `session_end` (live, or torn
    /// and not recovered yet).
    pub session_end: Option<Value>,
    /// Bundle-relative name → lower-case hex SHA-256; filled in by the
    /// caller (`kf-store`), which knows which files exist.
    pub files: BTreeMap<String, String>,
}

/// Hashes everything read through it.
struct HashRead<R> {
    r: R,
    hash: Sha256,
}

impl<R: Read> Read for HashRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let k = self.r.read(buf)?;
        self.hash.update(&buf[..k]);
        Ok(k)
    }
}

/// Lower-case hex SHA-256 of a whole stream.
pub fn sha256_hex<R: Read>(r: R) -> io::Result<String> {
    let mut h = HashRead {
        r,
        hash: Sha256::new(),
    };
    io::copy(&mut h, &mut io::sink())?;
    Ok(hex(&h.hash.finalize()))
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn invalid(e: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

impl Manifest {
    /// Reads a whole log stream. `files` is left empty. Fails on I/O errors,
    /// a stream that is not KFLOG, or one without a committed header; a torn
    /// tail is not an error (`session_end` is then `None`).
    pub fn from_log<R: Read>(r: R) -> io::Result<Manifest> {
        let hr = HashRead {
            r,
            hash: Sha256::new(),
        };
        let mut groups = GroupReader::new(hr).map_err(|e| match e {
            ReadError::Io(e) => e,
            e => invalid(e),
        })?;
        let mut header = None;
        let (mut sources, mut device_info) = (Vec::new(), Vec::new());
        let (mut budget, mut declarations_omitted) = (DECLARATION_BUDGET, 0);
        for g in groups.by_ref() {
            for rec in g.records() {
                let Record::Json { kind, value } = rec else {
                    continue;
                };
                let list = match kind {
                    Kind::SessionHeader if header.is_none() => {
                        header = Some(value.clone());
                        continue;
                    }
                    Kind::SourceDeclare => &mut sources,
                    Kind::DeviceInfo => &mut device_info,
                    _ => continue,
                };
                let len = serde_json::to_vec(value).map_or(usize::MAX, |v| v.len());
                if len <= budget {
                    budget -= len;
                    list.push(value.clone());
                } else {
                    declarations_omitted += 1;
                }
            }
        }
        let (ending, mut rest) = groups.ending();
        if let Some(ReadError::Io(e)) = ending.error {
            return Err(e);
        }
        // The identity covers the whole stream, tail included.
        io::copy(&mut rest, &mut io::sink())?;
        let header = header.ok_or_else(|| invalid("no committed session_header"))?;
        let recovered = ending
            .session_end
            .as_ref()
            .and_then(|e| e.get("end_reason"))
            .and_then(Value::as_str)
            == Some(end_reason::RECOVERED);
        Ok(Manifest {
            schema: SCHEMA.into(),
            session_identity_sha256: hex(&rest.hash.finalize()),
            recovered,
            header,
            sources,
            device_info,
            declarations_omitted,
            session_end: ending.session_end,
            files: BTreeMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden;
    use crate::recover::{Outcome, recover, recovered_stream};

    #[test]
    fn from_golden_logs() {
        let full = golden::full();
        let m = Manifest::from_log(full.as_slice()).unwrap();
        assert_eq!(m.schema, SCHEMA);
        assert_eq!(
            m.session_identity_sha256,
            sha256_hex(full.as_slice()).unwrap()
        );
        assert!(!m.recovered);
        assert_eq!(
            m.header["session_id"],
            "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11"
        );
        assert_eq!((m.sources.len(), m.device_info.len()), (2, 1));
        assert_eq!(m.sources[1]["units"], "dip");
        assert_eq!(m.session_end.unwrap()["end_reason"], "completed");

        // Torn: committed groups only, no end; recovered: the synthetic end.
        let torn = golden::torn();
        let m = Manifest::from_log(torn.as_slice()).unwrap();
        assert!(m.session_end.is_none() && !m.recovered);
        let Outcome::Recovered(sidecar) = recover(torn.as_slice()) else {
            panic!("torn recovers")
        };
        let stream = recovered_stream(torn.as_slice(), &sidecar).unwrap();
        let m = Manifest::from_log(stream).unwrap();
        assert!(m.recovered);
        assert_eq!(m.session_identity_sha256, sidecar.recovered_sha256);

        assert!(Manifest::from_log(&b"not a log"[..]).is_err());
        // Round trip.
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), m);
        assert!(!json.contains("declarations_omitted"));
    }

    #[test]
    fn declarations_beyond_the_budget_are_counted_not_copied() {
        use crate::json::SessionHeader;
        use crate::recorder::{FlushPolicy, Recorder};
        let header = SessionHeader {
            schema: crate::json::SCHEMA.into(),
            session_id: "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11".into(),
            started_at_utc: None,
            clock_domains: Vec::new(),
            simulation: None,
            sensitivity: None,
            rest: BTreeMap::new(),
        };
        let mut r = Recorder::new(&header, FlushPolicy::default()).unwrap();
        let info = |value| Record::Json {
            kind: Kind::DeviceInfo,
            value,
        };
        // 100 KiB each: two fit the 256 KiB budget, three do not; the small
        // one after them still does.
        for _ in 0..5 {
            r.push(info(serde_json::json!({ "a": "x".repeat(100 << 10) })))
                .unwrap();
        }
        r.push(info(serde_json::json!({ "small": 1 }))).unwrap();
        let log = r.cut(kf_core::Nanos(1));
        let m = Manifest::from_log(log.as_slice()).unwrap();
        assert_eq!((m.device_info.len(), m.declarations_omitted), (3, 3));
        assert_eq!(m.device_info[2]["small"], 1);
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"declarations_omitted\":3"));
        assert_eq!(serde_json::from_str::<Manifest>(&json).unwrap(), m);
    }
}
