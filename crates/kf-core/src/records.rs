//! Plain-data records of the raw log besides input events
//! (DATA_SPECIFICATION.md §6.1, §6.3, §6.4). Field layouts mirror the
//! on-disk records; on-disk sentinels are `Option`s here, and `Some` of a
//! sentinel value is invalid (encoders reject it). Enumerated fields stay raw
//! integers with named constants, so unknown values are preserved.

use crate::input::{Gap, GapReason, SourceId};
use crate::units::Nanos;

/// `frame` (0x10): written when the frame's simulation step completes;
/// authoritative for replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub frame: u64,
    pub t_start_ns: Nanos,
    pub t_input_cutoff_ns: Nanos,
    pub first_tick: u64,
    pub ticks: u32,
    /// [`frame_flags`] bits.
    pub flags: u16,
}

/// `frame.flags` bits (DATA_SPECIFICATION.md §6.7).
pub mod frame_flags {
    pub const PAUSED: u16 = 1 << 0;
    pub const CATCH_UP_CAPPED: u16 = 1 << 1;
    pub const RENDER_SKIPPED: u16 = 1 << 2;
}

/// `drain` (0x11): written only for frames that consumed ≥ 1 driving-source event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drain {
    pub frame: u64,
    pub source: SourceId,
    pub last_seq: u32,
}

/// `frame_timing` (0x12): written when timings resolve, possibly frames later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTiming {
    pub frame: u64,
    pub t_submit_ns: Nanos,
    /// Only where the platform reports real presentation time; never estimated.
    pub t_present_ns: Option<Nanos>,
    pub cpu_ns: u32,
    pub gpu_ns: Option<u32>,
}

/// `sim_camera` (0x20): on change, at most once per tick. `yaw_rad`/`pitch_rad`
/// must equal the recomputation from the unit totals bit for bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimCamera {
    pub tick: u64,
    pub yaw_units: i64,
    pub pitch_units: i64,
    pub yaw_rad: f64,
    pub pitch_rad: f64,
}

/// `sim_target` (0x21).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimTarget {
    pub tick: u64,
    pub target: u32,
    /// A [`target_event`] code.
    pub event: u8,
    /// A [`target_shape`] code.
    pub shape: u8,
    pub pos_m: [f64; 3],
    pub vel_m_s: [f64; 3],
    pub radius_m: f64,
}

/// `sim_target.event` codes (DATA_SPECIFICATION.md §6.7).
pub mod target_event {
    pub const SPAWN: u8 = 0;
    pub const UPDATE: u8 = 1;
    pub const DESPAWN: u8 = 2;
    pub const HIT: u8 = 3;
    pub const KILL: u8 = 4;
}

/// `sim_target.shape` codes (DATA_SPECIFICATION.md §6.7).
pub mod target_shape {
    pub const SPHERE: u8 = 0;
    pub const CAPSULE: u8 = 1;
    pub const BOX: u8 = 2;
}

/// `sim_shot` (0x22).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimShot {
    pub tick: u64,
    pub t_ns: Nanos,
    /// `None` when the shot hit no target.
    pub target: Option<u32>,
    /// `seq` of the button record that fired it.
    pub seq: u32,
    pub ang_err_rad: f64,
    pub d_yaw_rad: f64,
    pub d_pitch_rad: f64,
    pub hit: bool,
}

/// `sim_control` (0x23): every simulation state change that does not come
/// from driving-source records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimControl {
    pub tick: u64,
    pub t_ns: Nanos,
    pub frame: u64,
    /// A [`control_action`] code.
    pub action: u8,
    /// [`control_phase`]: before or after the frame's drain.
    pub phase: u8,
    /// A [`control_reason`] code.
    pub reason: u16,
    /// For `switch_source`; `None` otherwise.
    pub new_source: Option<SourceId>,
}

/// `sim_control.action` codes (DATA_SPECIFICATION.md §6.7).
pub mod control_action {
    pub const START_TRIAL: u8 = 0;
    pub const RESTART_TRIAL: u8 = 1;
    pub const PAUSE: u8 = 2;
    pub const RESUME: u8 = 3;
    pub const ABORT_TRIAL: u8 = 4;
    pub const END: u8 = 5;
    pub const NEXT_BLOCK: u8 = 6;
    pub const SWITCH_SOURCE: u8 = 7;
}

/// `sim_control.phase` values.
pub mod control_phase {
    pub const BEFORE_DRAIN: u8 = 0;
    pub const AFTER_DRAIN: u8 = 1;
}

/// `sim_control.reason` codes (DATA_SPECIFICATION.md §6.7).
pub mod control_reason {
    pub const USER: u16 = 0;
    pub const POINTER_LOCK_LOST: u16 = 1;
    pub const FOCUS_LOST: u16 = 2;
    pub const VISIBILITY_HIDDEN: u16 = 3;
    pub const CONTEXT_LOST: u16 = 4;
    pub const SIM_QUEUE_OVERFLOW: u16 = 5;
    pub const STORAGE_ERROR: u16 = 6;
    pub const INPUT_LEVEL_CHANGED: u16 = 7;
    pub const SCENARIO_COMPLETE: u16 = 8;
    pub const EXPERIMENT_SCHEDULE: u16 = 9;
}

/// `clock_sync` (0x7D): one observation relating a source clock domain to
/// the session clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSync {
    pub t_ns: Nanos,
    pub domain: u16,
    pub src_ts_ns: Nanos,
    pub uncertainty_ns: i64,
}

/// `annotation` (0x7E): gaps, state changes, experiment markers, user notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub t_ns: Nanos,
    /// An [`annotation_code`] (gap codes are [`GapReason`]).
    pub code: u16,
    pub source: Option<SourceId>,
    pub first_seq: u32,
    /// `None` when unknown.
    pub lost: Option<u32>,
    pub text: String,
}

/// Non-gap annotation codes (DATA_SPECIFICATION.md §6.7); gap codes
/// 0x0001–0x00FF are [`GapReason`].
pub mod annotation_code {
    pub const FOCUS_GAINED: u16 = 0x0100;
    pub const VISIBILITY_VISIBLE: u16 = 0x0101;
    pub const POINTER_LOCK_ACQUIRED: u16 = 0x0102;
    pub const DEVICE_ADDED: u16 = 0x0103;
    pub const CONTEXT_LOST: u16 = 0x0104;
    pub const CONTEXT_RESTORED: u16 = 0x0105;
    pub const FRAME_DROP: u16 = 0x0106;
    pub const BLOCK_START: u16 = 0x0200;
    pub const BLOCK_END: u16 = 0x0201;
    pub const CONDITION: u16 = 0x0202;
    pub const WARMUP_START: u16 = 0x0203;
    pub const WARMUP_END: u16 = 0x0204;
    pub const USER_MARKER: u16 = 0x0F00;
}

impl Annotation {
    /// The gap this annotation records, if its code is a gap code.
    pub fn gap(&self) -> Option<Gap> {
        Some(Gap {
            t_ns: self.t_ns,
            source: self.source,
            first_seq: self.first_seq,
            lost: self.lost,
            reason: GapReason::from_code(self.code)?,
        })
    }
}

impl From<Gap> for Annotation {
    fn from(g: Gap) -> Annotation {
        Annotation {
            t_ns: g.t_ns,
            code: g.reason.code(),
            source: g.source,
            first_seq: g.first_seq,
            lost: g.lost,
            text: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frozen wire values copied from DATA_SPECIFICATION.md §6.7.
    #[test]
    fn codes_match_spec() {
        use frame_flags::*;
        assert_eq!([PAUSED, CATCH_UP_CAPPED, RENDER_SKIPPED], [1, 2, 4]);
        use target_event::*;
        assert_eq!([SPAWN, UPDATE, DESPAWN, HIT, KILL], [0, 1, 2, 3, 4]);
        use target_shape::*;
        assert_eq!([SPHERE, CAPSULE, BOX], [0, 1, 2]);
        use control_action::*;
        assert_eq!(
            [
                START_TRIAL,
                RESTART_TRIAL,
                PAUSE,
                RESUME,
                ABORT_TRIAL,
                END,
                NEXT_BLOCK,
                SWITCH_SOURCE
            ],
            [0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(
            [control_phase::BEFORE_DRAIN, control_phase::AFTER_DRAIN],
            [0, 1]
        );
        {
            use control_reason::*;
            assert_eq!(
                [
                    USER,
                    POINTER_LOCK_LOST,
                    FOCUS_LOST,
                    VISIBILITY_HIDDEN,
                    CONTEXT_LOST,
                    SIM_QUEUE_OVERFLOW,
                    STORAGE_ERROR,
                    INPUT_LEVEL_CHANGED,
                    SCENARIO_COMPLETE,
                    EXPERIMENT_SCHEDULE
                ],
                [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
            );
        }
        use annotation_code::*;
        assert_eq!(
            [
                FOCUS_GAINED,
                VISIBILITY_VISIBLE,
                POINTER_LOCK_ACQUIRED,
                DEVICE_ADDED,
                CONTEXT_LOST,
                CONTEXT_RESTORED,
                FRAME_DROP,
                BLOCK_START,
                BLOCK_END,
                CONDITION,
                WARMUP_START,
                WARMUP_END,
                USER_MARKER
            ],
            [
                0x0100, 0x0101, 0x0102, 0x0103, 0x0104, 0x0105, 0x0106, 0x0200, 0x0201, 0x0202,
                0x0203, 0x0204, 0x0F00
            ]
        );
    }

    #[test]
    fn gap_annotation_round_trip() {
        let g = Gap {
            t_ns: Nanos(5),
            source: Some(3),
            first_seq: 10,
            lost: None,
            reason: GapReason::KernelDropped,
        };
        let a = Annotation::from(g);
        assert_eq!(a.code, 1);
        assert_eq!(a.gap(), Some(g));
        let marker = Annotation {
            code: annotation_code::USER_MARKER,
            ..a
        };
        assert_eq!(marker.gap(), None);
    }
}
