//! KineForge foundation (ARCHITECTURE.md §2): unit newtypes, provenance
//! labels, the plain-data records of the raw log and sensitivity
//! mathematics. Builds for every target including `wasm32-unknown-unknown`;
//! OS, clock and non-deterministic float APIs are rejected by the workspace
//! `clippy.toml`. Feature `serde`: the provenance enums (de)serialize as
//! their on-disk names.
//!
//! `SceneView` arrives with the renderer (M7).

pub mod input;
pub mod profiles;
pub mod provenance;
pub mod records;
pub mod sensitivity;
pub mod units;

pub use input::{Button, Gap, GapReason, InputEvent, Motion, SourceId, Wheel};
pub use profiles::{Claim, GameProfile, PROFILES};
pub use provenance::{
    ClaimStatus, ClockKind, ClockMeaning, EvidenceClass, InputLevel, ReceiptMeaning, Units,
    UnknownName,
};
pub use records::{
    Annotation, ClockSync, Drain, Frame, FrameTiming, SimCamera, SimControl, SimShot, SimTarget,
};
pub use sensitivity::{Fov, FovModel, LinearSensitivity, Match};
pub use units::{Aspect, Degrees, Nanos, Radians, ValueScale};

#[cfg(kf_canary)]
mod canary;
