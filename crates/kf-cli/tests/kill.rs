//! PERFORMANCE_PLAN N9b: data loss on process kill. `kf-cli bench` runs for
//! 10 s and is SIGKILLed at a random time 1–6 s in, at a random point of
//! its 100 ms cut cycle (progress lines come about 5 ms after each cut, so
//! the test sleeps a random 0..`FLUSH_NS` after the line before killing);
//! recovery must write a sidecar without touching `raw.kflog` and delete
//! the space reserve, the recovered stream must validate, and the loss —
//! the generated `t_ns` at the kill (the last line's plus the time since
//! it arrived) minus the recovered `t_end_ns` (the last commit) — must stay
//! within `chunk_flush_ms` + the largest writer backlog reported by that
//! line and the ones drained after the kill + [`ALLOWANCE_NS`].

use serde_json::Value;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FLUSH_NS: i64 = 100_000_000;
/// The kill time is extrapolated from the last line's receipt, which is at
/// most a pipe hop after the bench printed it; this covers what the lines
/// cannot show: the recorder's cut lag (it wakes every 5 ms), a group
/// submitted after the line and still in the writer at the kill, and
/// scheduling jitter on a loaded machine.
const ALLOWANCE_NS: i64 = 50_000_000;
const RUNS: u64 = 3;

fn cli(args: &[&Path]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kf-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn sigkill_loss_is_within_the_n9b_bound() {
    if !cfg!(unix) {
        eprintln!("skipped: Child::kill is SIGKILL only on Unix");
        return;
    }
    for run in 0..RUNS {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("kill-{run}"));
        let _ = fs::remove_dir_all(&root);
        let kill_after = Duration::from_millis(1000 + RandomState::new().hash_one(run) % 5000);
        let mut child = Command::new(env!("CARGO_BIN_EXE_kf-cli"))
            .args(["bench", "--seconds", "10", "--out"])
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let progress = |line: std::io::Result<String>| {
            // `ok()?`: the kill can tear the last line (eprintln! writes it
            // piecewise); every line before it is whole.
            let v: Value = serde_json::from_str(&line.unwrap()).ok()?;
            v.get("t_ns").is_some().then_some(v)
        };
        let mut last = None;
        for line in lines.by_ref() {
            last = progress(line).or(last);
            if start.elapsed() >= kill_after {
                break;
            }
        }
        let seen = Instant::now();
        std::thread::sleep(Duration::from_nanos(
            RandomState::new().hash_one(run) % FLUSH_NS as u64,
        ));
        let killed = Instant::now();
        child.kill().unwrap();
        let last: Value = last.expect("progress lines");
        // Lines written before the kill are still in the pipe: a group stuck
        // in the writer at the kill shows in their backlog.
        let backlog = lines
            .filter_map(progress)
            .chain([last.clone()])
            .map(|v| v["backlog_ns"].as_i64().unwrap())
            .max()
            .unwrap();
        assert!(
            !child.wait().unwrap().success(),
            "run {run}: bench ended before the kill"
        );

        let sessions: Vec<_> = fs::read_dir(root.join("sessions")).unwrap().collect();
        assert_eq!(sessions.len(), 1);
        let dir = sessions.into_iter().next().unwrap().unwrap().path();
        let raw = fs::read(dir.join("raw.kflog")).unwrap();
        let rec = cli(&[Path::new("recover"), &dir]);
        assert!(
            rec.status.success(),
            "{}",
            String::from_utf8_lossy(&rec.stderr)
        );
        assert!(String::from_utf8_lossy(&rec.stdout).starts_with("recovered"));
        assert!(
            fs::read(dir.join("raw.kflog")).unwrap() == raw,
            "recovery touched raw.kflog"
        );
        assert!(!dir.join("raw.kflog.reserve").exists());
        let val = cli(&[Path::new("validate"), &dir]);
        assert!(
            val.status.success(),
            "{}",
            String::from_utf8_lossy(&val.stdout)
        );
        let inspect: Value =
            serde_json::from_slice(&cli(&[Path::new("inspect"), &dir]).stdout).unwrap();
        assert_eq!(inspect["state"], "recovered");
        assert_eq!(
            inspect["losses"].as_array().map(Vec::len),
            Some(0),
            "seq gaps"
        );

        let t_end = inspect["session_end"]["t_end_ns"].as_i64().unwrap();
        let generated = last["t_ns"].as_i64().unwrap() + (killed - seen).as_nanos() as i64;
        let loss = generated - t_end;
        eprintln!(
            "run {run}: killed {:?} after a line: loss {:.1} ms, reported backlog {:.1} ms",
            killed - seen,
            loss as f64 / 1e6,
            backlog as f64 / 1e6
        );
        assert!(
            loss <= FLUSH_NS + backlog + ALLOWANCE_NS,
            "run {run}: loss {loss} ns > {FLUSH_NS} + backlog {backlog} + {ALLOWANCE_NS}"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
