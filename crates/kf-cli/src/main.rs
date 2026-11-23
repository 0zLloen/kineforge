//! `kf-cli` v0 (ARCHITECTURE.md §2, ROADMAP M4): headless tools for raw logs
//! and session bundles. Arguments are parsed by hand (std only). Exit codes:
//! 0 success, 1 failure (or validation errors), 2 bad command line.

mod bench;

use kf_store::{DERIVED, MANIFEST, Recovery, SIDECAR};
use kf_telemetry::json::end_reason;
use kf_telemetry::kfz::{self, DERIVED_TABLES, KfzWriter, Limits};
use kf_telemetry::manifest::{Manifest, sha256_hex};
use kf_telemetry::recover::{Outcome, recover};
use kf_telemetry::validate::{Report, validate};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage: kf-cli <command> [arguments]

  validate <raw.kflog | session-dir>    validate a log; exit 1 on errors
  inspect <file | session-dir>          summary as JSON (header, sources, counts,
                                        groups, state, losses, identity hash)
  dump <file | session-dir>             canonical JSON lines
  recover <session-dir>                 write raw.recovered.json for a dead session
  export <session-dir>                  write derived/<table>.parquet and manifest.json
  pack <session-dir> -o <file.kfz>      export a session as a stored ZIP
  unpack <file.kfz> <root> [--quota BYTES]
                                        import into <root>/sessions/<session-id>/
  golden <dir>                          write the golden example logs
  bench [--seconds 10] [--sources 3] [--rate 8000] [--frames 1000] [--out DIR]
        [--fsync-ms 1000] [--flush-ms 100]
                                        synthetic telemetry run (N9a); JSON on stdout,
                                        progress lines on stderr

A session-dir with a raw.recovered.json sidecar is read as its recovered stream.
";

const COMMANDS: [&str; 9] = [
    "validate", "inspect", "dump", "recover", "export", "pack", "unpack", "golden", "bench",
];

type Res<T = ExitCode> = Result<T, Box<dyn Error>>;

/// A bad command line: usage text and exit 2.
#[derive(Debug)]
struct Usage(String);

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Usage {}

fn usage<T>(msg: String) -> Res<T> {
    Err(Box::new(Usage(msg)))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) if e.is::<Usage>() => {
            eprintln!("kf-cli: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
        // `kf-cli dump … | head`
        Err(e)
            if e.downcast_ref::<io::Error>().map(io::Error::kind)
                == Some(io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kf-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

type Opts<'a> = BTreeMap<&'a str, &'a str>;

/// Positional arguments and `--flag value` options (only `flags` accepted).
fn parse<'a>(args: &'a [String], flags: &[&str]) -> Res<(Vec<&'a str>, Opts<'a>)> {
    let (mut pos, mut opts) = (Vec::new(), Opts::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !(a.starts_with('-') && a.len() > 1) {
            pos.push(a.as_str());
            continue;
        }
        if !flags.contains(&a.as_str()) {
            return usage(format!("unknown option {a}"));
        }
        let Some(v) = it.next() else {
            return usage(format!("{a} needs a value"));
        };
        if opts.insert(a.as_str(), v.as_str()).is_some() {
            return usage(format!("{a} given twice"));
        }
    }
    Ok((pos, opts))
}

/// A numeric option, or `default`.
fn num(opts: &Opts, key: &str, default: u64) -> Res<u64> {
    match opts.get(key) {
        None => Ok(default),
        Some(v) => v
            .parse()
            .or_else(|_| usage(format!("{key}: {v:?} is not a non-negative integer"))),
    }
}

fn run(args: &[String]) -> Res {
    let Some((cmd, rest)) = args.split_first() else {
        return usage("no command".into());
    };
    if matches!(cmd.as_str(), "help" | "-h" | "--help") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let flags: &[&str] = match cmd.as_str() {
        "pack" => &["-o"],
        "unpack" => &["--quota"],
        "bench" => &bench::FLAGS,
        _ => &[],
    };
    let (pos, opts) = parse(rest, flags)?;
    match (cmd.as_str(), pos.as_slice()) {
        ("validate", [p]) => cmd_validate(Path::new(p)),
        ("inspect", [p]) => inspect(Path::new(p)),
        ("dump", [p]) => {
            let mut out = BufWriter::new(io::stdout().lock());
            kf_telemetry::dump::dump(open(Path::new(p))?, &mut out)?;
            out.flush()?;
            Ok(ExitCode::SUCCESS)
        }
        ("recover", [d]) => cmd_recover(session_dir(d)?),
        ("export", [d]) => export(session_dir(d)?),
        ("pack", [d]) => match opts.get("-o") {
            Some(out) => pack(session_dir(d)?, Path::new(out)),
            None => usage("pack needs -o <file.kfz>".into()),
        },
        // Without a quota, the members written are still bounded by the
        // archive's length (STORED, sizes checked by kfz::unpack), and the
        // regenerated manifest.json by Manifest's DECLARATION_BUDGET.
        ("unpack", [z, root]) => unpack(
            Path::new(z),
            Path::new(root),
            num(&opts, "--quota", u64::MAX)?,
        ),
        ("golden", [d]) => golden(Path::new(d)),
        ("bench", []) => bench::run(&opts),
        (c, _) if COMMANDS.contains(&c) => usage(format!("wrong arguments for {c}")),
        (c, _) => usage(format!("unknown command {c:?}")),
    }
}

fn session_dir(p: &str) -> Res<&Path> {
    let p = Path::new(p);
    if p.is_dir() {
        Ok(p)
    } else {
        usage(format!("{} is not a session directory", p.display()))
    }
}

/// A log file, or a session directory's log stream (the recovered stream if
/// it has a sidecar; `kf_store::session_stream`).
fn open(path: &Path) -> io::Result<Box<dyn Read>> {
    let r: io::Result<Box<dyn Read>> = if path.is_dir() {
        kf_store::session_stream(path)
    } else {
        File::open(path).map(|f| Box::new(BufReader::new(f)) as _)
    };
    r.map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// `complete`, `recovered` (synthetic end) or `incomplete`.
fn state(r: &Report) -> &'static str {
    match &r.session_end {
        Some(e) if e.end_reason == end_reason::RECOVERED => "recovered",
        Some(_) => "complete",
        None => "incomplete",
    }
}

fn cmd_validate(path: &Path) -> Res {
    let r = validate(open(path)?);
    // writeln!, not println!: a closed pipe is an error main() handles.
    let mut out = io::stdout().lock();
    for issue in &r.issues {
        writeln!(out, "{issue}")?;
    }
    let (errors, warnings) = r.issue_counts();
    let hidden = r.suppressed_errors + r.suppressed_warnings;
    if hidden > 0 {
        writeln!(out, "... {hidden} more issues not shown")?;
    }
    writeln!(
        out,
        "{}: {} ({errors} errors, {warnings} warnings, {} groups)",
        path.display(),
        state(&r),
        r.groups
    )?;
    Ok(if r.is_valid() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

// ponytail: a session dir is read 4 times (sidecar check + validate, twice
// for the hash); hash while validating if inspect of huge logs gets slow.
fn inspect(path: &Path) -> Res {
    let r = validate(open(path)?);
    let identity = if path.is_dir() {
        kf_store::identity_sha256(path)?
    } else {
        sha256_hex(BufReader::new(File::open(path)?))?
    };
    let (errors, warnings) = r.issue_counts();
    let header = r.header.as_ref().map(|h| {
        json!({
            "session_id": h.session_id,
            "schema": h.schema,
            "started_at_utc": h.started_at_utc,
            "app": h.rest.get("app"),
            "simulation": h.simulation.as_ref().map(|s| json!({"sim_hz": s.sim_hz, "input_source": s.input_source})),
        })
    });
    let losses: Vec<Value> = r
        .losses
        .iter()
        .map(|l| json!({"source": l.source, "first_seq": l.first_seq, "count": l.count, "annotated": l.annotated}))
        .collect();
    let out = json!({
        "identity_sha256": identity,
        "state": state(&r),
        "valid": r.is_valid(),
        "errors": errors,
        "warnings": warnings,
        "header": header,
        "sources": r.sources.values().collect::<Vec<_>>(),
        "groups": r.groups,
        "counts": r.counts,
        "losses": losses,
        "session_end": r.session_end,
    });
    let json = serde_json::to_string_pretty(&out)?;
    writeln!(io::stdout().lock(), "{json}")?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_recover(dir: &Path) -> Res {
    let outcome = kf_store::recover_session(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match outcome {
        Recovery::Complete {
            sha256,
            stale_sidecar,
        } => {
            println!("complete: nothing to recover (identity sha256 {sha256})");
            if stale_sidecar {
                println!("note: the existing {SIDECAR} is stale and ignored");
            }
            Ok(ExitCode::SUCCESS)
        }
        Recovery::Recovered(s) => {
            println!("recovered: wrote {SIDECAR}");
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(ExitCode::SUCCESS)
        }
        Recovery::Unrecoverable(why) => {
            eprintln!("kf-cli: unrecoverable: {why}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The manifest of a session that has a terminal `session_end` (complete
/// or recovered); refuses live and torn ones.
fn ended_manifest(dir: &Path) -> Res<Manifest> {
    let m = Manifest::from_log(kf_store::session_stream(dir)?)?;
    if m.session_end.is_none() {
        return Err(format!(
            "{}: no session_end (live or torn); run `kf-cli recover` first",
            dir.display()
        )
        .into());
    }
    Ok(m)
}

fn export(dir: &Path) -> Res {
    let m = ended_manifest(dir)?;
    let derived = dir.join(DERIVED);
    fs::create_dir_all(&derived)?;
    let tables = kf_telemetry::parquet::export(
        kf_store::session_stream(dir)?,
        &m.session_identity_sha256,
        |t| File::create(derived.join(format!("{t}.parquet"))),
    )?;
    for t in &tables {
        println!("{DERIVED}/{}.parquet: {} rows", t.table, t.rows);
    }
    kf_store::write_built_manifest(dir, m, u64::MAX)?;
    println!("{MANIFEST}");
    Ok(ExitCode::SUCCESS)
}

/// Members: `raw.kflog` (the session stream, so a recovered session ships
/// its recovered bytes), `manifest.json` describing the archive's contents,
/// the derived tables present (flat names), `FORMAT.md`.
fn pack(dir: &Path, out: &Path) -> Res {
    let mut m = ended_manifest(dir)?;
    let tables: Vec<(String, PathBuf)> = DERIVED_TABLES
        .iter()
        .map(|t| format!("{t}.parquet"))
        .map(|n| (n.clone(), dir.join(DERIVED).join(n)))
        .filter(|(_, p)| p.is_file())
        .collect();
    m.files = BTreeMap::from([(kfz::RAW.to_owned(), m.session_identity_sha256.clone())]);
    for (name, p) in &tables {
        let hash = sha256_hex(BufReader::new(File::open(p)?))?;
        m.files.insert(format!("{DERIVED}/{name}"), hash);
    }
    let mut manifest = serde_json::to_vec_pretty(&m)?;
    manifest.push(b'\n');
    let file = File::create_new(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let written = (|| -> Res<()> {
        let mut z = KfzWriter::new(BufWriter::new(file));
        z.add(kfz::RAW, &mut kf_store::session_stream(dir)?)?;
        z.add(MANIFEST, &mut manifest.as_slice())?;
        for (name, p) in &tables {
            z.add(name, &mut BufReader::new(File::open(p)?))?;
        }
        z.add_format_md()?;
        z.finish()?.into_inner()?.sync_all()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(out);
        return Err(e);
    }
    println!("{}", out.display());
    Ok(ExitCode::SUCCESS)
}

/// Members are staged in `<root>/sessions/.unpack-<random>/` and moved into
/// a new `sessions/<session-id>/` only after the whole archive checked out;
/// the staging directory is always removed.
fn unpack(kfz_path: &Path, root: &Path, quota: u64) -> Res {
    let sessions = root.join("sessions");
    fs::create_dir_all(&sessions)?;
    let stage = sessions.join(format!(".unpack-{}", kf_store::new_session_id()));
    fs::create_dir(&stage)?;
    let dir = unpack_staged(kfz_path, root, &stage, quota);
    let _ = fs::remove_dir_all(&stage);
    println!("{}", dir?.display());
    Ok(ExitCode::SUCCESS)
}

fn unpack_staged(kfz_path: &Path, root: &Path, stage: &Path, quota: u64) -> Res<PathBuf> {
    let derived = stage.join(DERIVED);
    fs::create_dir(&derived)?;
    let limits = Limits {
        total: quota,
        member: quota,
    };
    let mut written = 0;
    kfz::unpack(
        BufReader::new(File::open(kfz_path)?),
        limits,
        |name, data| {
            // kfz's allow-list admits only flat names: raw.kflog,
            // manifest.json, FORMAT.md, <table>.parquet.
            let to = if name.ends_with(".parquet") {
                derived.join(name)
            } else {
                stage.join(name)
            };
            let f = File::create_new(to)?;
            let mut w = BufWriter::new(&f);
            written += io::copy(data, &mut w)?;
            w.flush()?;
            drop(w);
            f.sync_all()
        },
    )?;
    // The archive's manifest.json is regenerated; FORMAT.md is not part of
    // a bundle (DATA_SPECIFICATION.md §4).
    let raw = stage.join(kfz::RAW);
    let m = Manifest::from_log(BufReader::new(File::open(&raw)?))?;
    let id = m
        .header
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or("session_header has no session_id")?
        .to_owned();
    // Checks that the id is a UUID; never overwrites.
    let dir = kf_store::create_session_dir(root, &id).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => {
            format!("session {id} already exists in {}", root.display()).into()
        }
        _ => Box::<dyn Error>::from(e),
    })?;
    // The manifest counts against the quota too (members are within it).
    let moved = fs::rename(&raw, dir.join(kfz::RAW))
        .and_then(|()| fs::rename(&derived, dir.join(DERIVED)))
        .and_then(|()| {
            kf_store::write_built_manifest(&dir, m, quota.saturating_sub(written)).map(drop)
        })
        .and_then(|()| kf_store::sync_dir(&dir.join(DERIVED)))
        .and_then(|()| kf_store::sync_dir(&dir));
    if let Err(e) = moved {
        let _ = fs::remove_dir_all(&dir);
        return Err(e.into());
    }
    Ok(dir)
}

/// `<name>.kflog`, `<name>.jsonl` and, for recoverable logs,
/// `<name>.recovered.json`: the committed files in
/// crates/kf-telemetry/tests/golden/.
fn golden(dir: &Path) -> Res {
    fs::create_dir_all(dir)?;
    for (name, bytes) in kf_telemetry::golden::all() {
        fs::write(dir.join(format!("{name}.kflog")), &bytes)?;
        let mut jsonl = Vec::new();
        kf_telemetry::dump::dump(bytes.as_slice(), &mut jsonl)?;
        fs::write(dir.join(format!("{name}.jsonl")), jsonl)?;
        if let Outcome::Recovered(s) = recover(bytes.as_slice()) {
            let mut json = serde_json::to_vec_pretty(&s)?;
            json.push(b'\n');
            fs::write(dir.join(format!("{name}.recovered.json")), json)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
