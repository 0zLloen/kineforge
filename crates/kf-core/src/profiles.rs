//! Sensitivity constants of other applications. Every constant carries its
//! [`ClaimStatus`] and source (AIMLABS_RESEARCH.md §4). A passing unit test of
//! the formulas does not establish that an application uses them
//! (EXPERIMENT_PLAN.md §0.1); constants are measured by E3/E4 before they are
//! upgraded.

use crate::provenance::ClaimStatus;
use crate::sensitivity::FovModel;
use crate::units::{Aspect, Degrees, Radians};

/// A labelled value. `value` is `None` exactly when `status` is
/// [`ClaimStatus::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Claim<T> {
    pub value: Option<T>,
    pub status: ClaimStatus,
    /// Where the value (or the search for it) comes from.
    pub source: &'static str,
}

/// One application's linear sensitivity model: angle per count = constant ×
/// sensitivity, assuming raw input and no acceleration or smoothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameProfile {
    /// Stable identifier (lower-case, `_`-separated).
    pub id: &'static str,
    pub name: &'static str,
    /// Yaw per count at sensitivity 1.
    pub yaw: Claim<Degrees>,
    /// Pitch per count at sensitivity 1.
    pub pitch: Claim<Degrees>,
    /// How the FOV setting scales with aspect ratio.
    pub fov: Claim<FovModel>,
}

impl GameProfile {
    /// Yaw per count at sensitivity 1; `None` while unknown.
    pub fn yaw_per_count(&self) -> Option<Radians> {
        self.yaw.value.map(Degrees::to_radians)
    }

    /// The profile with this `id`.
    pub fn find(id: &str) -> Option<&'static GameProfile> {
        PROFILES.iter().find(|p| p.id == id)
    }
}

const COMMUNITY: &str = "community value; no primary source found (AIMLABS_RESEARCH.md §4)";
const NOT_RESEARCHED: &str = "not researched";

const fn known<T>(value: T, status: ClaimStatus, source: &'static str) -> Claim<T> {
    Claim {
        value: Some(value),
        status,
        source,
    }
}

const fn unknown<T>(source: &'static str) -> Claim<T> {
    Claim {
        value: None,
        status: ClaimStatus::Unknown,
        source,
    }
}

const fn community(deg: f64) -> Claim<Degrees> {
    known(Degrees(deg), ClaimStatus::CommunityEstablished, COMMUNITY)
}

/// The profile table (AIMLABS_RESEARCH.md §4). Values the research did not
/// establish are `unknown`, never estimated.
pub static PROFILES: &[GameProfile] = &[
    GameProfile {
        id: "source_sdk_2013",
        name: "Source engine (SDK 2013)",
        yaw: known(
            Degrees(0.022),
            ClaimStatus::Verified,
            "Source SDK 2013 src/game/client/in_mouse.cpp: m_yaw default \"0.022\" (github.com/ValveSoftware/source-sdk-2013 @3e957b70b167)",
        ),
        pitch: known(
            Degrees(0.022),
            ClaimStatus::Verified,
            "Source SDK 2013 src/game/client/in_mouse.cpp: m_pitch default \"0.022\" (github.com/ValveSoftware/source-sdk-2013 @3e957b70b167)",
        ),
        fov: known(
            FovModel::HorPlus {
                basis: Aspect::R4_3,
            },
            ClaimStatus::Verified,
            "Source SDK 2013 src/game/client/view.cpp: ScaleFOVByWidthRatio(fov, GetScreenAspectRatio() × 0.75), f32 arithmetic; covers aspects ≤ 1.85:1, or wider while sv_restrict_aspect_ratio_fov is 0 (2, or > 0 in windowed multiplayer, caps the aspect at 1.85:1) (github.com/ValveSoftware/source-sdk-2013 @3e957b70b167)",
        ),
    },
    GameProfile {
        id: "quake3",
        name: "Quake III Arena",
        yaw: known(
            Degrees(0.022),
            ClaimStatus::Verified,
            "Quake III Arena code/client/cl_main.c: m_yaw default \"0.022\" (github.com/id-Software/Quake-III-Arena @dbe4ddb10315)",
        ),
        pitch: known(
            Degrees(0.022),
            ClaimStatus::Verified,
            "Quake III Arena code/client/cl_main.c: m_pitch default \"0.022\" (github.com/id-Software/Quake-III-Arena @dbe4ddb10315)",
        ),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "cs2",
        name: "Counter-Strike 2",
        yaw: community(0.022),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "apex_legends",
        name: "Apex Legends",
        yaw: community(0.022),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "overwatch2",
        name: "Overwatch 2",
        yaw: community(0.0066),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "valorant",
        name: "VALORANT",
        yaw: community(0.07),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "rainbow6_siege",
        name: "Rainbow Six Siege",
        yaw: unknown(
            "published structure (input yaw × multiplier unit × sens) is verified; the base constant is unpublished (AIMLABS_RESEARCH.md §4)",
        ),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(NOT_RESEARCHED),
    },
    GameProfile {
        id: "aimlabs",
        name: "Aim Lab",
        yaw: unknown(
            "native scale undocumented; to be measured by AL-E experiments (AIMLABS_RESEARCH.md §4)",
        ),
        pitch: unknown(NOT_RESEARCHED),
        fov: unknown(
            "Source-like 4:3 Hor+ with the CS profile is a single-report hypothesis for AL-E2, not a profile (AIMLABS_RESEARCH.md §4)",
        ),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_invariants() {
        for (i, p) in PROFILES.iter().enumerate() {
            assert!(
                !p.id.is_empty()
                    && p.id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                p.id
            );
            assert!(
                PROFILES[..i].iter().all(|q| q.id != p.id),
                "duplicate {}",
                p.id
            );
            assert_eq!(GameProfile::find(p.id), Some(p));
            let statuses = [
                (p.yaw.value.is_some(), p.yaw.status, p.yaw.source),
                (p.pitch.value.is_some(), p.pitch.status, p.pitch.source),
                (p.fov.value.is_some(), p.fov.status, p.fov.source),
            ];
            for (has_value, status, source) in statuses {
                assert_eq!(has_value, status != ClaimStatus::Unknown, "{}", p.id);
                assert!(!source.is_empty(), "{}", p.id);
                // Verified constants cite a pinned commit.
                if status == ClaimStatus::Verified {
                    assert!(source.contains(" @"), "{}: {source}", p.id);
                }
            }
            if let Some(d) = p.yaw.value {
                assert!(d.0 > 0.0 && d.0.is_finite(), "{}", p.id);
            }
            assert_eq!(p.yaw_per_count().is_some(), p.yaw.value.is_some());
        }
        assert_eq!(GameProfile::find("nope"), None);
    }

    /// Values and statuses copied from AIMLABS_RESEARCH.md §4: a changed
    /// constant or label must be a deliberate edit of both.
    #[test]
    fn table_matches_research() {
        use ClaimStatus::{CommunityEstablished as C, Unknown as U, Verified as V};
        type Row = (
            &'static str,
            (Option<f64>, ClaimStatus),
            (Option<f64>, ClaimStatus),
            (Option<FovModel>, ClaimStatus),
        );
        let hor_plus_4_3 = Some(FovModel::HorPlus {
            basis: Aspect::R4_3,
        });
        let want: [Row; 8] = [
            (
                "source_sdk_2013",
                (Some(0.022), V),
                (Some(0.022), V),
                (hor_plus_4_3, V),
            ),
            ("quake3", (Some(0.022), V), (Some(0.022), V), (None, U)),
            ("cs2", (Some(0.022), C), (None, U), (None, U)),
            ("apex_legends", (Some(0.022), C), (None, U), (None, U)),
            ("overwatch2", (Some(0.0066), C), (None, U), (None, U)),
            ("valorant", (Some(0.07), C), (None, U), (None, U)),
            ("rainbow6_siege", (None, U), (None, U), (None, U)),
            ("aimlabs", (None, U), (None, U), (None, U)),
        ];
        let got: Vec<Row> = PROFILES
            .iter()
            .map(|p| {
                (
                    p.id,
                    (p.yaw.value.map(|d| d.0), p.yaw.status),
                    (p.pitch.value.map(|d| d.0), p.pitch.status),
                    (p.fov.value, p.fov.status),
                )
            })
            .collect();
        assert_eq!(got, want);
    }
}
