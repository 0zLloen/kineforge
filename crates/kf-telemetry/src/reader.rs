//! Streaming KFLOG v1 reader. Two layers:
//!
//! - [`ChunkReader`] frames chunks: file header, bounds (payload ≤ 1 MiB and
//!   a multiple of 8, checked before allocating), CRC and the zero trailer.
//!   It stops at the first short read, bound violation or CRC mismatch.
//! - [`GroupReader`] yields committed groups: a group's records count only
//!   once its `commit` chunk has been read (DATA_SPECIFICATION.md §5.4:
//!   records after the last commit are never used). [`Ending`] reports the
//!   terminal `session_end`, the uncommitted tail and why reading stopped.
//!
//! Untrusted input is fine: no length is trusted before it is bounded, and
//! nothing panics on malformed data.

use crate::format::{
    CHUNK_HEADER_LEN, ChunkHeader, FILE_HEADER_LEN, FORMAT_VERSION, KIND_VERSION, Kind, MAGIC,
    MAX_PAYLOAD, chunk_crc,
};
use crate::record::{Commit, DecodeError, Record, decode_payload};
use std::fmt;
use std::io::{self, Read};

/// Why reading stopped before a clean end of file.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    BadMagic,
    UnsupportedVersion(u32),
    /// File header `reserved` or chunk header `reserved` is not zero.
    NonZeroReserved {
        offset: u64,
    },
    /// End of input inside the file header or a chunk.
    Truncated {
        offset: u64,
    },
    PayloadTooLarge {
        offset: u64,
        len: u32,
    },
    PayloadMisaligned {
        offset: u64,
        len: u32,
    },
    CrcMismatch {
        offset: u64,
    },
    /// The 4 bytes after the CRC are not zero.
    NonZeroTrailer {
        offset: u64,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "I/O error: {e}"),
            ReadError::BadMagic => f.write_str("not a KFLOG file (bad magic)"),
            ReadError::UnsupportedVersion(v) => write!(f, "unsupported KFLOG format_version {v}"),
            ReadError::NonZeroReserved { offset } => {
                write!(f, "non-zero reserved field at byte {offset}")
            }
            ReadError::Truncated { offset } => write!(f, "truncated at byte {offset}"),
            ReadError::PayloadTooLarge { offset, len } => {
                write!(f, "chunk at byte {offset}: payload_len {len} exceeds 1 MiB")
            }
            ReadError::PayloadMisaligned { offset, len } => {
                write!(
                    f,
                    "chunk at byte {offset}: payload_len {len} is not a multiple of 8"
                )
            }
            ReadError::CrcMismatch { offset } => write!(f, "chunk at byte {offset}: CRC mismatch"),
            ReadError::NonZeroTrailer { offset } => {
                write!(f, "chunk at byte {offset}: non-zero bytes after the CRC")
            }
        }
    }
}

impl std::error::Error for ReadError {}

/// A CRC-valid chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct RawChunk {
    /// Byte offset of the chunk header in the file.
    pub offset: u64,
    pub header: ChunkHeader,
    pub payload: Vec<u8>,
}

impl RawChunk {
    /// Bytes this chunk occupies: 24 + payload_len.
    pub fn stride(&self) -> u64 {
        24 + u64::from(self.header.payload_len)
    }
}

/// Reads into `buf` until it is full or the input ends; returns bytes read.
fn fill<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Frames chunks from a byte stream.
pub struct ChunkReader<R> {
    r: R,
    /// Offset after the last good chunk (or the file header).
    offset: u64,
    stopped: bool,
}

impl<R: Read> ChunkReader<R> {
    /// Reads and checks the 16-byte file header.
    pub fn new(mut r: R) -> Result<ChunkReader<R>, ReadError> {
        let mut h = [0u8; FILE_HEADER_LEN as usize];
        if fill(&mut r, &mut h).map_err(ReadError::Io)? < h.len() {
            return Err(if h[..8] == MAGIC[..] || h.iter().all(|&b| b == 0) {
                ReadError::Truncated { offset: 0 }
            } else {
                ReadError::BadMagic
            });
        }
        if h[..8] != MAGIC {
            return Err(ReadError::BadMagic);
        }
        let version = u32::from_le_bytes(h[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(ReadError::UnsupportedVersion(version));
        }
        if h[12..16] != [0; 4] {
            return Err(ReadError::NonZeroReserved { offset: 12 });
        }
        Ok(ChunkReader {
            r,
            offset: FILE_HEADER_LEN,
            stopped: false,
        })
    }

    /// Offset just after the last CRC-valid chunk.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn get_ref(&self) -> &R {
        &self.r
    }

    pub fn into_inner(self) -> R {
        self.r
    }

    fn read_chunk(&mut self) -> Result<Option<RawChunk>, ReadError> {
        let offset = self.offset;
        let mut hb = [0u8; CHUNK_HEADER_LEN];
        match fill(&mut self.r, &mut hb).map_err(ReadError::Io)? {
            0 => return Ok(None),
            n if n < hb.len() => return Err(ReadError::Truncated { offset }),
            _ => {}
        }
        let header = ChunkHeader::parse(&hb);
        let len = header.payload_len;
        if len > MAX_PAYLOAD {
            return Err(ReadError::PayloadTooLarge { offset, len });
        }
        if !len.is_multiple_of(8) {
            return Err(ReadError::PayloadMisaligned { offset, len });
        }
        let mut payload = vec![0u8; len as usize];
        let mut trailer = [0u8; 8];
        if fill(&mut self.r, &mut payload).map_err(ReadError::Io)? < payload.len()
            || fill(&mut self.r, &mut trailer).map_err(ReadError::Io)? < trailer.len()
        {
            return Err(ReadError::Truncated { offset });
        }
        if u32::from_le_bytes(trailer[..4].try_into().unwrap()) != chunk_crc(&hb, &payload) {
            return Err(ReadError::CrcMismatch { offset });
        }
        if trailer[4..] != [0; 4] {
            return Err(ReadError::NonZeroTrailer { offset });
        }
        if header.reserved != 0 {
            return Err(ReadError::NonZeroReserved { offset });
        }
        let chunk = RawChunk {
            offset,
            header,
            payload,
        };
        self.offset += chunk.stride();
        Ok(Some(chunk))
    }
}

impl<R: Read> Iterator for ChunkReader<R> {
    type Item = Result<RawChunk, ReadError>;

    /// `None` at a clean end of file; after an error, `None` forever.
    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped {
            return None;
        }
        let r = self.read_chunk().transpose();
        if !matches!(r, Some(Ok(_))) {
            self.stopped = true;
        }
        r
    }
}

/// A CRC-valid chunk, decoded.
#[derive(Clone, Debug, PartialEq)]
pub enum Chunk {
    Records {
        offset: u64,
        kind: Kind,
        records: Vec<Record>,
    },
    /// Unknown kind, unknown `kind_version` or unsupported `flags`: skipped
    /// by `payload_len` and reported (DATA_SPECIFICATION.md §5.3).
    Skipped { offset: u64, header: ChunkHeader },
    /// A known v1 kind whose payload does not decode.
    Undecodable { offset: u64, error: DecodeError },
}

impl Chunk {
    pub fn offset(&self) -> u64 {
        match self {
            Chunk::Records { offset, .. }
            | Chunk::Skipped { offset, .. }
            | Chunk::Undecodable { offset, .. } => *offset,
        }
    }

    /// The known kind, if any.
    pub fn kind(&self) -> Option<Kind> {
        match self {
            Chunk::Records { kind, .. } => Some(*kind),
            Chunk::Undecodable { error, .. } => Some(error.kind),
            Chunk::Skipped { .. } => None,
        }
    }
}

/// Decodes a CRC-valid chunk.
pub fn decode_chunk(raw: &RawChunk) -> Chunk {
    let h = &raw.header;
    match Kind::from_u8(h.kind) {
        Some(kind) if h.kind_version == KIND_VERSION && h.flags == 0 => {
            match decode_payload(kind, h.record_count, &raw.payload) {
                Ok(records) => Chunk::Records {
                    offset: raw.offset,
                    kind,
                    records,
                },
                Err(error) => Chunk::Undecodable {
                    offset: raw.offset,
                    error,
                },
            }
        }
        _ => Chunk::Skipped {
            offset: raw.offset,
            header: *h,
        },
    }
}

/// A committed group: its chunks (the commit chunk excluded) and the commit.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Offset of the group's first chunk.
    pub start: u64,
    /// Offset of its commit chunk.
    pub commit_offset: u64,
    /// Offset just after its commit chunk.
    pub end: u64,
    pub chunks: Vec<Chunk>,
    pub commit: Commit,
}

impl Group {
    /// The group's records in file order (skipped/undecodable chunks omitted).
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.chunks.iter().flat_map(|c| match c {
            Chunk::Records { records, .. } => records.as_slice(),
            _ => &[],
        })
    }
}

/// How the file ended, known once [`GroupReader`] is exhausted.
#[derive(Debug, Default)]
pub struct Ending {
    /// The terminal `session_end` JSON, when it directly follows a commit.
    pub session_end: Option<serde_json::Value>,
    /// Offset just after the last commit (or the file header if none).
    pub committed_end: u64,
    /// Offset just after the last CRC-valid chunk.
    pub valid_end: u64,
    /// Offsets of the CRC-valid chunks after the last commit that no commit
    /// covers (a `session_end` that does not directly follow a commit included).
    pub uncommitted: Vec<u64>,
    /// Offsets of the CRC-valid chunks after the terminal `session_end`.
    pub after_end: Vec<u64>,
    /// Why reading stopped, if not at a clean end of file.
    pub error: Option<ReadError>,
}

/// Yields committed groups; then [`GroupReader::ending`] says how the file ended.
pub struct GroupReader<R> {
    chunks: ChunkReader<R>,
    /// The open group's chunks, kept raw (decoded JSON is ~16× its bytes)
    /// and decoded only once its commit arrives.
    pending: Vec<RawChunk>,
    ending: Ending,
    finished: bool,
}

impl<R: Read> GroupReader<R> {
    pub fn new(r: R) -> Result<GroupReader<R>, ReadError> {
        Ok(GroupReader {
            chunks: ChunkReader::new(r)?,
            pending: Vec::new(),
            ending: Ending {
                committed_end: FILE_HEADER_LEN,
                valid_end: FILE_HEADER_LEN,
                ..Ending::default()
            },
            finished: false,
        })
    }

    /// The underlying reader; everything up to [`ChunkReader::offset`] has
    /// been consumed and nothing more.
    pub fn get_ref(&self) -> &R {
        self.chunks.get_ref()
    }

    /// Reads to the end (discarding remaining groups) and reports how the
    /// file ended.
    pub fn ending(mut self) -> (Ending, R) {
        while self.next().is_some() {}
        (self.ending, self.chunks.into_inner())
    }
}

impl<R: Read> Iterator for GroupReader<R> {
    type Item = Group;

    fn next(&mut self) -> Option<Group> {
        if self.finished {
            return None;
        }
        loop {
            let raw = match self.chunks.next() {
                Some(Ok(raw)) => raw,
                Some(Err(e)) => {
                    self.ending.error = Some(e);
                    break;
                }
                None => break,
            };
            self.ending.valid_end = self.chunks.offset();
            if self.ending.session_end.is_some() {
                self.ending.after_end.push(raw.offset);
                continue;
            }
            if raw.header.kind == Kind::Commit as u8
                && let Chunk::Records { mut records, .. } = decode_chunk(&raw)
                && let Some(Record::Commit(commit)) = records.pop()
            {
                let start = self.pending.first().map_or(raw.offset, |c| c.offset);
                self.ending.committed_end = self.chunks.offset();
                // ponytail: a group is buffered raw and decoded whole, so one
                // huge committed group still costs ~16× its bytes; a maximum
                // group size in FORMAT.md (mirrored in read_kflog.py) would bound it.
                return Some(Group {
                    start,
                    commit_offset: raw.offset,
                    end: self.chunks.offset(),
                    chunks: self.pending.drain(..).map(|c| decode_chunk(&c)).collect(),
                    commit,
                });
            }
            if raw.header.kind == Kind::SessionEnd as u8
                && self.pending.is_empty()
                && self.ending.committed_end > FILE_HEADER_LEN
                && let Chunk::Records { mut records, .. } = decode_chunk(&raw)
                && let Some(Record::Json { value, .. }) = records.pop()
            {
                self.ending.session_end = Some(value);
                continue;
            }
            self.pending.push(raw);
        }
        self.finished = true;
        self.ending.uncommitted = self.pending.drain(..).map(|c| c.offset).collect();
        None
    }
}
