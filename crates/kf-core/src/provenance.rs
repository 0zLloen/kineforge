//! Provenance labels (DATA_SPECIFICATION.md §2.1, §3; EXPERIMENT_PLAN.md
//! §0.2). Each enum's `as_str` name is its on-disk JSON value; names are
//! frozen once 1.0 ships (DATA_SPECIFICATION.md §11).
//!
//! Two label sets that must not be confused: [`EvidenceClass`] labels data,
//! [`ClaimStatus`] labels claims and constants.

use core::fmt;
use core::str::FromStr;

/// Error for a string that names no variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownName;

impl fmt::Display for UnknownName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown enum name")
    }
}

impl std::error::Error for UnknownName {}

macro_rules! named_enum {
    ($(#[$m:meta])* pub enum $name:ident { $($(#[$vm:meta])* $var:ident = $s:literal,)+ }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$vm])* $var,)+ }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            /// The on-disk (JSON) name.
            pub const fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
        }

        impl FromStr for $name {
            type Err = UnknownName;
            fn from_str(s: &str) -> Result<$name, UnknownName> {
                match s { $($s => Ok($name::$var),)+ _ => Err(UnknownName) }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        /// Serialized as its [`as_str`](Self::as_str) name.
        #[cfg(feature = "serde")]
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        #[cfg(feature = "serde")]
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<$name, D::Error> {
                const NAMES: &[&str] = &[$($s),+];
                let s = String::deserialize(d)?;
                s.parse().map_err(|_| serde::de::Error::unknown_variant(&s, NAMES))
            }
        }
    };
}

named_enum! {
    /// Pipeline layer a source's samples come from (DATA_SPECIFICATION.md
    /// §3.1). Labelled with the lowest applicable layer; never mixed within a
    /// source.
    pub enum InputLevel {
        /// L1: integer X/Y of a HID input report (firmware processing included).
        DeviceReport = "device_report",
        /// L2: OS relative events carrying the HID values, one per report, no ballistics.
        OsRawCounts = "os_raw_counts",
        /// L2: pre-ballistics OS deltas that may be rotated, scaled, aggregated or plugin-modified.
        OsUnaccelerated = "os_unaccelerated",
        /// L2†: absolute coordinates through a raw API; never deltas.
        OsAbsolutePointer = "os_absolute_pointer",
        /// L3: post-ballistics pointer motion in display space.
        OsPointer = "os_pointer",
        /// L4u: Pointer Lock whose `unadjustedMovement: true` request resolved.
        BrowserLockedUnadjusted = "browser_locked_unadjusted",
        /// L4a: Pointer Lock without unadjusted movement (OS acceleration applies).
        BrowserLocked = "browser_locked",
        /// L4p: cursor positions; UI only, never aiming analysis.
        BrowserUnlocked = "browser_unlocked",
        /// Generated inside KineForge (test patterns, bots); seed recorded.
        Synthetic = "synthetic",
        /// Re-emitted from a recording; references the original log hash.
        Replayed = "replayed",
    }
}

named_enum! {
    /// Unit of a source's motion values before `value_scale`
    /// (DATA_SPECIFICATION.md §3.1).
    pub enum Units {
        Counts = "counts",
        OsUnits = "os_units",
        LogicalPx = "logical_px",
        Dip = "dip",
        CssPx = "css_px",
        Points = "points",
        Abs65535 = "abs_65535",
    }
}

named_enum! {
    /// What kind of observation a datum is (DATA_SPECIFICATION.md §3.2).
    pub enum EvidenceClass {
        /// Directly captured from a sensor/OS/browser API, or imported from a capture tool's file.
        Observed = "observed",
        /// Displayed or reported by an application, transcribed or captured.
        AppReported = "app_reported",
        /// Produced by a model from observations; references the model or experiment.
        Inferred = "inferred",
        /// Ground truth from KineForge's own simulation.
        SimulatedTruth = "simulated_truth",
        /// Statistics computed from other data; references inputs by hash and algorithm version.
        Derived = "derived",
    }
}

named_enum! {
    /// Status of a claim or constant (EXPERIMENT_PLAN.md §0.2, the single
    /// definition).
    pub enum ClaimStatus {
        /// Primary documentation or source code, the local system observed directly, or our own code under test.
        Verified = "verified",
        /// Measured by an EXPERIMENT_PLAN procedure; reported with n, spread and rig.
        ExperimentallyObserved = "experimentally_observed",
        /// Derived from verified facts or observations by stated reasoning or a named model.
        Inferred = "inferred",
        /// Widely used community value with no primary source found.
        CommunityEstablished = "community_established",
        /// Not established.
        Unknown = "unknown",
        /// Single observation, unreplicated, rig not yet characterised, or sources conflicting / weakly sourced.
        RequiresFurtherValidation = "requires_further_validation",
    }
}

named_enum! {
    /// Clock-domain `kind` (DATA_SPECIFICATION.md §2.1).
    pub enum ClockKind {
        Session = "session",
        LinuxMonotonic = "linux_monotonic",
        LinuxRealtime = "linux_realtime",
        LinuxBoottime = "linux_boottime",
        XServerMs = "x_server_ms",
        WaylandUtime = "wayland_utime",
        GameinputUs = "gameinput_us",
        MachAbsolute = "mach_absolute",
        BrowserPerf = "browser_perf",
    }
}

named_enum! {
    /// Clock-domain `meaning`: which moment a stamp marks
    /// (DATA_SPECIFICATION.md §2.1).
    pub enum ClockMeaning {
        /// Domain 0 only; offsets 0.
        SessionClock = "session_clock",
        /// evdev / libinput.
        KernelFlush = "kernel_flush",
        /// IOHID AbsoluteTime.
        KernelEvent = "kernel_event",
        /// X server; Chrome-on-X11 event times.
        OsDispatch = "os_dispatch",
        /// Wayland.
        Compositor = "compositor",
        /// Chrome on Windows / Wayland.
        BrowserReceipt = "browser_receipt",
    }
}

named_enum! {
    /// Per source: what the receipt time `t_ns` marks (DATA_SPECIFICATION.md §2.1).
    pub enum ReceiptMeaning {
        /// Capture-thread stamp at drain/read.
        AppDequeue = "app_dequeue",
        /// Web handler entry.
        DomCallback = "dom_callback",
        /// Timestamps from an imported file.
        Imported = "imported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<T: Copy>(all: &[T], f: fn(T) -> &'static str) -> Vec<&'static str> {
        all.iter().map(|&v| f(v)).collect()
    }

    /// On-disk names are a frozen contract: these lists are copied from
    /// DATA_SPECIFICATION.md §2.1/§3 and EXPERIMENT_PLAN.md §0.2 (whose
    /// upper-case labels are display forms of these values).
    #[test]
    fn names_match_spec_and_round_trip() {
        let cases: [(Vec<&str>, &str); 7] = [
            (
                names(InputLevel::ALL, InputLevel::as_str),
                "device_report os_raw_counts os_unaccelerated os_absolute_pointer os_pointer \
                 browser_locked_unadjusted browser_locked browser_unlocked synthetic replayed",
            ),
            (
                names(Units::ALL, Units::as_str),
                "counts os_units logical_px dip css_px points abs_65535",
            ),
            (
                names(EvidenceClass::ALL, EvidenceClass::as_str),
                "observed app_reported inferred simulated_truth derived",
            ),
            (
                names(ClaimStatus::ALL, ClaimStatus::as_str),
                "verified experimentally_observed inferred community_established unknown \
                 requires_further_validation",
            ),
            (
                names(ClockKind::ALL, ClockKind::as_str),
                "session linux_monotonic linux_realtime linux_boottime x_server_ms wayland_utime \
                 gameinput_us mach_absolute browser_perf",
            ),
            (
                names(ClockMeaning::ALL, ClockMeaning::as_str),
                "session_clock kernel_flush kernel_event os_dispatch compositor browser_receipt",
            ),
            (
                names(ReceiptMeaning::ALL, ReceiptMeaning::as_str),
                "app_dequeue dom_callback imported",
            ),
        ];
        for (got, want) in cases {
            assert_eq!(got.join(" "), want);
        }
        for &v in InputLevel::ALL {
            assert_eq!(v.as_str().parse::<InputLevel>(), Ok(v));
        }
        for &v in ClaimStatus::ALL {
            assert_eq!(v.to_string().parse::<ClaimStatus>(), Ok(v));
        }
        assert_eq!("VERIFIED".parse::<ClaimStatus>(), Err(UnknownName));
        assert_eq!("".parse::<Units>(), Err(UnknownName));
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[test]
    fn json_names() {
        assert_eq!(
            serde_json::to_string(&InputLevel::BrowserLockedUnadjusted).unwrap(),
            "\"browser_locked_unadjusted\""
        );
        assert_eq!(
            serde_json::from_str::<ClaimStatus>("\"community_established\"").unwrap(),
            ClaimStatus::CommunityEstablished
        );
        assert!(serde_json::from_str::<Units>("\"COUNTS\"").is_err());
    }
}
