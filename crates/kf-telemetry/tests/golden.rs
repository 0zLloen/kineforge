//! The committed golden logs (tests/golden/) against the generator and the
//! reader, on every target: the writer must produce the same bytes, the
//! reader the same dump, recovery the same sidecar (ROADMAP M4: "a file
//! written on Linux reads identically on Windows/macOS/wasm"). Regenerate
//! with `kf-cli golden crates/kf-telemetry/tests/golden` after a deliberate
//! format change.

use kf_telemetry::golden;
use kf_telemetry::recover::{Outcome, Sidecar, hex, recover, recovered_stream};
use kf_telemetry::validate::{Level, validate};
use sha2::{Digest, Sha256};

const FILES: [(&str, &[u8], &str); 4] = [
    (
        "full",
        include_bytes!("golden/full.kflog"),
        include_str!("golden/full.jsonl"),
    ),
    (
        "minimal",
        include_bytes!("golden/minimal.kflog"),
        include_str!("golden/minimal.jsonl"),
    ),
    (
        "torn",
        include_bytes!("golden/torn.kflog"),
        include_str!("golden/torn.jsonl"),
    ),
    (
        "corrupt",
        include_bytes!("golden/corrupt.kflog"),
        include_str!("golden/corrupt.jsonl"),
    ),
];

#[test]
fn writer_reproduces_committed_bytes() {
    let generated = golden::all();
    assert_eq!(generated.len(), FILES.len());
    for ((name, bytes), (file, committed, _)) in generated.iter().zip(FILES) {
        assert_eq!(*name, file);
        assert!(
            bytes == committed,
            "{name}: generator output differs from tests/golden/{name}.kflog"
        );
    }
}

#[test]
fn reader_reproduces_committed_dumps() {
    for (name, bytes, jsonl) in FILES {
        let mut out = Vec::new();
        kf_telemetry::dump::dump(bytes, &mut out).unwrap();
        assert!(
            out == jsonl.as_bytes(),
            "{name}: dump differs from tests/golden/{name}.jsonl"
        );
    }
}

fn messages(level: Level, bytes: &[u8]) -> Vec<String> {
    validate(bytes)
        .issues
        .into_iter()
        .filter(|i| i.level == level)
        .map(|i| i.message)
        .collect()
}

#[test]
fn validation_results() {
    let full = validate(FILES[0].1);
    assert!(full.is_valid() && full.complete, "{:?}", full.issues);
    assert_eq!(full.groups, 4);
    assert_eq!(full.counts["input_motion"], 32);
    assert_eq!(full.sources.len(), 2);
    assert_eq!(
        messages(Level::Warning, FILES[0].1),
        [
            "skipped chunk: kind 0x20 version 2 flags 0x0000",
            "skipped chunk: kind 0x40 version 1 flags 0x0000",
            "source 1: 1 seqs missing from 6 without a gap annotation (capture bug)",
        ]
    );
    // The annotated loss on source 0 is reported but not flagged.
    assert!(
        full.losses
            .iter()
            .any(|l| l.source == 0 && l.count == 2 && l.annotated)
    );

    let minimal = validate(FILES[1].1);
    assert!(minimal.is_valid() && minimal.complete && minimal.issues.is_empty());

    assert_eq!(
        messages(Level::Error, FILES[2].1),
        [
            "reading stopped: truncated at byte 3888",
            "no terminal session_end: the session is incomplete (recover it)",
        ]
    );
    assert!(!validate(FILES[3].1).is_valid());
}

#[test]
fn recovery() {
    let sha = |b: &[u8]| hex(&Sha256::digest(b));
    for (name, bytes, _) in [FILES[0], FILES[1]] {
        match recover(bytes) {
            Outcome::Complete { sha256 } => assert_eq!(sha256, sha(bytes), "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    let committed: Sidecar =
        serde_json::from_str(include_str!("golden/torn.recovered.json")).unwrap();
    let Outcome::Recovered(sidecar) = recover(FILES[2].1) else {
        panic!("torn is recoverable")
    };
    assert_eq!(sidecar, committed);
    assert_eq!(
        sidecar.valid_prefix_bytes + sidecar.tail_bytes_ignored,
        FILES[2].1.len() as u64
    );
    // The recovered stream validates as a complete session with that identity.
    let mut stream = Vec::new();
    std::io::copy(
        &mut recovered_stream(FILES[2].1, &sidecar).unwrap(),
        &mut stream,
    )
    .unwrap();
    assert_eq!(sha(&stream), sidecar.recovered_sha256);
    let report = validate(stream.as_slice());
    assert!(report.is_valid() && report.complete, "{:?}", report.issues);
    let end = report.session_end.unwrap();
    assert_eq!(
        (end.completed, end.end_reason.as_str()),
        (false, "recovered")
    );
    // The recovered stream is itself complete; recovering it is a no-op.
    assert!(matches!(
        recover(stream.as_slice()),
        Outcome::Complete { .. }
    ));
    assert!(matches!(recover(FILES[3].1), Outcome::Unrecoverable(_)));
}
