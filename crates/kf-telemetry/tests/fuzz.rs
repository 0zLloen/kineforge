//! Seeded mutation fuzzing of every untrusted-input path (ROADMAP M4,
//! DATA_SPECIFICATION.md §12.1). Inputs are the golden logs, the torn log's
//! recovered stream and `.kfz` archives packed from them. Mutations are
//! byte-level (flips, overwrites, inserts, deletes, truncation), chunk-level
//! (duplicate, remove, reorder, splice) and targeted: `payload_len`,
//! `record_count`, kind/version/flags, JSON edits, annotation lengths, commit
//! counts, ZIP header and directory fields, duplicate entries, ZIP64 records.
//! Most chunk attacks re-seal the CRC so they reach the decoders.
//!
//! Every log runs the group reader, `validate`, `dump` and `recover` (and
//! the recovered stream through `validate` and `recover` again); with
//! feature `parquet`, one log in four also runs the Parquet export. Every
//! archive runs `kfz::unpack` with tight limits. Nothing may panic, no case
//! may take seconds, and no unpacked member may exceed its limit.
//!
//! Case `i` of a seed is the same input on every target, so a failure names
//! its case and `run(seed, i, 1)` replays it. `fuzz_quick` runs by default
//! (also on wasm32: no threads, no files); the long run is
//! `cargo test --release -p kf-telemetry [--features parquet] --test fuzz -- --ignored --nocapture`.

use kf_telemetry::format::{Kind, MAX_PAYLOAD, chunk_crc};
use kf_telemetry::kfz::{KfzWriter, Limits, RAW, unpack};
use kf_telemetry::recover::{Outcome, hex, recover, recovered_stream};
use kf_telemetry::validate::validate;
use kf_telemetry::{Chunk, GroupReader};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::{self, Cursor, Read, Write};
use std::time::{Duration, Instant};

/// About 3 s in debug natively (each log case parses its JSON 4–6 times
/// through unoptimised serde_json and SHA-256); 5 s on wasm32.
const QUICK: u64 = 5_000;
const LONG: u64 = 1_000_000;
const CASE_LIMIT: Duration = Duration::from_secs(3);
/// Tight against the seeds (the largest member is 5.4 KiB), and tighter.
const LIMITS: [Limits; 2] = [
    Limits {
        total: 32 << 10,
        member: 8 << 10,
    },
    Limits {
        total: 4 << 10,
        member: 2 << 10,
    },
];

const LOGS: [&[u8]; 4] = [
    include_bytes!("golden/full.kflog"),
    include_bytes!("golden/minimal.kflog"),
    include_bytes!("golden/torn.kflog"),
    include_bytes!("golden/corrupt.kflog"),
];

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let z = (self.0 ^ (self.0 >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

/// Little-endian integer of `width` bytes at `at`.
fn get(b: &[u8], at: usize, width: usize) -> u64 {
    let mut x = [0u8; 8];
    x[..width].copy_from_slice(&b[at..at + width]);
    u64::from_le_bytes(x)
}

fn put(b: &mut [u8], at: usize, width: usize, v: u64) {
    b[at..at + width].copy_from_slice(&v.to_le_bytes()[..width]);
}

/// An edge value for an integer field of `width` bytes holding `cur`.
fn edge(rng: &mut Rng, width: usize, cur: u64) -> u64 {
    let max = if width == 8 {
        u64::MAX
    } else {
        (1 << (8 * width)) - 1
    };
    let v = match rng.below(17) {
        0 => 0,
        1 => 1,
        2 => 7,
        3 => 8,
        4 => max,
        5 => max >> 1,
        6 => (max >> 1) + 1,
        7 => cur.wrapping_add(1),
        8 => cur.wrapping_sub(1),
        9 => cur.wrapping_add(8),
        10 => cur.wrapping_sub(8),
        // 8 × n and 24 + n wrap a 32-bit usize.
        11 => 0x2000_0000,
        12 => 0xFFFF_FFF9,
        13 => u64::from(MAX_PAYLOAD),
        14 => u64::from(MAX_PAYLOAD) + 8,
        15 => f64::NAN.to_bits(),
        _ => rng.next(),
    };
    v & max
}

/// Generic byte-level mutation.
fn bytes_op(rng: &mut Rng, b: &mut Vec<u8>) {
    let at = rng.below(b.len() + 1);
    let r = rng.next();
    match rng.below(6) {
        0 if at < b.len() => b[at] ^= 1 << (r % 8),
        1 if at < b.len() => b[at] = rng.pick(&[0, 1, 0x7F, 0x80, 0xFF, r as u8]),
        2 => {
            let n = 1 + rng.below(16);
            b.splice(at..at, r.to_le_bytes().into_iter().cycle().take(n));
        }
        3 if at < b.len() => {
            let end = b.len().min(at + 1 + rng.below(16));
            b.drain(at..end);
        }
        4 => b.truncate(at),
        _ if at + 4 <= b.len() => {
            let v = edge(rng, 4, get(b, at, 4));
            put(b, at, 4, v);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- KFLOG

/// A log as its file header, framed chunks (header ‖ payload ‖ CRC ‖ zeros)
/// and a tail that does not frame.
#[derive(Clone)]
struct Log {
    head: Vec<u8>,
    chunks: Vec<Vec<u8>>,
    tail: Vec<u8>,
}

fn split(b: &[u8]) -> Log {
    let mut at = b.len().min(16);
    let head = b[..at].to_vec();
    let mut chunks = Vec::new();
    while b.len() - at >= 24 {
        let len = get(b, at, 4) as usize;
        if len > b.len() - at - 24 {
            break;
        }
        chunks.push(b[at..at + 24 + len].to_vec());
        at += 24 + len;
    }
    Log {
        head,
        chunks,
        tail: b[at..].to_vec(),
    }
}

fn join(log: Log) -> Vec<u8> {
    let mut out = log.head;
    for c in log.chunks {
        out.extend_from_slice(&c);
    }
    out.extend_from_slice(&log.tail);
    out
}

/// Rewrites a framed chunk's CRC over its own header and payload.
fn reseal(c: &mut [u8]) {
    let n = c.len() - 8;
    let crc = chunk_crc(c[..16].try_into().unwrap(), &c[16..n]);
    c[n..n + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Gives a framed chunk a payload of `len` bytes (cut, or grown with
/// `fill`) and a matching `payload_len`.
fn resize_payload(c: &mut Vec<u8>, len: usize, fill: u8) {
    let trailer = c.split_off(c.len() - 8);
    c.resize(16 + len, fill);
    c.extend_from_slice(&trailer);
    put(c, 0, 4, len as u64);
}

/// A chunk for which `want` holds, else any chunk.
fn choose(rng: &mut Rng, chunks: &[Vec<u8>], want: impl Fn(&[u8]) -> bool) -> usize {
    let of: Vec<usize> = (0..chunks.len()).filter(|&i| want(&chunks[i])).collect();
    if of.is_empty() {
        rng.below(chunks.len())
    } else {
        rng.pick(&of)
    }
}

fn leaf(rng: &mut Rng) -> Value {
    match rng.below(18) {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(0),
        3 => json!(1),
        4 => json!(-1),
        5 => json!(3),
        6 => json!(1024),
        7 => json!(0xFFFF),
        8 => json!(u32::MAX),
        9 => json!(u64::MAX),
        10 => json!(i64::MIN),
        11 => json!(1e308),
        12 => json!(-0.0),
        13 => json!(""),
        14 => json!("linear"),
        15 => json!("kineforge.session/0.9"),
        16 => json!([]),
        _ => json!({}),
    }
}

/// Replaces, removes or adds one node somewhere in `v`.
fn poke(rng: &mut Rng, v: &mut Value) {
    const KEYS: [&str; 8] = [
        "simulation",
        "sensitivity",
        "source_rank",
        "coi",
        "counts",
        "id",
        "x",
        "",
    ];
    match v {
        Value::Object(m) if !m.is_empty() && !rng.one_in(4) => {
            let k = m.keys().nth(rng.below(m.len())).unwrap().clone();
            match rng.below(6) {
                0 => drop(m.remove(&k)),
                1 => drop(m.insert(rng.pick(&KEYS).into(), leaf(rng))),
                _ => poke(rng, m.get_mut(&k).unwrap()),
            }
        }
        Value::Array(a) if !a.is_empty() && !rng.one_in(4) => {
            if rng.one_in(6) {
                a.push(a[0].clone());
            } else {
                let i = rng.below(a.len());
                poke(rng, &mut a[i]);
            }
        }
        _ => *v = leaf(rng),
    }
}

/// An edited JSON payload, padded to 8 (usually with spaces).
fn edit_json(rng: &mut Rng, payload: &[u8]) -> Vec<u8> {
    let mut doc = match serde_json::from_slice::<Value>(payload) {
        Ok(mut v) if !rng.one_in(8) => {
            for _ in 0..1 + rng.below(3) {
                poke(rng, &mut v);
            }
            serde_json::to_vec(&v).unwrap()
        }
        _ => match rng.below(6) {
            0 => b"[]".to_vec(),
            1 => b"{\"a\":1,\"a\":2}".to_vec(),
            2 => b"{\"\xff\":1}".to_vec(),
            3 => b"{\"schema\":1e400}".to_vec(),
            4 => b"{".to_vec(),
            _ => {
                let d = rng.pick(&[127, 128, 200, 5000]);
                format!("{{\"a\":{}{}}}", "[".repeat(d), "]".repeat(d)).into_bytes()
            }
        },
    };
    if rng.one_in(8) {
        doc.truncate(rng.below(doc.len() + 1));
    }
    let pad = if rng.one_in(16) { 0 } else { b' ' };
    doc.resize(doc.len().next_multiple_of(8), pad);
    doc
}

/// One structural or field attack on the chunks; most re-seal the CRC.
fn attack(rng: &mut Rng, log: &mut Log, donors: &[Log]) {
    let ch = &mut log.chunks;
    if ch.is_empty() {
        return;
    }
    let n = ch.len();
    let op = rng.below(13);
    match op {
        0 => {
            let c = ch[rng.below(n)].clone();
            ch.insert(rng.below(n + 1), c);
        }
        1 => drop(ch.remove(rng.below(n))),
        2 => ch.swap(rng.below(n), rng.below(n)),
        3 => {
            let d = &donors[rng.below(donors.len())].chunks;
            let c = d[rng.below(d.len())].clone();
            ch.insert(rng.below(n + 1), c);
        }
        4 => ch.truncate(rng.below(n)),
        _ => {
            let i = match op {
                10 => choose(rng, ch, |c| c.len() > 24 && c[16] == b'{'),
                11 => choose(rng, ch, |c| c[4] == Kind::Annotation as u8),
                12 => choose(rng, ch, |c| c[4] == Kind::Commit as u8),
                _ => rng.below(n),
            };
            let c = &mut ch[i];
            if field_attack(rng, op, c) && !rng.one_in(16) {
                reseal(c);
            }
        }
    }
}

/// Attacks one framed chunk; false if the CRC must stay as it is.
fn field_attack(rng: &mut Rng, op: usize, c: &mut Vec<u8>) -> bool {
    let len = c.len() - 24;
    let fill = rng.pick(&[0, b' ', 0xFF]);
    match op {
        // payload_len alone: the framing breaks.
        5 => {
            let v = edge(rng, 4, get(c, 0, 4));
            put(c, 0, 4, v);
        }
        // A payload of another length, framed consistently.
        6 => {
            let new = match rng.below(7) {
                0 => 0,
                1 => len + 8,
                2 => len.saturating_sub(8),
                3 => len + 1 + rng.below(7),
                4 if rng.one_in(8) => MAX_PAYLOAD as usize,
                _ => 8 * rng.below(64),
            };
            resize_payload(c, new, fill);
        }
        7 => match rng.below(5) {
            0 => {
                let v = edge(rng, 4, get(c, 8, 4));
                put(c, 8, 4, v);
            }
            1 => {
                c[4] = if rng.one_in(4) {
                    rng.next() as u8
                } else {
                    rng.pick(Kind::ALL) as u8
                }
            }
            2 => c[5] = rng.pick(&[0, 2, 0xFF]),
            3 => put(c, 6, 2, rng.pick(&[1, 2, 0x8000, 0xFFFF])),
            _ => put(c, 12, 4, rng.pick(&[1, 0x8000_0000])),
        },
        // A field inside the payload.
        8 => {
            let w = rng.pick(&[1, 2, 4, 8]);
            if len >= w {
                let at = 16 + rng.below(len / w) * w;
                let v = edge(rng, w, get(c, at, w));
                put(c, at, w, v);
            }
        }
        // CRC or the zero bytes after it.
        9 => {
            let at = c.len() - 8 + rng.below(8);
            c[at] ^= 1 << rng.below(8);
            return false;
        }
        10 => {
            let doc = edit_json(rng, &c[16..16 + len]);
            resize_payload(c, doc.len(), 0);
            c[16..16 + doc.len()].copy_from_slice(&doc);
        }
        11 => annotation_attack(rng, c, len, fill),
        _ => commit_attack(rng, c, len, fill),
    }
    true
}

/// Annotation record fields (24-byte header: t_ns, code, source, first_seq,
/// lost, len; then text padded to 8).
fn annotation_attack(rng: &mut Rng, c: &mut Vec<u8>, len: usize, fill: u8) {
    let mut recs = Vec::new();
    let mut at = 0u64;
    while len as u64 >= at + 24 {
        recs.push(at as usize);
        at += 24 + get(c, 16 + at as usize + 20, 4).next_multiple_of(8);
    }
    let Some(&r) = recs.get(rng.below(recs.len().max(1))) else {
        return;
    };
    let f = 16 + r;
    match rng.below(5) {
        0 | 1 => {
            let rest = (len - r - 24) as u64;
            let v = match rng.below(3) {
                0 => rest + rng.pick(&[0, 1, 8]),
                _ => edge(rng, 4, get(c, f + 20, 4)),
            };
            put(c, f + 20, 4, v);
        }
        2 if len > r + 24 => c[f + 24] = rng.pick(&[0xFF, 0xC3, 0x80]),
        3 => {
            let (at, w) = rng.pick(&[(8, 2), (10, 2), (12, 4), (16, 4)]);
            let v = edge(rng, w, get(c, f + at, w));
            put(c, f + at, w, v);
        }
        _ => resize_payload(c, r + 8 * rng.below(3), fill),
    }
}

/// Commit fields (t_ns, tick, frame, n, pad; then n × (source, pad, last_seq)).
fn commit_attack(rng: &mut Rng, c: &mut Vec<u8>, len: usize, fill: u8) {
    if len < 32 {
        return;
    }
    let n = get(c, 16 + 24, 4);
    match rng.below(5) {
        0 | 1 => {
            let v = edge(rng, 4, n);
            put(c, 16 + 24, 4, v);
        }
        // One entry more or less, consistently.
        2 if rng.one_in(2) || len < 40 => {
            resize_payload(c, len + 8, fill);
            put(c, 16 + 24, 4, n.wrapping_add(1));
        }
        2 => {
            resize_payload(c, len - 8, fill);
            put(c, 16 + 24, 4, n.wrapping_sub(1));
        }
        3 => {
            let at = 16 + 8 * rng.below(4);
            let v = edge(rng, 8, get(c, at, 8));
            put(c, at, 8, v);
        }
        _ if len >= 40 => {
            let e = 16 + 32 + 8 * rng.below((len - 32) / 8);
            let (at, w) = rng.pick(&[(0, 2), (2, 2), (4, 4)]);
            let v = edge(rng, w, get(c, e + at, w));
            put(c, e + at, w, v);
        }
        _ => {}
    }
}

fn mutate_log(rng: &mut Rng, logs: &[Log]) -> Vec<u8> {
    // `full` (index 0) has every kind and four groups: weight it up.
    let mut log = logs[rng.pick(&[0, 0, 0, 1, 2, 3, 4])].clone();
    let structured = if rng.one_in(8) { 0 } else { 1 + rng.below(3) };
    for _ in 0..structured {
        if rng.one_in(16) {
            match rng.below(3) {
                0 => log.head[rng.below(16)] = rng.next() as u8,
                1 => log.head[8] = rng.pick(&[0, 2, 0xFF]),
                _ => log.head[12 + rng.below(4)] = 1,
            }
        } else {
            attack(rng, &mut log, logs);
        }
    }
    let mut b = join(log);
    let raw = match structured {
        0 => 1 + rng.below(3),
        _ => usize::from(rng.one_in(16)),
    };
    for _ in 0..raw {
        bytes_op(rng, &mut b);
    }
    b
}

// ---------------------------------------------------------------- .kfz

struct Zip {
    bytes: Vec<u8>,
    /// Local and central header offsets of each entry.
    entries: Vec<(usize, usize)>,
    eocd: usize,
}

fn zip_seed(bytes: Vec<u8>) -> Zip {
    let a = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let entries = (0..a.len())
        .map(|i| {
            let e = a.by_index_data(i).unwrap();
            (e.header_start() as usize, e.central_header_start() as usize)
        })
        .collect();
    drop(a);
    let eocd = bytes.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    Zip {
        bytes,
        entries,
        eocd,
    }
}

/// (local offset, central offset, width) of fields both headers carry:
/// flags, method, CRC, compressed size, size, name length, extra length.
const SHARED: [(usize, usize, usize); 7] = [
    (6, 8, 2),
    (8, 10, 2),
    (14, 16, 4),
    (18, 20, 4),
    (22, 24, 4),
    (26, 28, 2),
    (28, 30, 2),
];
/// Central only: made by, version needed, comment length, disk, attributes, local offset.
const CENTRAL: [(usize, usize); 6] = [(4, 2), (6, 2), (32, 2), (34, 2), (38, 4), (42, 4)];
/// End of central directory: disks, entry counts, directory size and offset, comment length.
const EOCD: [(usize, usize); 7] = [(4, 2), (6, 2), (8, 2), (10, 2), (12, 4), (16, 4), (20, 2)];

fn central_len(b: &[u8], c: usize) -> usize {
    46 + (get(b, c + 28, 2) + get(b, c + 30, 2) + get(b, c + 32, 2)) as usize
}

fn mutate_kfz(rng: &mut Rng, z: &Zip) -> Vec<u8> {
    let mut b = z.bytes.clone();
    let fields = rng.below(3);
    for _ in 0..fields {
        let (l, c) = rng.pick(&z.entries);
        match rng.below(6) {
            // Both sizes, in both headers: a consistent lie.
            0 => {
                let v = edge(rng, 4, get(&b, c + 24, 4));
                for at in [l + 18, l + 22, c + 20, c + 24] {
                    put(&mut b, at, 4, v);
                }
            }
            1 => {
                let (lo, co, w) = rng.pick(&SHARED);
                let v = if lo == 8 && rng.one_in(2) {
                    // Deflate, deflate64, bzip2, LZMA, zstd, AES.
                    rng.pick(&[1, 8, 9, 12, 14, 93, 99])
                } else {
                    edge(rng, w, get(&b, c + co, w))
                };
                let where_ = rng.below(4);
                if where_ != 1 {
                    put(&mut b, l + lo, w, v);
                }
                if where_ != 0 {
                    put(&mut b, c + co, w, v);
                }
            }
            2 => {
                let (o, w) = rng.pick(&CENTRAL);
                let v = edge(rng, w, get(&b, c + o, w));
                put(&mut b, c + o, w, v);
            }
            3 => {
                let (o, w) = rng.pick(&EOCD);
                let v = edge(rng, w, get(&b, z.eocd + o, w));
                put(&mut b, z.eocd + o, w, v);
            }
            // A name: another entry's of the same length (a duplicate), or one bad byte.
            _ => {
                let n = get(&z.bytes, c + 28, 2) as usize;
                let same: Vec<usize> = z
                    .entries
                    .iter()
                    .filter(|&&(_, c2)| c2 != c && get(&z.bytes, c2 + 28, 2) as usize == n)
                    .map(|&(_, c2)| c2)
                    .collect();
                let name = if same.is_empty() || rng.one_in(3) {
                    let mut name = z.bytes[c + 46..c + 46 + n].to_vec();
                    name[rng.below(n)] = rng.pick(&[b'/', b'.', b'\\', 0, 0xFF, b'A']);
                    name
                } else {
                    let c2 = rng.pick(&same);
                    z.bytes[c2 + 46..c2 + 46 + n].to_vec()
                };
                let where_ = rng.below(3);
                if where_ != 1 {
                    b[l + 30..l + 30 + n].copy_from_slice(&name);
                }
                if where_ != 0 {
                    b[c + 46..c + 46 + n].copy_from_slice(&name);
                }
            }
        }
    }
    let i = rng.below(z.entries.len());
    let c = z.entries[i].1;
    let n = central_len(&z.bytes, c);
    let e = z.eocd;
    match rng.below(if fields == 0 { 4 } else { 8 }) {
        // A second central record for an entry.
        0 => {
            let rec = b[c..c + n].to_vec();
            b.splice(e..e, rec);
            for (o, w, d) in [(8, 2, 1), (10, 2, 1), (12, 4, n as u64)] {
                let v = get(&b, e + n + o, w).wrapping_add(d);
                put(&mut b, e + n + o, w, v);
            }
        }
        // An entry left out of the directory.
        1 => {
            b.drain(c..c + n);
            let e = e - n;
            for (o, w, d) in [(8, 2, 1), (10, 2, 1), (12, 4, n as u64)] {
                let v = get(&b, e + o, w).wrapping_sub(d);
                put(&mut b, e + o, w, v);
            }
        }
        // A ZIP64 end record and locator, real or edge values.
        2 => {
            let real = [
                get(&b, e + 8, 2),
                get(&b, e + 10, 2),
                get(&b, e + 12, 4),
                get(&b, e + 16, 4),
            ];
            let mut rec = b"PK\x06\x06".to_vec();
            rec.extend_from_slice(&44u64.to_le_bytes());
            rec.extend_from_slice(&[45, 3, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            for v in real {
                let v = if rng.one_in(2) { edge(rng, 8, v) } else { v };
                rec.extend_from_slice(&v.to_le_bytes());
            }
            rec.extend_from_slice(b"PK\x06\x07\0\0\0\0");
            rec.extend_from_slice(&(e as u64).to_le_bytes());
            rec.extend_from_slice(&1u32.to_le_bytes());
            let k = rec.len();
            b.splice(e..e, rec);
            for (o, w) in [(8, 2), (10, 2), (12, 4), (16, 4)] {
                if rng.one_in(2) {
                    put(&mut b, e + k + o, w, u64::MAX);
                }
            }
        }
        3 => bytes_op(rng, &mut b),
        _ => {}
    }
    b
}

// ---------------------------------------------------------------- targets

#[derive(Default)]
struct Stats(BTreeMap<String, u64>);

impl Stats {
    fn add(&mut self, key: &str, n: u64) {
        match self.0.get_mut(key) {
            Some(v) => *v += n,
            None => drop(self.0.insert(key.to_owned(), n)),
        }
    }
}

/// `Variant` of an enum's `Debug` output.
fn variant(e: &dyn Debug) -> String {
    let s = format!("{e:?}");
    s.split(['(', ' ', '{'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// `parquet`: also run the Parquet export (feature `parquet`).
fn log_targets(data: &[u8], parquet: bool, st: &mut Stats) {
    st.add("run/reader", 1);
    match GroupReader::new(data) {
        Ok(mut groups) => {
            let mut chunks = |cs: &[Chunk]| {
                for c in cs {
                    let key = match c {
                        Chunk::Records { .. } => "chunk/Records",
                        Chunk::Skipped { .. } => "chunk/Skipped",
                        Chunk::Undecodable { .. } => "chunk/Undecodable",
                    };
                    st.add(key, 1);
                }
            };
            let mut end = 0;
            for g in groups.by_ref() {
                assert!(g.end > end && g.start >= end);
                end = g.end;
                chunks(&g.chunks);
            }
            let (e, _) = groups.ending();
            assert!(e.committed_end <= e.valid_end && e.valid_end <= data.len() as u64);
            let mut tail = e.uncommitted.iter().chain(&e.after_end);
            assert!(tail.all(|&o| o >= end && o < e.valid_end));
            let stop = e.error.as_ref().map_or("clean".into(), |e| variant(e));
            st.add(&format!("stop/{stop}"), 1);
        }
        Err(e) => st.add(&format!("stop/{}", variant(&e)), 1),
    }

    st.add("run/validate", 1);
    let report = validate(data);
    assert!(!report.is_valid() || report.complete);
    st.add(
        if report.is_valid() {
            "validate/valid"
        } else {
            "validate/invalid"
        },
        1,
    );

    st.add("run/dump", 1);
    kf_telemetry::dump::dump(data, &mut io::sink()).unwrap();

    st.add("run/recover", 1);
    let outcome = recover(data);
    st.add(&format!("recover/{}", variant(&outcome)), 1);
    match outcome {
        Outcome::Complete { sha256 } => assert_eq!(sha256, sha(data)),
        Outcome::Recovered(sc) => {
            assert_eq!(
                sc.valid_prefix_bytes + sc.tail_bytes_ignored,
                data.len() as u64
            );
            let mut stream = Vec::new();
            recovered_stream(data, &sc)
                .unwrap()
                .read_to_end(&mut stream)
                .unwrap();
            assert!(validate(stream.as_slice()).complete);
            match recover(stream.as_slice()) {
                Outcome::Complete { sha256 } => assert_eq!(sha256, sc.recovered_sha256),
                other => panic!("a recovered stream recovers again: {other:?}"),
            }
        }
        Outcome::Unrecoverable(_) => {}
    }

    #[cfg(not(feature = "parquet"))]
    let _ = parquet;
    #[cfg(feature = "parquet")]
    if parquet {
        st.add("run/parquet", 1);
        let r = kf_telemetry::parquet::export(data, &"0".repeat(64), |_| Ok(io::sink()));
        let key = r.map_or_else(
            |e| format!("parquet/{:?}", e.kind()),
            |_| "parquet/Ok".into(),
        );
        st.add(&key, 1);
    }
}

fn kfz_target(z: &[u8], limits: Limits, st: &mut Stats) {
    st.add("run/unpack", 1);
    let mut total = 0u64;
    let r = unpack(Cursor::new(z), limits, |name, data| {
        let mut v = Vec::new();
        data.take(limits.member + 1).read_to_end(&mut v)?;
        assert!(
            v.len() as u64 <= limits.member,
            "{name}: over the member limit"
        );
        total += v.len() as u64;
        assert!(total <= limits.total, "over the total limit");
        Ok(())
    });
    let key = r.as_ref().map_or_else(|e| variant(e), |()| "Ok".into());
    st.add(&format!("unpack/{key}"), 1);
}

// ---------------------------------------------------------------- runner

struct Corpus {
    logs: Vec<Log>,
    kfz: Vec<Zip>,
}

fn pack(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = KfzWriter::new(Cursor::new(Vec::new()));
    for (name, data) in members {
        w.add(name, &mut &data[..]).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn corpus() -> Corpus {
    let torn = LOGS[2];
    let Outcome::Recovered(sc) = recover(torn) else {
        panic!("torn is recoverable")
    };
    let mut recovered = Vec::new();
    recovered_stream(torn, &sc)
        .unwrap()
        .read_to_end(&mut recovered)
        .unwrap();
    let [full, minimal, _, corrupt] = LOGS;

    // ZIP64 extra fields on the raw log (KfzWriter never writes them).
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file(RAW, stored.large_file(true)).unwrap();
    w.write_all(&recovered).unwrap();
    w.start_file("manifest.json", stored).unwrap();
    w.write_all(b"{}").unwrap();
    let zip64 = w.finish().unwrap().into_inner();

    let stand_ins: Vec<(String, Vec<u8>)> = kf_telemetry::kfz::DERIVED_TABLES
        .iter()
        .map(|t| (format!("{t}.parquet"), b"PAR1".to_vec()))
        .collect();
    let mut tables: Vec<(&str, &[u8])> = vec![(RAW, corrupt)];
    tables.extend(stand_ins.iter().map(|(n, d)| (n.as_str(), d.as_slice())));

    let mut logs: Vec<Log> = LOGS.iter().map(|b| split(b)).collect();
    logs.push(split(&recovered));
    Corpus {
        logs,
        kfz: vec![
            zip_seed(pack(&[(RAW, minimal)])),
            // FORMAT.md and raw.kflog, frames and drains: names of equal length.
            zip_seed(pack(&[
                (RAW, full),
                ("manifest.json", b"{}"),
                ("FORMAT.md", b"# KFLOG"),
                ("frames.parquet", b"PAR1"),
                ("drains.parquet", b"PAR1"),
            ])),
            zip_seed(zip64),
            zip_seed(pack(&tables)),
        ],
    }
}

/// Names the failing case while a panic unwinds (native; wasm aborts).
struct CaseGuard(u64, u64);

impl Drop for CaseGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("fuzz: case {} of seed {:#x} failed", self.1, self.0);
        }
    }
}

fn one_case(corpus: &Corpus, seed: u64, i: u64, st: &mut Stats) {
    let mut rng = Rng(seed ^ i.wrapping_mul(0xD1B5_4A32_D192_ED03));
    if rng.one_in(4) {
        let z = &corpus.kfz[rng.below(corpus.kfz.len())];
        let limits = LIMITS[usize::from(rng.one_in(4))];
        kfz_target(&mutate_kfz(&mut rng, z), limits, st);
    } else {
        let log = mutate_log(&mut rng, &corpus.logs);
        // ponytail: Parquet on 1 log case in 4 keeps `cargo test --workspace`
        // (kf-cli turns the feature on) at seconds in debug; drop the
        // sampling if the export grows its own untrusted-input handling.
        log_targets(&log, rng.one_in(4), st);
    }
}

/// Runs cases `first..first + n` of `seed`.
// The clock only times cases; nothing here is part of the format.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn run(seed: u64, first: u64, n: u64) -> Stats {
    let corpus = corpus();
    let mut st = Stats::default();
    let mut slowest = (Duration::ZERO, first);
    let start = Instant::now();
    for i in first..first + n {
        let _guard = CaseGuard(seed, i);
        let t = Instant::now();
        one_case(&corpus, seed, i, &mut st);
        let dt = t.elapsed();
        assert!(dt < CASE_LIMIT, "case {i} took {dt:?}");
        slowest = slowest.max((dt, i));
    }
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "fuzz: {n} cases of seed {seed:#x} in {secs:.1} s ({:.0} cases/s); slowest case {} took {:?}",
        n as f64 / secs,
        slowest.1,
        slowest.0
    );
    for (k, v) in &st.0 {
        eprintln!("  {k:<28} {v}");
    }
    st
}

#[test]
fn fuzz_quick() {
    let st = run(0x4B46_4C4F_4700_0001, 0, QUICK);
    // The mutations reach every outcome, so the decoders and checks behind
    // the CRC and the ZIP directory really ran.
    for key in [
        "validate/valid",
        "validate/invalid",
        "recover/Complete",
        "recover/Recovered",
        "recover/Unrecoverable",
        "chunk/Records",
        "chunk/Skipped",
        "chunk/Undecodable",
        "stop/clean",
        "stop/CrcMismatch",
        "stop/Truncated",
        "stop/PayloadTooLarge",
        "stop/PayloadMisaligned",
        "stop/BadMagic",
        "stop/NonZeroTrailer",
        "stop/UnsupportedVersion",
        "stop/NonZeroReserved",
        "unpack/Ok",
        "unpack/Zip",
        "unpack/TooLarge",
        "unpack/Duplicate",
        "unpack/Name",
    ] {
        assert!(st.0.contains_key(key), "no case reached {key}");
    }
}

#[test]
#[ignore = "long: run with --release -- --ignored --nocapture"]
fn fuzz_long() {
    run(0x4B46_4C4F_4700_1000, 0, LONG);
}
