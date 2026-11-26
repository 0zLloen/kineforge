//! KineForge simulation (ARCHITECTURE.md §4; ROADMAP M5): fixed-tick clock,
//! camera from integer unit totals, own xoshiro256++ and samplers, analytic
//! (constant-velocity) target positions, ray–sphere/box hit tests, the
//! static-flick scenario, scoring model `kf.flick.v1`, a per-tick state hash
//! and the frame-by-frame replay driver. Pure: no I/O, no clocks, no
//! `HashMap`, transcendental functions only from the pinned `libm`, so
//! every target computes the same bits (EXPERIMENT_PLAN E10).

pub mod camera;
pub mod flick;
pub mod geom;
pub mod hash;
pub mod replay;
pub mod rng;
pub mod score;
pub mod sim;

pub use camera::{Camera, CameraError};
pub use flick::FlickConfig;
pub use score::{Attempt, Outcome};
pub use sim::{
    ConfigError, Control, FrameError, FrameIn, MAX_SIM_HZ, Scenario, Sim, SimConfig, SimRecord,
    Target, TrialState,
};
