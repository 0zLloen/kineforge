//! KFLOG v1, the raw session log (DATA_SPECIFICATION.md §5–§6, §10), and
//! the session JSON records. Pure Rust with no I/O and no clocks: it reads
//! from `std::io::Read` and returns bytes to append, so the native writer,
//! the browser OPFS worker and the tools share one implementation.
//!
//! - [`format`](mod@format): framing, kinds, CRC.
//! - [`record`]: the payload codec for every kind.
//! - [`json`]: typed views of the JSON kinds.
//! - [`recorder`]: the group encoder the writer side drives.
//! - [`reader`]: streaming chunk and committed-group reader.
//! - [`validate`]: the validator.
//! - [`recover`]: crash recovery without touching the raw log.
//! - [`dump`]: canonical JSON lines (parity with `tools/read_kflog.py`).
//! - [`golden`]: deterministic example logs for tests and parity checks.

pub mod dump;
pub mod format;
pub mod golden;
pub mod json;
pub mod kfz;
pub mod manifest;
#[cfg(feature = "parquet")]
pub mod parquet;
pub mod reader;
pub mod record;
pub mod recorder;
pub mod recover;
pub mod validate;

pub use format::Kind;
pub use reader::{Chunk, ChunkReader, Ending, Group, GroupReader, ReadError};
pub use record::{Commit, Record};
pub use recorder::{FlushPolicy, Recorder};
