//! Native session storage (ARCHITECTURE.md §6 "Native", DATA_SPECIFICATION.md
//! §4, §5.2, §5.4): the raw-log writer ([`SessionWriter`]) and the session
//! bundle `sessions/<session-id>/` — directory, ids, recovery sidecar,
//! session stream, identity hash and `manifest.json`.

mod writer;

pub use writer::{RAW, RESERVE, SessionWriter, Status, WriterOptions, sync_dir};

use kf_telemetry::json::is_uuid;
use kf_telemetry::manifest::{Manifest, sha256_hex};
use kf_telemetry::recover::{Outcome, Sidecar, recover};
use std::fs::{self, File, TryLockError};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, BufReader, Cursor, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Recovery sidecar (DATA_SPECIFICATION.md §5.4).
pub const SIDECAR: &str = "raw.recovered.json";
pub const MANIFEST: &str = "manifest.json";
/// Derived tables (`derived/*.parquet`).
pub const DERIVED: &str = "derived";

/// A random UUIDv4 (never time-based: DATA_SPECIFICATION.md §4), from the
/// OS-seeded keys of `std`'s `RandomState`.
pub fn new_session_id() -> String {
    let mut b = [0u8; 16];
    for (i, half) in b.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(i);
        half.copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let x = kf_telemetry::recover::hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &x[..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..]
    )
}

/// `8-4-4-4-12` hex digits: safe as a directory name.
/// The wall clock as RFC 3339 UTC, whole seconds (`started_at_utc`).
pub fn utc_now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (civil-from-days, H. Hinnant).
fn rfc3339(secs: u64) -> String {
    let z = secs / 86_400 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Creates `<root>/sessions/<session_id>/` (it must not exist yet) and
/// returns it. `session_id` must be a UUID (it may come from an imported file).
pub fn create_session_dir(root: &Path, session_id: &str) -> io::Result<PathBuf> {
    if !is_uuid(session_id) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("session id {session_id:?} is not a UUID"),
        ));
    }
    let sessions = root.join("sessions");
    fs::create_dir_all(&sessions)?;
    let dir = sessions.join(session_id);
    fs::create_dir(&dir)?;
    writer::sync_dir(&sessions)?;
    Ok(dir)
}

/// What [`recover_session`] found.
#[derive(Debug)]
pub enum Recovery {
    /// `raw.kflog` ends with a committed `session_end`; nothing written.
    /// `stale_sidecar`: a `raw.recovered.json` exists and is ignored (it is
    /// not deleted).
    Complete { sha256: String, stale_sidecar: bool },
    /// `raw.recovered.json` was written.
    Recovered(Sidecar),
    /// No committed group (or not a KFLOG file); nothing written.
    Unrecoverable(String),
}

/// Recovers a dead session (DATA_SPECIFICATION.md §5.4). Refuses with
/// `ResourceBusy` while the writer holds the liveness lock. Never modifies
/// `raw.kflog`; deletes a leftover space reserve.
pub fn recover_session(dir: &Path) -> io::Result<Recovery> {
    let file = File::open(dir.join(RAW))?;
    // Held until return, so no writer is live. Shared: it works on a
    // read-only fd (NFS refuses an exclusive one), and a concurrent
    // recovery writes the same sidecar.
    match file.try_lock_shared() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(io::Error::new(
                ErrorKind::ResourceBusy,
                "session is live: raw.kflog is locked by its writer",
            ));
        }
        Err(TryLockError::Error(e)) => return Err(e),
    }
    let _ = fs::remove_file(dir.join(RESERVE));
    Ok(match recover(BufReader::new(&file)) {
        Outcome::Complete { sha256 } => Recovery::Complete {
            sha256,
            stale_sidecar: dir.join(SIDECAR).exists(),
        },
        Outcome::Recovered(sidecar) => {
            let mut json = serde_json::to_vec_pretty(&sidecar)?;
            json.push(b'\n');
            fs::write(dir.join(SIDECAR), json)?;
            Recovery::Recovered(sidecar)
        }
        Outcome::Unrecoverable(why) => Recovery::Unrecoverable(why),
    })
}

/// The session's log stream: `raw.kflog`, or with a sidecar the recovered
/// stream (prefix ‖ synthetic end). A sidecar applies only if recovery of
/// `raw.kflog` reproduces it exactly; it is ignored when `raw.kflog` is
/// complete, and any other mismatch is an `InvalidData` error.
// ponytail: verifying the sidecar rescans raw.kflog; cache if export latency matters.
pub fn session_stream(dir: &Path) -> io::Result<Box<dyn Read>> {
    let raw = dir.join(RAW);
    let open = || File::open(&raw).map(BufReader::new);
    let sidecar: Sidecar = match fs::read(dir.join(SIDECAR)) {
        Ok(json) => serde_json::from_slice(&json)?,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Box::new(open()?)),
        Err(e) => return Err(e),
    };
    match recover(open()?) {
        Outcome::Complete { .. } => Ok(Box::new(open()?)),
        Outcome::Recovered(s) if s == sidecar => {
            let end = s.synthetic_end().expect("recover encodes valid base64");
            Ok(Box::new(
                open()?.take(s.valid_prefix_bytes).chain(Cursor::new(end)),
            ))
        }
        _ => Err(io::Error::new(
            ErrorKind::InvalidData,
            "raw.recovered.json does not match raw.kflog (run recovery again)",
        )),
    }
}

/// The session identity hash (DATA_SPECIFICATION.md §4): SHA-256 of
/// [`session_stream`].
pub fn identity_sha256(dir: &Path) -> io::Result<String> {
    sha256_hex(session_stream(dir)?)
}

/// Builds the manifest from [`session_stream`], hashes the bundle files
/// present (`raw.kflog`, `raw.recovered.json`, `derived/*`), and writes
/// `manifest.json`.
pub fn write_manifest(dir: &Path) -> io::Result<Manifest> {
    write_built_manifest(dir, Manifest::from_log(session_stream(dir)?)?, u64::MAX)
}

/// [`write_manifest`] with `m` already built from [`session_stream`].
/// Refuses with `QuotaExceeded`, writing nothing, when `manifest.json` would
/// be longer than `max_len` bytes.
pub fn write_built_manifest(dir: &Path, mut m: Manifest, max_len: u64) -> io::Result<Manifest> {
    let mut names = vec![RAW.to_owned(), SIDECAR.to_owned()];
    match fs::read_dir(dir.join(DERIVED)) {
        Ok(entries) => {
            for e in entries {
                let e = e?;
                if e.file_type()?.is_file()
                    && let Some(name) = e.file_name().to_str()
                {
                    names.push(format!("{DERIVED}/{name}"));
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    for name in names {
        match File::open(dir.join(&name)) {
            Ok(f) => {
                m.files.insert(name, sha256_hex(BufReader::new(f))?);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let mut json = serde_json::to_vec_pretty(&m)?;
    json.push(b'\n');
    if json.len() as u64 > max_len {
        return Err(io::Error::new(
            ErrorKind::QuotaExceeded,
            format!(
                "{MANIFEST} ({} B) exceeds the limit of {max_len} B",
                json.len()
            ),
        ));
    }
    fs::write(dir.join(MANIFEST), json)?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_civil_from_days() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_794_690_245), "2026-11-14T21:04:05Z");
        assert_eq!(rfc3339(4_107_542_399), "2100-02-28T23:59:59Z");
        assert_eq!(utc_now_rfc3339().len(), 20);
    }

    #[test]
    fn session_ids_are_uuid_v4() {
        let (a, b) = (new_session_id(), new_session_id());
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert!(is_uuid(id), "{id}");
            assert_eq!(&id[14..15], "4");
            assert!("89ab".contains(&id[19..20]), "{id}");
        }
        assert!(!is_uuid("../../../../etc/passwd-0000-0000-0000"));
        assert!(create_session_dir(Path::new("."), "..").is_err());
    }
}
