//! The native raw-log writer (DATA_SPECIFICATION.md §5.2, ARCHITECTURE.md §6
//! "Native"). The recorder thread [`SessionWriter::submit`]s each group
//! returned by `Recorder::cut`; a writer thread appends them in order (one
//! `write` per group) and a separate sync thread calls `sync_data` every
//! `fsync_ms` and at close, so writes never wait for a sync.
//!
//! - **Liveness lock:** an exclusive lock on `raw.kflog` (`File::try_lock`:
//!   `flock`/`LockFileEx`) held for the whole session.
//! - **Space reserve:** a synced `raw.kflog.reserve` of random bytes (zeros
//!   take no space on compressing filesystems) next to the log, deleted by
//!   [`SessionWriter::close`] right before it writes the final bytes, so the
//!   final commit and `session_end` fit after `ENOSPC`. (Portable stand-in
//!   for `fallocate(FALLOC_FL_KEEP_SIZE)`/`FileAllocationInfo`.)
//! - **Write errors** (`ENOSPC` or any I/O error): the torn group is cut off
//!   with `set_len` (it was never committed), [`Status::storage_error`] is
//!   set and later groups are dropped (counted) until
//!   [`SessionWriter::close`].
//! - **Memory cap:** the queue is bounded only by `memory_cap_bytes`; a group
//!   that would exceed it is dropped and counted.
//! - **Recorder thread:** whenever it reads [`SessionWriter::status`], it
//!   passes `written_bytes` to `Recorder::written`, which forgets older group
//!   boundaries (memory stays bounded).
//! - **After any drop** the file ends at an earlier group boundary than the
//!   recorder believes. The recorder thread then stops submitting, calls
//!   [`SessionWriter::drain`], hands `written_bytes` to `Recorder::writer_lost`
//!   (which rolls back and records `storage_error` gaps) and ends the session
//!   with `end_reason` `storage_error`. The writer itself never parses or
//!   rewrites groups: it is a byte sink.
//!
//! Dropping a `SessionWriter` without `close` behaves like a crash once the
//! queue drains: no `session_end`, recoverable.

use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The raw log of a session bundle.
pub const RAW: &str = "raw.kflog";
/// The space reserve next to it while the session is live.
pub const RESERVE: &str = "raw.kflog.reserve";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriterOptions {
    /// Interval between `sync_data` calls (default 1000; 0 is taken as 1).
    /// Each sync starts early by the previous one's duration, so with steady
    /// syncs shorter than half the interval a write is durable about
    /// `fsync_ms` after it. In general it is durable within
    /// `max(fsync_ms, longest sync) + longest sync` (longest sync =
    /// [`Status::max_sync_ns`]; a sync that outlasts the interval makes the
    /// next one start at once).
    pub fsync_ms: u64,
    /// Queue limit in bytes (default 64 MiB).
    pub memory_cap_bytes: u64,
    /// Size of the space reserve (default 64 KiB).
    pub reserve_bytes: u64,
    /// Fault injection for tests: the first write that would take
    /// `raw.kflog` past this many bytes writes only up to it and fails with
    /// `StorageFull`, as `ENOSPC` would; later writes succeed (as into the
    /// reserve that `close` frees). `None` (default) in production.
    pub fail_after_bytes: Option<u64>,
}

impl Default for WriterOptions {
    fn default() -> WriterOptions {
        WriterOptions {
            fsync_ms: 1000,
            memory_cap_bytes: 64 << 20,
            reserve_bytes: 64 << 10,
            fail_after_bytes: None,
        }
    }
}

/// Writer counters; see [`SessionWriter::status`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Bytes appended to `raw.kflog` (whole groups only).
    pub written_bytes: u64,
    /// File length covered by the last completed `sync_data`.
    pub durable_bytes: u64,
    /// Queue (submitted, not yet written or dropped), in bytes.
    pub backlog_bytes: u64,
    /// Largest queue seen, in bytes.
    pub max_backlog_bytes: u64,
    /// Longest `sync_data` so far, in ns; bounds how late a write becomes
    /// durable (see [`WriterOptions::fsync_ms`]).
    pub max_sync_ns: u64,
    pub dropped_groups: u64,
    pub dropped_bytes: u64,
    /// A write or sync failed: end the session with `end_reason`
    /// `storage_error` (after a `storage_error` gap annotation).
    pub storage_error: bool,
}

#[derive(Default)]
struct Shared {
    /// Submitted, not yet written or dropped.
    backlog: AtomicU64,
    max_backlog: AtomicU64,
    max_sync_ns: AtomicU64,
    written: AtomicU64,
    durable: AtomicU64,
    storage_error: AtomicBool,
    dropped_groups: AtomicU64,
    dropped_bytes: AtomicU64,
}

impl Shared {
    fn drop_group(&self, group: &[u8]) {
        self.dropped_groups.fetch_add(1, AcqRel);
        self.dropped_bytes.fetch_add(group.len() as u64, AcqRel);
    }

    fn status(&self) -> Status {
        // Durable first: it never exceeds written, so the snapshot keeps
        // durable_bytes <= written_bytes.
        let durable_bytes = self.durable.load(Acquire);
        Status {
            written_bytes: self.written.load(Acquire),
            durable_bytes,
            backlog_bytes: self.backlog.load(Acquire),
            max_backlog_bytes: self.max_backlog.load(Acquire),
            max_sync_ns: self.max_sync_ns.load(Acquire),
            dropped_groups: self.dropped_groups.load(Acquire),
            dropped_bytes: self.dropped_bytes.load(Acquire),
            storage_error: self.storage_error.load(Acquire),
        }
    }
}

/// Makes directory entries durable (Unix; elsewhere a no-op).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// The appending end of `raw.kflog`, owned by the writer thread, then by close.
struct Sink {
    file: File,
    dir: PathBuf,
    reserve: bool,
    fail_after: Option<u64>,
    /// Truncating a torn group failed: the file position is unknown.
    broken: bool,
    shared: Arc<Shared>,
}

impl Sink {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::other("raw.kflog is in an unknown state"));
        }
        let at = self.shared.written.load(Acquire);
        let end = at + bytes.len() as u64;
        let result = match self.fail_after {
            Some(limit) if end > limit => {
                self.fail_after = None;
                let keep = (limit.saturating_sub(at)) as usize;
                self.file.write_all(&bytes[..keep]).and(Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected fault (fail_after_bytes)",
                )))
            }
            _ => self.file.write_all(bytes),
        };
        if result.is_ok() {
            self.shared.written.store(end, Release);
            return result;
        }
        // The torn group was never committed: cut it off and rewind.
        self.broken = self
            .file
            .set_len(at)
            .and_then(|()| self.file.seek(SeekFrom::Start(at)))
            .is_err();
        self.shared.storage_error.store(true, Release);
        result
    }

    fn release_reserve(&mut self) {
        if std::mem::take(&mut self.reserve) {
            let _ = fs::remove_file(self.dir.join(RESERVE));
            let _ = sync_dir(&self.dir);
        }
    }
}

fn write_loop(mut sink: Sink, rx: Receiver<Vec<u8>>) -> Sink {
    for group in rx {
        let n = group.len() as u64;
        if sink.shared.storage_error.load(Acquire) || sink.append(&group).is_err() {
            sink.shared.drop_group(&group);
        }
        sink.shared.backlog.fetch_sub(n, AcqRel);
    }
    sink
}

/// Syncs every `every` (on a fixed schedule) and once more when `stop`
/// disconnects; returns the last sync's result.
fn sync_loop(file: File, stop: Receiver<()>, shared: &Shared, every: Duration) -> io::Result<()> {
    let mut next = Instant::now() + every;
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        let last = !matches!(stop.recv_timeout(wait), Err(RecvTimeoutError::Timeout));
        // Everything written before the sync starts is covered by it.
        let written = shared.written.load(Acquire);
        let started = Instant::now();
        let r = file.sync_data();
        let took = started.elapsed();
        shared
            .max_sync_ns
            .fetch_max(u64::try_from(took.as_nanos()).unwrap_or(u64::MAX), AcqRel);
        if r.is_ok() {
            shared.durable.fetch_max(written, AcqRel);
        } else {
            shared.storage_error.store(true, Release);
        }
        if last {
            return r;
        }
        // Start the next sync early by this one's duration, so bytes written
        // just after this sync started are durable about `every` later
        // (PERFORMANCE_PLAN N9c), not `every` plus a sync: they lag by
        // `every` plus the next sync's duration minus this one's. A sync
        // longer than `every` makes the next start at once (lag: the two
        // syncs), so the bound is max(every, longest) + longest.
        next = Instant::now().max(next + every.saturating_sub(took));
    }
}

/// A live session's raw log; see the module docs.
pub struct SessionWriter {
    shared: Arc<Shared>,
    cap: u64,
    tx: Sender<Vec<u8>>,
    writer: JoinHandle<Sink>,
    stop_sync: Sender<()>,
    syncer: JoinHandle<io::Result<()>>,
    /// Holds the liveness lock.
    lock: File,
}

impl SessionWriter {
    /// Creates `<dir>/raw.kflog` (it must not exist), locks it, creates the
    /// space reserve and starts the writer and sync threads.
    pub fn create(dir: &Path, opts: WriterOptions) -> io::Result<SessionWriter> {
        let path = dir.join(RAW);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        SessionWriter::start(file, dir, opts).inspect_err(|_| {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(dir.join(RESERVE));
        })
    }

    fn start(file: File, dir: &Path, opts: WriterOptions) -> io::Result<SessionWriter> {
        file.try_lock()?;
        let mut reserve = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(RESERVE))?;
        let s = RandomState::new();
        let random: Vec<u8> = (0..opts.reserve_bytes.div_ceil(8))
            .flat_map(|i| s.hash_one(i).to_le_bytes())
            .take(opts.reserve_bytes as usize)
            .collect();
        reserve.write_all(&random)?;
        reserve.sync_all()?;
        sync_dir(dir)?;
        let shared = Arc::new(Shared::default());
        let sink = Sink {
            file: file.try_clone()?,
            dir: dir.to_owned(),
            reserve: true,
            fail_after: opts.fail_after_bytes,
            broken: false,
            shared: shared.clone(),
        };
        let sync_file = file.try_clone()?;
        let (tx, rx) = mpsc::channel();
        let writer = thread::Builder::new()
            .name("kf-store-writer".into())
            .spawn(move || write_loop(sink, rx))?;
        let (stop_sync, stop) = mpsc::channel();
        let every = Duration::from_millis(opts.fsync_ms.max(1));
        let syncer = thread::Builder::new().name("kf-store-sync".into()).spawn({
            let shared = shared.clone();
            move || sync_loop(sync_file, stop, &shared, every)
        })?;
        Ok(SessionWriter {
            shared,
            cap: opts.memory_cap_bytes,
            tx,
            writer,
            stop_sync,
            syncer,
            lock: file,
        })
    }

    /// Queues one group (`Recorder::cut` output) without blocking. Returns
    /// `false` if it was dropped (memory cap, or after a storage error);
    /// empty groups are ignored.
    pub fn submit(&self, group: Vec<u8>) -> bool {
        if group.is_empty() {
            return true;
        }
        let sh = &self.shared;
        let n = group.len() as u64;
        if sh.storage_error.load(Acquire) {
            sh.drop_group(&group);
            return false;
        }
        let queued = sh.backlog.fetch_add(n, AcqRel) + n;
        if queued <= self.cap {
            match self.tx.send(group) {
                Ok(()) => {
                    sh.max_backlog.fetch_max(queued, AcqRel);
                    return true;
                }
                // The writer thread is gone (it panicked).
                Err(mpsc::SendError(g)) => {
                    sh.backlog.fetch_sub(n, AcqRel);
                    sh.drop_group(&g);
                    return false;
                }
            }
        }
        sh.backlog.fetch_sub(n, AcqRel);
        sh.drop_group(&group);
        false
    }

    pub fn status(&self) -> Status {
        self.shared.status()
    }

    /// Waits until every submitted group is written or dropped; then
    /// `written_bytes` is the file's length, a group boundary.
    pub fn drain(&self) -> Status {
        // ponytail: 1 ms polling; a condvar if drain ever sits on a hot path.
        while self.shared.backlog.load(Acquire) > 0 && !self.writer.is_finished() {
            thread::sleep(Duration::from_millis(1));
        }
        self.shared.status()
    }

    /// Ends the session: waits for the queue, deletes the reserve, writes
    /// `final_bytes` (the output of `Recorder::finish`: final group, commit,
    /// `session_end`; it is written even after a storage error, into the
    /// freed reserve), syncs, stops the threads and releases the lock.
    /// Cut and submit the last regular group before `finish`, so
    /// `final_bytes` stays small enough for the reserve.
    pub fn close(self, final_bytes: Vec<u8>) -> io::Result<Status> {
        let SessionWriter {
            shared,
            tx,
            writer,
            stop_sync,
            syncer,
            lock,
            ..
        } = self;
        drop(tx);
        let mut sink = writer
            .join()
            .map_err(|_| io::Error::other("writer thread panicked"))?;
        sink.release_reserve();
        let wrote = sink.append(&final_bytes);
        drop(stop_sync);
        let synced = syncer
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("sync thread panicked")));
        let unlocked = lock.unlock();
        wrote?;
        synced?;
        unlocked?;
        Ok(shared.status())
    }
}
