//! `.kfz` session export/import (DATA_SPECIFICATION.md §4 "export", §12.1):
//! a ZIP whose members are all STORED, under flat names from a fixed
//! allow-list, with `raw.kflog` required. Pure: `std::io` traits only and no
//! clock; every member is stamped 1980-01-01 by a Unix "made by" system, so
//! the same inputs in the same order give byte-identical archives on every
//! target. A recovered session is exported with its recovered byte stream
//! (§5.4) as `raw.kflog`, so there is no sidecar member.

use std::fmt;
use std::io::{self, Read, Seek, Write};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipArchive, ZipWriter};

/// Derived Parquet tables of a session bundle (DATA_SPECIFICATION.md §4),
/// re-encoding the raw records; each is `<name>.parquet`. Shared by the
/// Parquet export and the `.kfz` member allow-list.
pub const DERIVED_TABLES: [&str; 12] = [
    "input_motion",
    "input_events",
    "frames",
    "frame_timing",
    "drains",
    "sim_camera",
    "sim_targets",
    "sim_shots",
    "sim_control",
    "annotations",
    "app_results",
    "clock_sync",
];

/// The required member: the raw log, or a recovered session's recovered
/// byte stream.
pub const RAW: &str = "raw.kflog";

const FORMAT_MD: &str = include_str!("../FORMAT.md");

/// Exact matches only, so no path, `..`, absolute or empty name and no
/// directory can pass.
fn allowed(name: &str) -> bool {
    matches!(name, RAW | "manifest.json" | "FORMAT.md")
        || name
            .strip_suffix(".parquet")
            .is_some_and(|t| DERIVED_TABLES.contains(&t))
}

#[derive(Debug)]
pub enum Error {
    /// A member name outside the allow-list (lossy UTF-8).
    Name(String),
    /// A member name appears twice.
    Duplicate,
    /// A member is not STORED.
    NotStored(String),
    Encrypted(String),
    /// A member's compressed and uncompressed sizes differ, or its data is
    /// not as long as declared.
    Size(String),
    MissingRaw,
    /// A member's declared size, or the declared total, exceeds [`Limits`].
    TooLarge {
        size: u64,
        limit: u64,
    },
    /// From the zip crate. Import: malformed or truncated archive, local and
    /// central headers disagreeing, CRC mismatch. Export: the target failed.
    Zip(ZipError),
    /// Export: the member source or the target failed. Import: the sink failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Name(n) => write!(f, "member {n:?} is not allowed in a .kfz"),
            Error::Duplicate => f.write_str("duplicate member name"),
            Error::NotStored(n) => write!(f, "member {n:?} is compressed (only STORED is allowed)"),
            Error::Encrypted(n) => write!(f, "member {n:?} is encrypted"),
            Error::Size(n) => write!(
                f,
                "member {n:?}: data length differs from its declared size"
            ),
            Error::MissingRaw => write!(f, "no {RAW} member"),
            Error::TooLarge { size, limit } => {
                write!(f, "declared size {size} B exceeds the limit of {limit} B")
            }
            Error::Zip(e) => write!(f, "invalid archive: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ZipError> for Error {
    fn from(e: ZipError) -> Self {
        Error::Zip(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Writes a `.kfz`. After any error the archive is unusable; discard it.
/// Dropping the writer without [`KfzWriter::finish`] still lets the zip
/// crate finalise whatever was written, so callers discard on error rather
/// than rely on the target being empty.
pub struct KfzWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    names: Vec<String>,
}

impl<W: Write + Seek> KfzWriter<W> {
    pub fn new(w: W) -> Self {
        KfzWriter {
            zip: ZipWriter::new(w),
            names: Vec::new(),
        }
    }

    /// Adds a STORED member, streaming `data` to its end. Rejects names
    /// outside the allow-list and duplicates.
    // ponytail: no ZIP64, so a member must stay under 4 GiB (the zip crate
    // fails the write beyond); set `large_file` from a size hint if sessions
    // ever get that long.
    pub fn add(&mut self, name: &str, data: &mut dyn Read) -> Result<(), Error> {
        if !allowed(name) {
            return Err(Error::Name(name.into()));
        }
        if self.names.iter().any(|n| n == name) {
            return Err(Error::Duplicate);
        }
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::default())
            .system(System::Unix);
        self.zip.start_file(name, options)?;
        io::copy(data, &mut self.zip)?;
        self.names.push(name.into());
        Ok(())
    }

    /// Adds `FORMAT.md`, the byte-level format description built into this crate.
    pub fn add_format_md(&mut self) -> Result<(), Error> {
        self.add("FORMAT.md", &mut FORMAT_MD.as_bytes())
    }

    /// Writes the central directory and returns the target. Fails without a
    /// `raw.kflog` member.
    pub fn finish(self) -> Result<W, Error> {
        if !self.names.iter().any(|n| n == RAW) {
            return Err(Error::MissingRaw);
        }
        Ok(self.zip.finish()?)
    }
}

/// Bounds on an imported archive's declared sizes.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Sum of all members: the caller's storage quota.
    pub total: u64,
    /// Any one member.
    pub member: u64,
}

/// Counts what the sink reads and notes whether the archive side failed.
struct Counted<R> {
    r: R,
    n: u64,
    failed: bool,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let k = self.r.read(buf).inspect_err(|_| self.failed = true)?;
        self.n += k as u64;
        Ok(k)
    }
}

/// Imports an untrusted `.kfz` (§12.1). The whole directory is checked
/// first: names on the allow-list, no duplicates, local and central headers
/// agreeing (the zip crate's consistency check, which also rejects
/// overlapping and hidden entries), STORED, unencrypted, compressed size =
/// size, every declared size and the declared total within `limits`, and
/// `raw.kflog` present. Only then is `sink` called once per member, in
/// archive order, with its name and data, which never yields more than the
/// declared size. Whatever the sink leaves unread is drained so the CRC is
/// always checked; as a CRC mismatch shows only at a member's end, the sink
/// may already hold data when `unpack` fails, so callers stage members and
/// keep them only on `Ok`.
///
/// Memory while parsing the central directory is linear in the archive's
/// length (the zip crate bounds every count by the bytes behind it), never
/// in a declared size.
pub fn unpack<R: Read + Seek>(
    r: R,
    limits: Limits,
    mut sink: impl FnMut(&str, &mut dyn Read) -> io::Result<()>,
) -> Result<(), Error> {
    let mut zip = ZipArchive::new(r)?;
    if zip.has_duplicate_names() {
        return Err(Error::Duplicate);
    }
    zip.validate_header_consistency()?;
    let mut members = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let e = zip.by_index_data(i)?;
        let name = match std::str::from_utf8(e.name_raw()) {
            Ok(n) if allowed(n) => n.to_owned(),
            _ => return Err(Error::Name(String::from_utf8_lossy(e.name_raw()).into())),
        };
        if e.encrypted() {
            return Err(Error::Encrypted(name));
        }
        if e.compression() != CompressionMethod::Stored {
            return Err(Error::NotStored(name));
        }
        if e.compressed_size() != e.size() {
            return Err(Error::Size(name));
        }
        if e.size() > limits.member {
            return Err(Error::TooLarge {
                size: e.size(),
                limit: limits.member,
            });
        }
        total = total.saturating_add(e.size());
        members.push((name, e.size()));
    }
    if total > limits.total {
        return Err(Error::TooLarge {
            size: total,
            limit: limits.total,
        });
    }
    if !members.iter().any(|(n, _)| n == RAW) {
        return Err(Error::MissingRaw);
    }
    for (i, (name, size)) in members.into_iter().enumerate() {
        let mut data = Counted {
            r: zip.by_index(i)?,
            n: 0,
            failed: false,
        };
        let res = sink(&name, &mut data).and_then(|()| io::copy(&mut data, &mut io::sink()));
        match res {
            Err(e) if data.failed => return Err(Error::Zip(e.into())),
            Err(e) => return Err(Error::Io(e)),
            Ok(_) if data.n != size => return Err(Error::Size(name)),
            Ok(_) => {}
        }
    }
    Ok(())
}
