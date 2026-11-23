//! `kf-cli golden` reproduces the committed golden files; `pack`/`unpack`
//! round-trip a recovered session; malicious archives fail cleanly; the
//! bench hands its last regular group to the writer.

use kf_telemetry::GroupReader;
use kf_telemetry::kfz::KfzWriter;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ID: &str = "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11";

fn tmp(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn cli<I: AsRef<std::ffi::OsStr>>(args: impl IntoIterator<Item = I>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kf-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[track_caller]
fn ok(o: Output) -> String {
    assert!(
        o.status.success(),
        "{:?}: {}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

fn committed() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../kf-telemetry/tests/golden")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn golden_regenerates_the_committed_files() {
    let out = tmp("golden");
    ok(cli([Path::new("golden"), &out]));
    let want = names(&committed());
    assert_eq!(names(&out), want);
    for n in want {
        assert!(
            fs::read(out.join(&n)).unwrap() == fs::read(committed().join(&n)).unwrap(),
            "{n} differs from the committed copy"
        );
    }
    fs::remove_dir_all(out).unwrap();
}

/// A session bundle holding a golden log as its raw.kflog.
fn session(root: &Path, golden: &str) -> PathBuf {
    let dir = root.join("sessions").join(ID);
    fs::create_dir_all(&dir).unwrap();
    fs::copy(committed().join(golden), dir.join("raw.kflog")).unwrap();
    dir
}

fn inspect(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&ok(cli([Path::new("inspect"), dir]))).unwrap()
}

#[test]
fn pack_unpack_round_trip() {
    let t = tmp("pack");
    let src = session(&t.join("a"), "torn.kflog");
    let kfz = t.join("s.kfz");
    // Torn and not recovered yet: nothing to export.
    assert!(
        !cli([Path::new("pack"), &src, Path::new("-o"), &kfz])
            .status
            .success()
    );
    assert!(!kfz.exists());
    ok(cli([Path::new("recover"), &src]));
    ok(cli([Path::new("export"), &src]));
    ok(cli([Path::new("pack"), &src, Path::new("-o"), &kfz]));
    // Never overwrites.
    assert!(
        !cli([Path::new("pack"), &src, Path::new("-o"), &kfz])
            .status
            .success()
    );

    let root = t.join("b");
    ok(cli([Path::new("unpack"), &kfz, &root]));
    let dst = root.join("sessions").join(ID);
    // The archive carries the recovered stream as raw.kflog.
    let mut recovered = Vec::new();
    kf_store::session_stream(&src)
        .unwrap()
        .read_to_end(&mut recovered)
        .unwrap();
    assert!(fs::read(dst.join("raw.kflog")).unwrap() == recovered);
    let derived = names(&src.join("derived"));
    assert_eq!(derived.len(), 12);
    assert_eq!(names(&dst.join("derived")), derived);
    for n in &derived {
        assert!(
            fs::read(src.join("derived").join(n)).unwrap()
                == fs::read(dst.join("derived").join(n)).unwrap()
        );
    }
    assert_eq!(names(&dst), ["derived", "manifest.json", "raw.kflog"]);
    let (a, b) = (inspect(&src), inspect(&dst));
    assert_eq!(a["identity_sha256"], b["identity_sha256"]);
    assert_eq!(b["state"], "recovered");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(dst.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["session_identity_sha256"], a["identity_sha256"]);
    ok(cli([Path::new("validate"), &dst]));

    // The session exists now: refused, and nothing staged is left behind.
    let again = cli([Path::new("unpack"), &kfz, &root]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));
    assert_eq!(names(&root.join("sessions")), [ID]);
    fs::remove_dir_all(t).unwrap();
}

#[test]
fn malicious_archives_fail_cleanly() {
    let t = tmp("malicious");
    let src = session(&t.join("a"), "full.kflog");
    ok(cli([Path::new("export"), &src]));
    let kfz = t.join("good.kfz");
    ok(cli([Path::new("pack"), &src, Path::new("-o"), &kfz]));
    let good = fs::read(&kfz).unwrap();

    let replace = |from: &[u8], to: &[u8]| {
        let mut z = good.clone();
        let mut i = 0;
        while let Some(p) = z[i..].windows(from.len()).position(|w| w == from) {
            z[i + p..i + p + to.len()].copy_from_slice(to);
            i += p + to.len();
        }
        z
    };
    // A path in the name (local and central header agree, so only the
    // allow-list stops it).
    let traversal = replace(b"manifest.json", b"../../etc/pwn");
    // A flipped byte in raw.kflog's data: the ZIP CRC fails at the member's end.
    let mut crc = good.clone();
    let raw_at = good.windows(8).position(|w| w == b"KFLOG\r\n\x1a").unwrap();
    crc[raw_at + 100] ^= 1;
    let truncated = good[..good.len() / 2].to_vec();
    let not_zip = b"KFLOG\r\n\x1a not an archive".to_vec();
    // (case, archive, extra arguments, expected error)
    let cases: [(&str, Vec<u8>, &[&str], &str); 5] = [
        ("traversal", traversal, &[], "not allowed"),
        ("crc", crc, &[], "checksum"),
        ("truncated", truncated, &[], "invalid archive"),
        ("not_zip", not_zip, &[], "invalid archive"),
        (
            "quota",
            good.clone(),
            &["--quota", "1000"],
            "exceeds the limit",
        ),
    ];
    for (name, bytes, extra, error) in cases {
        let path = t.join(format!("{name}.kfz"));
        fs::write(&path, bytes).unwrap();
        let root = t.join(name);
        let o = Command::new(env!("CARGO_BIN_EXE_kf-cli"))
            .arg("unpack")
            .arg(&path)
            .arg(&root)
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(1), "{name}");
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(stderr.contains(error), "{name}: {stderr}");
        // Neither a session nor a staging directory is left.
        assert_eq!(
            names(&root.join("sessions")),
            Vec::<String>::new(),
            "{name}"
        );
    }
    // The good archive itself unpacks.
    ok(cli([Path::new("unpack"), &kfz, &t.join("good")]));
    fs::remove_dir_all(t).unwrap();
}

#[test]
fn unpack_counts_the_manifest_against_the_quota() {
    let t = tmp("quota");
    let raw = fs::read(committed().join("full.kflog")).unwrap();
    let mut z = KfzWriter::new(Cursor::new(Vec::new()));
    z.add("raw.kflog", &mut raw.as_slice()).unwrap();
    let kfz = t.join("raw.kfz");
    fs::write(&kfz, z.finish().unwrap().into_inner()).unwrap();
    let unpack = |root: &Path, quota: usize| {
        let quota = quota.to_string();
        cli([
            Path::new("unpack"),
            &kfz,
            root,
            Path::new("--quota"),
            Path::new(&quota),
        ])
    };
    // raw.kflog fits the quota exactly, the regenerated manifest.json does not.
    let tight = t.join("tight");
    let o = unpack(&tight, raw.len());
    assert_eq!(o.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("manifest.json") && stderr.contains("exceeds the limit of 0 B"),
        "{stderr}"
    );
    assert_eq!(names(&tight.join("sessions")), Vec::<String>::new());
    ok(unpack(&t.join("roomy"), raw.len() + (64 << 10)));
    fs::remove_dir_all(t).unwrap();
}

#[test]
fn closed_stdout_is_not_a_panic() {
    for cmd in ["validate", "inspect"] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let o = Command::new(env!("CARGO_BIN_EXE_kf-cli"))
            .arg(cmd)
            .arg(committed().join("full.kflog"))
            .stdout(writer)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(0), "{cmd}: {stderr}");
        assert!(stderr.is_empty(), "{cmd}: {stderr}");
    }
}

#[test]
fn bench_final_group_holds_only_the_end() {
    let t = tmp("bench");
    let o = cli([
        Path::new("bench"),
        Path::new("--seconds"),
        Path::new("1"),
        Path::new("--out"),
        &t,
    ]);
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let dir = PathBuf::from(r["session_dir"].as_str().unwrap());
    let raw = fs::read(dir.join("raw.kflog")).unwrap();
    // Everything recorded went into regular groups, so `finish` wrote only
    // the commit and session_end, which fit the space reserve.
    let last = GroupReader::new(raw.as_slice()).unwrap().last().unwrap();
    assert_eq!(last.records().count(), 0);
    assert!(r["events_recorded"].as_u64().unwrap() > 0);
    // On Linux the CPU times come from schedstat (ns), not stat's 10 ms ticks.
    for k in ["recorder", "writer", "sync"] {
        if let Some(s) = r["cpu_seconds"][k].as_f64() {
            let ticks = s * 100.0;
            assert!((ticks - ticks.round()).abs() > 1e-6, "{k}: {s} s");
        }
    }
    fs::remove_dir_all(t).unwrap();
}
