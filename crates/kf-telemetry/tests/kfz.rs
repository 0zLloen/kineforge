//! `.kfz` export/import (DATA_SPECIFICATION.md §4, §12.1): a deterministic
//! round trip of the golden logs, and a malicious-archive suite that must
//! only ever return errors. No file system, so it runs on wasm32 too.

use kf_telemetry::kfz::{DERIVED_TABLES, Error, KfzWriter, Limits, RAW, unpack};
use kf_telemetry::recover::{Sidecar, hex, recovered_stream};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const NO_LIMIT: Limits = Limits {
    total: u64::MAX,
    member: u64::MAX,
};
const FORMAT_MD: &str = include_str!("../FORMAT.md");
const MANIFEST: &[u8] = br#"{"session_id":"6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11"}"#;
const MINIMAL: &[u8] = include_bytes!("golden/minimal.kflog");

type Members = BTreeMap<String, Vec<u8>>;

/// Every golden log as it would be exported: the torn one as its recovered
/// stream (§5.4).
fn golden_raws() -> Vec<(&'static str, Vec<u8>)> {
    let torn: &[u8] = include_bytes!("golden/torn.kflog");
    let sidecar: Sidecar =
        serde_json::from_str(include_str!("golden/torn.recovered.json")).unwrap();
    let mut recovered = Vec::new();
    recovered_stream(torn, &sidecar)
        .unwrap()
        .read_to_end(&mut recovered)
        .unwrap();
    vec![
        ("full", include_bytes!("golden/full.kflog").to_vec()),
        ("minimal", MINIMAL.to_vec()),
        ("torn", recovered),
        ("corrupt", include_bytes!("golden/corrupt.kflog").to_vec()),
    ]
}

/// The members of a full export: raw log, manifest, FORMAT.md and a stand-in
/// for every derived table.
fn members(raw: &[u8]) -> Members {
    let mut m = Members::new();
    m.insert(RAW.into(), raw.to_vec());
    m.insert("manifest.json".into(), MANIFEST.to_vec());
    m.insert("FORMAT.md".into(), FORMAT_MD.as_bytes().to_vec());
    for t in DERIVED_TABLES {
        m.insert(
            format!("{t}.parquet"),
            format!("PAR1 {t} PAR1").into_bytes(),
        );
    }
    m
}

fn pack(m: &Members) -> Vec<u8> {
    let mut w = KfzWriter::new(Cursor::new(Vec::new()));
    for (name, data) in m {
        if name == "FORMAT.md" {
            w.add_format_md().unwrap();
        } else {
            w.add(name, &mut data.as_slice()).unwrap();
        }
    }
    w.finish().unwrap().into_inner()
}

fn unpack_all(z: &[u8], limits: Limits) -> Result<Members, Error> {
    let mut out = Members::new();
    unpack(Cursor::new(z), limits, |name, data| {
        let mut v = Vec::new();
        data.read_to_end(&mut v)?;
        assert!(out.insert(name.into(), v).is_none());
        Ok(())
    })?;
    Ok(out)
}

/// A stored archive built with the zip crate directly, bypassing the
/// writer's checks.
fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(*name, options).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Writes `bytes` into entry `i`'s local header at `local` and its central
/// header at `central` (field offsets from each header's signature).
fn patch(z: &mut [u8], i: usize, local: usize, central: usize, bytes: &[u8]) {
    let a = ZipArchive::new(Cursor::new(&*z)).unwrap();
    let e = a.by_index_data(i).unwrap();
    let (l, c) = (e.header_start() as usize, e.central_header_start() as usize);
    drop(a);
    assert_eq!(&z[l..l + 4], b"PK\x03\x04");
    assert_eq!(&z[c..c + 4], b"PK\x01\x02");
    z[l + local..l + local + bytes.len()].copy_from_slice(bytes);
    z[c + central..c + central + bytes.len()].copy_from_slice(bytes);
}

fn data_start(z: &[u8], name: &str) -> usize {
    let mut a = ZipArchive::new(Cursor::new(z)).unwrap();
    a.by_name(name).unwrap().data_start().unwrap() as usize
}

// Field offsets in the local / central header.
const FLAGS: (usize, usize) = (6, 8);
const METHOD: (usize, usize) = (8, 10);
const CRC: (usize, usize) = (14, 16);
const CSIZE: (usize, usize) = (18, 20);
const USIZE: (usize, usize) = (22, 24);
const NAME: (usize, usize) = (30, 46);

#[test]
fn round_trip_is_exact_and_deterministic() {
    for (log, raw) in golden_raws() {
        let m = members(&raw);
        let z = pack(&m);
        assert!(z == pack(&m), "{log}: packing twice differs");
        assert!(unpack_all(&z, NO_LIMIT).unwrap() == m, "{log}: round trip");
        // Every member STORED, stamped 1980-01-01, no extra fields.
        let mut a = ZipArchive::new(Cursor::new(&z)).unwrap();
        for i in 0..a.len() {
            let e = a.by_index(i).unwrap();
            assert_eq!(e.compression(), CompressionMethod::Stored);
            assert_eq!(e.last_modified().unwrap(), zip::DateTime::default());
            assert!(e.extra_data().is_none_or(|x| x.is_empty()));
        }
    }
}

/// The same inputs give the same archive on every target (no clock, no
/// host-dependent "made by" system). FORMAT.md is left out so editing it
/// does not move the hash.
#[test]
fn archive_bytes_are_pinned() {
    let mut w = KfzWriter::new(Cursor::new(Vec::new()));
    w.add(RAW, &mut &MINIMAL[..]).unwrap();
    w.add("manifest.json", &mut &MANIFEST[..]).unwrap();
    let z = w.finish().unwrap().into_inner();
    assert_eq!(
        hex(&Sha256::digest(&z)),
        "eb3e92dc0d9dc374e74a5e19b1331bd8d08803cbb86d0bd9a8797f110f56cea1"
    );
}

#[test]
fn writer_rejects_bad_members() {
    let mut w = KfzWriter::new(Cursor::new(Vec::new()));
    for bad in [
        "",
        "../raw.kflog",
        "/raw.kflog",
        "derived/frames.parquet",
        "raw.kflog/",
        "trials.parquet",
        "raw.recovered.json",
    ] {
        assert!(
            matches!(w.add(bad, &mut &b"x"[..]), Err(Error::Name(_))),
            "{bad:?}"
        );
    }
    w.add("manifest.json", &mut &MANIFEST[..]).unwrap();
    assert!(matches!(
        w.add("manifest.json", &mut &MANIFEST[..]),
        Err(Error::Duplicate)
    ));
    assert!(matches!(w.finish(), Err(Error::MissingRaw)));
}

#[test]
fn rejects_names_off_the_allow_list() {
    for bad in [
        "../raw.kflog",
        "/raw.kflog",
        "a/raw.kflog",
        "./raw.kflog",
        "a\\raw.kflog",
        "C:raw.kflog",
        "derived/frames.parquet",
        "derived/",
        "RAW.KFLOG",
        "raw.kflog ",
        "x.parquet",
        ".parquet",
        "trials.parquet",
        "raw.recovered.json",
        "read_kflog.py",
    ] {
        let z = raw_zip(&[(RAW, MINIMAL), (bad, b"x")]);
        assert!(
            matches!(unpack_all(&z, NO_LIMIT), Err(Error::Name(n)) if n == bad),
            "{bad:?}"
        );
    }
    let z = raw_zip(&[(RAW, MINIMAL), ("", b"x")]);
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Name(n)) if n.is_empty()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    // A directory entry.
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(RAW, options).unwrap();
    w.write_all(MINIMAL).unwrap();
    w.add_directory("frames.parquet", options).unwrap();
    let z = w.finish().unwrap().into_inner();
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Name(_))));
    // Not UTF-8.
    let mut z = raw_zip(&[(RAW, MINIMAL), ("manifest.jsoX", b"{}")]);
    patch(&mut z, 1, NAME.0 + 12, NAME.1 + 12, b"\xff");
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Name(_))));
}

#[test]
fn rejects_duplicate_names() {
    let mut z = raw_zip(&[(RAW, MINIMAL), ("raw.kflox", MINIMAL)]);
    patch(&mut z, 1, NAME.0 + 8, NAME.1 + 8, b"g");
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Duplicate)));
}

#[test]
fn rejects_compressed_and_encrypted_members() {
    // Deflate (8), deflate64, bzip2, LZMA, zstd and the AES marker (99):
    // the zip crate is built without codecs, so only the field is patched.
    for method in [8u16, 9, 12, 14, 93, 99] {
        let mut z = raw_zip(&[(RAW, MINIMAL)]);
        patch(&mut z, 0, METHOD.0, METHOD.1, &method.to_le_bytes());
        let r = unpack_all(&z, NO_LIMIT);
        assert!(matches!(r, Err(Error::NotStored(_))), "{method}: {r:?}");
    }
    let mut z = raw_zip(&[(RAW, MINIMAL)]);
    patch(&mut z, 0, FLAGS.0, FLAGS.1, &1u16.to_le_bytes());
    let r = unpack_all(&z, NO_LIMIT);
    assert!(matches!(r, Err(Error::Encrypted(_))), "{r:?}");
}

#[test]
fn rejects_size_and_crc_mismatches() {
    let len = MINIMAL.len() as u32;
    let base = raw_zip(&[(RAW, MINIMAL), ("manifest.json", MANIFEST)]);

    // Compressed size ≠ size (both headers agree).
    let mut z = base.clone();
    patch(&mut z, 0, USIZE.0, USIZE.1, &(len + 1).to_le_bytes());
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Size(_))));

    // Local and central headers disagree.
    let mut z = base.clone();
    let mut a = ZipArchive::new(Cursor::new(&z)).unwrap();
    let c = a.by_index(0).unwrap().central_header_start() as usize;
    drop(a);
    z[c + USIZE.1..c + USIZE.1 + 4].copy_from_slice(&(len - 8).to_le_bytes());
    z[c + CSIZE.1..c + CSIZE.1 + 4].copy_from_slice(&(len - 8).to_le_bytes());
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Zip(_))));

    // Declared shorter or longer than the data, in both headers.
    for declared in [0, 1, len - 1, len + 1, len + 8, u32::MAX - 1] {
        let mut z = base.clone();
        patch(&mut z, 0, CSIZE.0, CSIZE.1, &declared.to_le_bytes());
        patch(&mut z, 0, USIZE.0, USIZE.1, &declared.to_le_bytes());
        let r = unpack_all(&z, NO_LIMIT);
        assert!(r.is_err(), "declared {declared}: {r:?}");
    }

    // A flipped data byte, and a wrong CRC in both headers.
    let mut z = base.clone();
    let start = data_start(&z, RAW);
    z[start + 100] ^= 0x40;
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Zip(_))));
    let mut z = base.clone();
    patch(&mut z, 1, CRC.0, CRC.1, &0xdead_beef_u32.to_le_bytes());
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::Zip(_))));
}

#[test]
fn rejects_missing_raw_log() {
    let z = raw_zip(&[("manifest.json", MANIFEST)]);
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::MissingRaw)));
    let z = raw_zip(&[]);
    assert!(matches!(unpack_all(&z, NO_LIMIT), Err(Error::MissingRaw)));
}

#[test]
fn limits_are_checked_before_any_write() {
    let m = members(MINIMAL);
    let z = pack(&m);
    let total: u64 = m.values().map(|v| v.len() as u64).sum();
    let largest = m.values().map(|v| v.len() as u64).max().unwrap();
    for (limits, ok) in [
        (
            Limits {
                total,
                member: largest,
            },
            true,
        ),
        (
            Limits {
                total: total - 1,
                member: largest,
            },
            false,
        ),
        (
            Limits {
                total,
                member: largest - 1,
            },
            false,
        ),
        (
            Limits {
                total: 0,
                member: 0,
            },
            false,
        ),
    ] {
        let mut called = false;
        let r = unpack(Cursor::new(&z), limits, |_, data| {
            called = true;
            io::copy(data, &mut io::sink()).map(drop)
        });
        assert_eq!(r.is_ok(), ok, "{limits:?}: {r:?}");
        if !ok {
            assert!(matches!(r, Err(Error::TooLarge { .. })));
            assert!(!called, "{limits:?}: sink called before the limit check");
        }
    }
}

#[test]
fn sink_errors_and_partial_reads() {
    let z = pack(&members(MINIMAL));
    let r = unpack(Cursor::new(&z), NO_LIMIT, |_, _| {
        Err(io::Error::other("quota"))
    });
    assert!(matches!(r, Err(Error::Io(_))));
    // A sink that stops early still gets the CRC checked (by the drain).
    let mut bad = z.clone();
    bad[data_start(&z, RAW) + MINIMAL.len() - 1] ^= 1;
    let r = unpack(Cursor::new(&bad), NO_LIMIT, |_, data| {
        data.read_exact(&mut [0; 8])
    });
    assert!(matches!(r, Err(Error::Zip(_))), "{r:?}");
}

#[test]
fn garbage_is_rejected() {
    let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
    let mut inputs = vec![
        Vec::new(),
        b"PK".to_vec(),
        b"PK\x05\x06".to_vec(),
        MINIMAL.to_vec(),
    ];
    for len in [22, 100, 4096] {
        inputs.push((0..len).map(|_| next(&mut rng) as u8).collect());
    }
    for input in inputs {
        assert!(unpack_all(&input, NO_LIMIT).is_err());
    }
}

/// Every truncation fails; a single flipped byte anywhere either fails or
/// leaves every member's bytes intact (it hit a field nobody reads, such
/// as a timestamp or "version made by"). Neither may panic.
#[test]
fn truncations_and_byte_flips_never_panic() {
    let m = members(MINIMAL);
    let z = pack(&m);
    for len in 0..z.len() {
        assert!(unpack_all(&z[..len], NO_LIMIT).is_err(), "prefix {len}");
    }
    let mut rng = 0x2545_f491_4f6c_dd1d_u64;
    let mut failed = 0;
    let rounds = 4000;
    for _ in 0..rounds {
        let mut bad = z.clone();
        let at = (next(&mut rng) % z.len() as u64) as usize;
        bad[at] ^= 1 + (next(&mut rng) % 255) as u8;
        match unpack_all(&bad, NO_LIMIT) {
            Err(_) => failed += 1,
            Ok(got) => assert!(got == m, "flip at {at} changed member data"),
        }
    }
    assert!(
        failed > rounds * 3 / 4,
        "only {failed} of {rounds} flips failed"
    );
}

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}
