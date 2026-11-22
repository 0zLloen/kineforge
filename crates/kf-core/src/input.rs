//! Input events as captured (DATA_SPECIFICATION.md §6.2): one per HID report,
//! OS packet or browser event, never smoothed, resampled or merged. Field
//! layouts mirror the `input_motion`/`input_button`/`input_wheel` records and
//! the gap annotations; the source's declaration (level, units, scale, clocks)
//! travels separately, once per source.

use crate::units::Nanos;

/// Declared source id (a new id per pointer-lock acquisition; absolute
/// packets get their own source). Valid ids are 0..=0xFFFE: 0xFFFF means
/// "none" in annotations and `sim_control` (DATA_SPECIFICATION.md §3.1).
pub type SourceId = u16;

/// Motion and wheel `flags` bits (DATA_SPECIFICATION.md §6.2).
pub mod event_flags {
    /// Coalesced sub-event.
    pub const COALESCED: u16 = 1 << 0;
    /// Background delivery (`RIM_INPUTSINK`).
    pub const BACKGROUND: u16 = 1 << 1;
    /// Absolute packet; never applied as motion.
    pub const ABSOLUTE: u16 = 1 << 2;
    /// Suspected injected (`hDevice == 0`, `LLMHF_INJECTED`, virtual device).
    pub const SUSPECT_INJECTED: u16 = 1 << 3;
    /// Clamped by the capture layer.
    pub const CLAMPED: u16 = 1 << 4;
    /// High-resolution wheel.
    pub const HI_RES: u16 = 1 << 5;
}

/// Button `state` bits (DATA_SPECIFICATION.md §6.2).
pub mod button_state {
    pub const DOWN: u8 = 1 << 0;
    pub const BACKGROUND: u8 = 1 << 1;
    pub const SUSPECT_INJECTED: u8 = 1 << 2;
    pub const CLAMPED: u8 = 1 << 3;
}

/// `button` codes (DATA_SPECIFICATION.md §6.7); 5 and above are device-specific.
pub mod button {
    pub const LEFT: u8 = 0;
    pub const RIGHT: u8 = 1;
    pub const MIDDLE: u8 = 2;
    /// X1.
    pub const BACK: u8 = 3;
    /// X2.
    pub const FORWARD: u8 = 4;
}

/// Relative motion: `dx`/`dy` are the source's units × `value_scale`; +x is
/// rightward hand motion, +y is motion toward the user (device frame).
/// Records flagged [`event_flags::ABSOLUTE`] carry absolute coordinates
/// instead and are never applied as motion.
///
/// `Some` of an on-disk sentinel (`src_ts_ns` = `i64::MIN`, a `Gap`'s
/// `source` = 0xFFFF or `lost` = `u32::MAX`) is invalid: it would read back
/// as `None`, so encoders reject it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Receipt on the session clock (always present).
    pub t_ns: Nanos,
    /// The source's own stamp in its clock domain, unmodified; `None` when
    /// the source has none (Windows Raw Input, hidraw).
    pub src_ts_ns: Option<Nanos>,
    pub dx: i32,
    pub dy: i32,
    /// One counter per source, shared by motion/button/wheel; motion precedes
    /// the button record from the same report.
    pub seq: u32,
    pub source: SourceId,
    /// [`event_flags`] bits.
    pub flags: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub t_ns: Nanos,
    pub src_ts_ns: Option<Nanos>,
    pub seq: u32,
    pub source: SourceId,
    /// A [`button`] code.
    pub button: u8,
    /// [`button_state`] bits.
    pub state: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wheel {
    pub t_ns: Nanos,
    pub src_ts_ns: Option<Nanos>,
    /// Vertical and horizontal raw wheel values × `value_scale`; the wheel
    /// unit (detents, 1/120 detents with [`event_flags::HI_RES`], pixels…) is
    /// not yet specified (DATA_SPECIFICATION.md §6.2).
    pub dv: i32,
    pub dh: i32,
    pub seq: u32,
    pub source: SourceId,
    /// [`event_flags`] bits.
    pub flags: u16,
}

/// Known loss of events (a gap annotation, DATA_SPECIFICATION.md §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub t_ns: Nanos,
    /// `None` when the loss is not tied to one source.
    pub source: Option<SourceId>,
    pub first_seq: u32,
    /// Events lost; `None` when unknown (e.g. evdev `SYN_DROPPED`).
    pub lost: Option<u32>,
    pub reason: GapReason,
}

/// Gap annotation codes (DATA_SPECIFICATION.md §6.7: 0x0001–0x00FF, of which
/// 0x0001–0x000B are named). Unnamed gap codes are kept in
/// [`GapReason::Other`], never dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GapReason {
    KernelDropped,
    QueueOverflow,
    SimQueueOverflow,
    RecordQueueOverflow,
    PointerLockLost,
    FocusLost,
    VisibilityHidden,
    BackgroundThrottled,
    DeviceRemoved,
    ClockChanged,
    StorageError,
    /// An unnamed gap code (0x000C–0x00FF); [`GapReason::from_code`] never
    /// puts a named code in it.
    Other(u16),
}

impl GapReason {
    const NAMED: [GapReason; 11] = [
        GapReason::KernelDropped,
        GapReason::QueueOverflow,
        GapReason::SimQueueOverflow,
        GapReason::RecordQueueOverflow,
        GapReason::PointerLockLost,
        GapReason::FocusLost,
        GapReason::VisibilityHidden,
        GapReason::BackgroundThrottled,
        GapReason::DeviceRemoved,
        GapReason::ClockChanged,
        GapReason::StorageError,
    ];

    /// The on-disk annotation code.
    pub const fn code(self) -> u16 {
        match self {
            GapReason::KernelDropped => 0x0001,
            GapReason::QueueOverflow => 0x0002,
            GapReason::SimQueueOverflow => 0x0003,
            GapReason::RecordQueueOverflow => 0x0004,
            GapReason::PointerLockLost => 0x0005,
            GapReason::FocusLost => 0x0006,
            GapReason::VisibilityHidden => 0x0007,
            GapReason::BackgroundThrottled => 0x0008,
            GapReason::DeviceRemoved => 0x0009,
            GapReason::ClockChanged => 0x000A,
            GapReason::StorageError => 0x000B,
            GapReason::Other(c) => c,
        }
    }

    /// `None` outside the gap range 0x0001–0x00FF (state, experiment and
    /// marker annotations are not gaps); otherwise
    /// `from_code(c).unwrap().code() == c`.
    pub const fn from_code(code: u16) -> Option<GapReason> {
        match code {
            0x0001..=0x000B => Some(GapReason::NAMED[(code - 1) as usize]),
            0x000C..=0x00FF => Some(GapReason::Other(code)),
            _ => None,
        }
    }
}

/// One captured input record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Motion(Motion),
    Button(Button),
    Wheel(Wheel),
    Gap(Gap),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_codes_round_trip() {
        for c in 0x0001..=0x00FF {
            assert_eq!(GapReason::from_code(c).map(GapReason::code), Some(c));
        }
        for (i, r) in GapReason::NAMED.iter().enumerate() {
            assert_eq!(usize::from(r.code()), i + 1);
        }
        assert_eq!(GapReason::from_code(0x000B), Some(GapReason::StorageError));
        assert_eq!(GapReason::from_code(0x000C), Some(GapReason::Other(0x000C)));
        // Not gaps: none, state, experiment, user marker.
        for c in [0, 0x0100, 0x0102, 0x0200, 0x0F00, u16::MAX] {
            assert_eq!(GapReason::from_code(c), None);
        }
    }

    /// Frozen wire values copied from DATA_SPECIFICATION.md §6.2 / §6.7.
    #[test]
    fn wire_bits_match_spec() {
        use event_flags::*;
        assert_eq!(
            [
                COALESCED,
                BACKGROUND,
                ABSOLUTE,
                SUSPECT_INJECTED,
                CLAMPED,
                HI_RES
            ],
            [1, 2, 4, 8, 16, 32]
        );
        use button_state as bs;
        assert_eq!(
            [bs::DOWN, bs::BACKGROUND, bs::SUSPECT_INJECTED, bs::CLAMPED],
            [1, 2, 4, 8]
        );
        use button::*;
        assert_eq!([LEFT, RIGHT, MIDDLE, BACK, FORWARD], [0, 1, 2, 3, 4]);
    }
}
