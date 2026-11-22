//! KineForge foundation (ARCHITECTURE.md §2): unit newtypes, provenance
//! labels, captured input events and sensitivity mathematics. Builds for every
//! target including `wasm32-unknown-unknown`; OS, clock and non-deterministic
//! float APIs are rejected by the workspace `clippy.toml`.
//!
//! The other plain-data record types (frames, drains, simulation records,
//! `SceneView`) arrive with the milestones that produce them (M4, M5, M7).

pub mod input;
pub mod profiles;
pub mod provenance;
pub mod sensitivity;
pub mod units;

pub use input::{Button, Gap, GapReason, InputEvent, Motion, SourceId, Wheel};
pub use profiles::{Claim, GameProfile, PROFILES};
pub use provenance::{
    ClaimStatus, ClockKind, ClockMeaning, EvidenceClass, InputLevel, ReceiptMeaning, Units,
    UnknownName,
};
pub use sensitivity::{Fov, FovModel, LinearSensitivity, Match};
pub use units::{Aspect, Degrees, Nanos, Radians, ValueScale};

#[cfg(kf_canary)]
mod canary;
