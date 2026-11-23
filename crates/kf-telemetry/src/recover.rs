//! Recovery, never in place (DATA_SPECIFICATION.md §5.4). One pass over the
//! raw log finds the valid prefix (up to the last commit before the first
//! short read, bound violation or CRC mismatch), hashes it, and builds a
//! deterministic synthetic `session_end`. The recovered byte stream is
//! *prefix ‖ synthetic end chunk*; its SHA-256 is the session identity.
//! The caller (`kf-store`) writes the sidecar next to the untouched
//! `raw.kflog`, and only when the liveness lock is free.

use crate::format::{Kind, write_chunk};
use crate::json::{SessionEnd, end_reason, to_value};
use crate::reader::GroupReader;
use crate::record::encode_json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};

/// `raw.recovered.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub valid_prefix_bytes: u64,
    /// The synthetic `session_end` chunk (framed), standard base64.
    pub synthetic_end_b64: String,
    /// Lower-case hex SHA-256 of prefix ‖ synthetic end.
    pub recovered_sha256: String,
    pub tail_bytes_ignored: u64,
}

impl Sidecar {
    pub fn synthetic_end(&self) -> Option<Vec<u8>> {
        base64_decode(&self.synthetic_end_b64)
    }
}

#[derive(Debug)]
pub enum Outcome {
    /// A CRC-valid `session_end` directly follows a commit: the session was
    /// not dead, and any sidecar is ignored. `sha256` is the identity hash
    /// of the whole file.
    Complete {
        sha256: String,
    },
    Recovered(Sidecar),
    /// No committed group (or not a KFLOG file): nothing to recover.
    Unrecoverable(String),
}

/// Counts bytes and hashes everything read through it.
struct HashRead<R> {
    r: R,
    hash: Sha256,
    n: u64,
}

impl<R: Read> Read for HashRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let k = self.r.read(buf)?;
        self.hash.update(&buf[..k]);
        self.n += k as u64;
        Ok(k)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Scans a whole raw log and decides what recovery means for it.
pub fn recover<R: Read>(r: R) -> Outcome {
    let hr = HashRead {
        r,
        hash: Sha256::new(),
        n: 0,
    };
    let mut groups = match GroupReader::new(hr) {
        Ok(g) => g,
        Err(e) => return Outcome::Unrecoverable(e.to_string()),
    };
    // State at the last commit boundary.
    let mut prefix: Option<(u64, Sha256, i64)> = None;
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    // Not `for`: the hash state is read between groups.
    #[allow(clippy::while_let_on_iterator)]
    while let Some(g) = groups.next() {
        for rec in g.records() {
            *counts.entry(rec.kind().name().to_owned()).or_insert(0) += 1;
        }
        prefix = Some((g.end, groups.get_ref().hash.clone(), g.commit.t_ns.0));
    }
    let (ending, mut rest) = groups.ending();
    if ending.session_end.is_some() {
        // Hash the whole file, including anything after the end.
        if io::copy(&mut rest, &mut io::sink()).is_err() {
            return Outcome::Unrecoverable("read error after session_end".into());
        }
        return Outcome::Complete {
            sha256: hex(&rest.hash.finalize()),
        };
    }
    let Some((valid_prefix_bytes, hash, t_end_ns)) = prefix else {
        return Outcome::Unrecoverable("no committed group".into());
    };
    if io::copy(&mut rest, &mut io::sink()).is_err() {
        return Outcome::Unrecoverable("read error while measuring the tail".into());
    }
    let end = SessionEnd {
        completed: false,
        end_reason: end_reason::RECOVERED.into(),
        t_end_ns,
        counts,
        dropped_chunks: 0,
        max_writer_backlog_bytes: 0,
        interruptions: 0,
        refresh_hz_measured: None,
        raf_hz_measured: None,
        rest: BTreeMap::new(),
    };
    let payload = encode_json(Kind::SessionEnd, &to_value(&end)).expect("synthetic end is small");
    let mut chunk = Vec::new();
    write_chunk(&mut chunk, Kind::SessionEnd, 1, &payload);
    let mut hash = hash;
    hash.update(&chunk);
    Outcome::Recovered(Sidecar {
        valid_prefix_bytes,
        synthetic_end_b64: base64_encode(&chunk),
        recovered_sha256: hex(&hash.finalize()),
        tail_bytes_ignored: rest.n - valid_prefix_bytes,
    })
}

/// The recovered byte stream: the first `valid_prefix_bytes` of the raw log,
/// then the synthetic end chunk. `None` if the sidecar's base64 is invalid.
pub fn recovered_stream<R: Read>(raw: R, sidecar: &Sidecar) -> Option<impl Read + use<R>> {
    let end = sidecar.synthetic_end()?;
    Some(raw.take(sidecar.valid_prefix_bytes).chain(Cursor::new(end)))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 §4).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Strict standard base64 with padding; `None` on any invalid input.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (q, quad) in b.chunks(4).enumerate() {
        let last = q == b.len() / 4 - 1;
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            let v = B64.iter().position(|&x| x == c)? as u32;
            n = (n << 6) | v;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        // Non-canonical encodings (set bits in the padding) are rejected.
        if (pad == 1 && bytes[2] != 0) || (pad == 2 && (bytes[1] != 0 || bytes[2] != 0)) {
            return None;
        }
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors() {
        for (plain, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), plain.as_bytes());
        }
        for bad in ["Zg=", "Z===", "Zh==", "Zg==Zg==", "Zm9v!A==", "===="] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
    }
}
