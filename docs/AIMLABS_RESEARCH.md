# AIMLABS_RESEARCH — what permitted research can establish

Research dates: 2026-11-13/14. Aimlabs is a **reference** for observable
behaviour only. Nothing here is, or may become, a copy of Aimlabs code,
assets, scenarios or formulas. Working notes with the full source list:
[`research/notes/aimlabs.md`](../research/notes/aimlabs.md) (marked private —
it contains a file-level inventory that should not be published, see §6.4).

## Evidence labels

This document labels every finding with a `claim_status` value — VERIFIED, EXPERIMENTALLY OBSERVED, INFERRED, COMMUNITY-ESTABLISHED, UNKNOWN, REQUIRES FURTHER VALIDATION — defined once in [EXPERIMENT_PLAN.md §0.2](EXPERIMENT_PLAN.md). Nothing here is EXPERIMENTALLY OBSERVED yet: no experiment has been run.

## 1. Summary

1. **No Aimlabs-specific constant, formula or scoring rule is published, and none is claimed here.** The native sensitivity scale (yaw per count), vertical ratio, ADS rule, acceleration options and score formulas are all **UNKNOWN**. They can only be established by the black-box experiments in §7, which have not been run.
2. Aimlabs has **never been launched** in this Proton prefix, so there are no settings keys, logs or session records to inventory yet (VERIFIED by listing). Launching it is the user's decision because it writes to the prefix.
3. The Terms of Service forbid reverse engineering and automated extraction of output; passive observation during normal use is the defensible method (§6). Automated injection + automated capture rigs need legal review or Statespace's consent first.
4. On this host (Proton 11, X11), Windows "raw input" inside the game is fed from X11 XInput2 raw motion, whose raw values come from libinput's unaccelerated deltas, so the host's libinput "adaptive" profile does not apply to that path. Labels: Wine derives Windows raw input from XI2 raw motion — VERIFIED (Proton source); XI2 raw = libinput unaccelerated — VERIFIED (driver source); equality with HID counts (no rotation or scaling) — INFERRED, to be tested by X-L1 and AL-E0.
5. The sensitivity mathematics KineForge needs is engine-independent and well established (§4). It does not depend on Aimlabs at all.

## 2. Research-capability inventory (requirement 2.1)

| Method | Available? | Status / notes |
|---|---|---|
| Official documentation | Partly | aimlabs.com articles readable; **support.aimlabs.com returns HTTP 403 to automated fetchers** — not bypassed; needs a manual read in a browser (PC Sensitivity Guide, Troubleshooting Guide, Guest Mode) |
| Published sensitivity formulas | **No** | Official material says a "Game Profile determines the fundamental sensitivity scale and formula" but publishes none. One official example (VALORANT 0.4 → Overwatch ≈1.35) contradicts community constants (which give ≈4.24) — REQUIRES FURTHER VALIDATION; treat marketing articles as non-authoritative for maths |
| User-accessible settings | Yes, after first launch | Game Profile, sensitivity, manual DPI entry, FOV (locked for some profiles), ADS multiplier, community-reported "Advanced FOV options". Key names/types UNKNOWN until launched |
| Supported data exports | **No public API or export** | Only route: privacy-policy request for a copy of your own data (help@statespacelabs.com). Whether that includes per-shot telemetry: UNKNOWN. The game's private backend services must not be called |
| Accessible session records | **None present** | No `Player.log`, no `LocalLow/Statespace/aimlab_tb`, no registry key — never launched |
| External mouse input recordings | **Yes — lowest risk** | Recording your own mouse on the host (evdev, or MouseTester on Windows) touches nothing in Aimlabs. Open Directional DPI already uses this: its calibration CLI has an `aimlabs` provider alias that ingests MouseTester-format logs recorded while playing (OPEN_DIRECTIONAL_DPI_AUDIT.md §5) |
| Screen recordings | Yes | Recording your own play is normal use (ToS §13 permits streaming under conditions). `ffmpeg -f x11grab` / OBS outside the game |
| Controlled behavioural experiments | Yes, with conditions | Human-operated: OK. Synthetic input + automated capture: grey zone (§6) |
| File formats documented by their owners | **None** | Creator Studio files are Easy Save 3 JSON with parametric scoring keys (ScoreHit/Miss/Kill, ScoreSQRTBonus, multipliers, time bonuses) and per-weapon `zoomRatio` — INFERRED from plain text; **not** a documented or supported format |
| Reverse-engineering tools (REA) | Not used | Locally present: `objdump`, `strings`, `gdb` only (no Ghidra/radare2/IL2CPP dumpers). **No authorization exists** for Aimlabs (ToS §12(d), §15); its code was not inspected and will not be |

Additional local facts relevant to experiments: the user account has read/write access to `/dev/uinput` through an ACL (VERIFIED, `getfacl`), so a virtual mouse can be created without changing permissions; `python3-evdev` is not installed; the user is not in the `input` group (reading `/dev/input/event*` would need a permission change — ask first).

## 3. What the install shows (requirement 2.1, inventory)

| Finding | Label |
|---|---|
| Unity **2022.3.62f2**, IL2CPP build (`GameAssembly.dll`, `il2cpp_data/`) | VERIFIED (version string, file names) |
| Unity's `Camera.fieldOfView` is the **vertical** FOV; any horizontal FOV setting must be converted | VERIFIED (Unity docs); how Aimlabs converts: INFERRED/UNKNOWN |
| No EasyAntiCheat/BattlEye directories; client-side anti-tamper components may be present | VERIFIED (directory listing) for the former. Irrelevant as long as nothing touches the process — and nothing will |
| Native input plugins for Rewired (Raw Input/DirectInput/Windows.Gaming.Input) in the plugins folder; Unity's own input paths may also be used | VERIFIED (plugin file names) for the Rewired plugins. **Which path drives mouse-look: UNKNOWN** → experiment AL-E0 |
| Steam appid 714010, build 26479530, runs under Proton Experimental 11 | VERIFIED |
| Task names: Gridshot, Sixshot, Spidershot, Microshot, Multishot, Reflexshot, Snipershot, Microflex, Motionshot, Strafetrack, Strafeshot, Circletrack, Circleshot, Spheretrack, Switchtrack, Motiontrack, Linetrace, Blinkshot, Detection, Capacity, … each in Ultimate/Standard/Precision/Speed variants | VERIFIED (names only, from tutorial video names and plain-text playlists). Mechanics: see §5 |
| ProtonDB: gold tier (177 reports); one report claims Linux support was removed | REQUIRES FURTHER VALIDATION (a launch will settle it) |

## 4. Sensitivity systems (requirement 2.2)

Notation: k = degrees of yaw per count at sensitivity 1; s = in-game sensitivity; CPI = counts per inch.

| Item | Statement | Label |
|---|---|---|
| Yaw per count | θ = k·s | VERIFIED for engines with published source (below); assumed linear for others until measured |
| cm/360 | cm360 = 2.54 · 360 / (k · s · CPI) | VERIFIED (derivation). e.g. k = 0.022, s = 1, 800 CPI → 51.95 cm |
| Same-feel conversion (equal cm/360) | s₂ = s₁ · k₁ / k₂ (same CPI) | VERIFIED (derivation) |
| Source engine (SDK 2013) | `m_yaw` = `m_pitch` = 0.022 default; `viewangles[YAW] -= m_yaw * mouse_x` after `ScaleMouse` applies `sensitivity` (`src/game/client/in_mouse.cpp` @3e957b70b167) | VERIFIED (source code) |
| Quake III | `m_yaw` = `m_pitch` = 0.022 | VERIFIED (source code) |
| CS2, Apex Legends | 0.022 | COMMUNITY-ESTABLISHED |
| Overwatch 2 | 0.0066 | COMMUNITY-ESTABLISHED |
| VALORANT | 0.07 | COMMUNITY-ESTABLISHED |
| Rainbow Six Siege | Published structure (yaw = input yaw × multiplier unit × sens); base constant unpublished | VERIFIED (structure) / UNKNOWN (constant) |
| **Aimlabs native scale** | — | **UNKNOWN** |
| Horizontal ↔ vertical FOV | H = 2·atan(tan(V/2)·AR); V = 2·atan(tan(H/2)/AR) | VERIFIED (geometry) |
| Hor+ from a 4:3 basis (Source) | H = 2·atan(tan(H₄:₃/2)·(AR / (4/3))); 90° → 106.26° at 16:9. The engine caps AR at 1.85:1 when `sv_restrict_aspect_ratio_fov` is 2, or > 0 in windowed multiplayer (`view.cpp`) | VERIFIED (Source SDK `ScaleFOVByWidthRatio`, `src/game/client/view.cpp` @3e957b70b167) for AR ≤ 1.85:1 or with the restriction off |
| Aimlabs FOV field with the CS profile behaves like Source 4:3 Hor+ | a single community report ("FOV 90 → 73.74° V / 106.26° H at 16:9", one aspect ratio) is consistent with Source 4:3-basis Hor+ — a **hypothesis** for AL-E2 to test at ≥2 aspect ratios; not implemented as a profile before then | REQUIRES FURTHER VALIDATION |
| Monitor-distance match at fraction p of the half-width | s₂/s₁ = atan(p·tan(H₂/2)) / atan(p·tan(H₁/2)) | VERIFIED (derivation); "MDH/MDV %" naming COMMUNITY-ESTABLISHED |
| 0 % match ("focal length", zoom ratio) | p → 0: s₂/s₁ = tan(H₂/2) / tan(H₁/2) | VERIFIED (derivation) |
| 100 % match | s₂/s₁ = H₂/H₁ | VERIFIED (derivation) |
| Viewspeed variants | several competing community formulas | COMMUNITY-ESTABLISHED; not adopted as fact |
| Pixel ↔ angle near screen centre | dθ/dx = 2·tan(H/2)/W rad/px | VERIFIED (derivation) |
| Hipfire vs ADS in Aimlabs | separate ADS/scoped multiplier; Creator Studio weapons carry a `zoomRatio` | VERIFIED (multiplier exists) / INFERRED (zoomRatio) / UNKNOWN (rule) |
| Vertical ratio, acceleration, raw-input toggle, smoothing, quantisation in Aimlabs | not documented | UNKNOWN |
| Input quantisation and smoothing in general | integer counts per report; any smoothing is application-side | VERIFIED (HID semantics) for counts; application behaviour must be measured |
| Coordinate conventions | Source and Quake III both subtract `m_yaw * mx` from a yaw that increases to the left (Source's `+=` is only Portal's upside-down branch); KineForge's yaw increases to the right | VERIFIED (source: Source `in_mouse.cpp`, Quake III `code/client/cl_input.c` @dbe4ddb10315); KineForge's convention is fixed in DATA_SPECIFICATION.md §2.2 |

**Rule for KineForge:** sensitivity "game profiles" in KineForge carry their constant, its label (VERIFIED / COMMUNITY-ESTABLISHED / INFERRED-by-experiment-ID) and source. No profile for Aimlabs's native scale exists until experiment AL-E1 produces one, and it will carry its experiment ID and confidence interval.

## 5. Aim-training mechanics → generalised models (requirement 2.3)

No official description of task mechanics could be retrieved. Partner/community descriptions (Gridshot = 3 simultaneous static targets; Sixshot = 6; Spidershot = centre/peripheral alternation; Microshot = small close flicks; Motionshot = fast-moving targets; Strafetrack = horizontal tracking; Circletrack = tracking with added movement) are **INFERRED** and to be validated only by playing. KineForge does **not** reproduce any of these tasks. Instead it defines generalised, parametric task families from first principles:

| Family | Independent variables (all seeded, recorded) | What it trains / measures |
|---|---|---|
| **Static flick** | target count on screen, angular distance distribution, angular size distribution, direction set (balanced over 8/16/24 directions), spawn rule (on hit / on timer), lifetime | target acquisition, endpoint error, overshoot/undershoot, Fitts throughput, directional performance |
| **Centre-out flick** | return-to-centre constraint, distances, directions | clean per-direction flick data (calibration input for directional gain) |
| **Target switching** | n targets, switch order rule, inter-target angles | switching time, reacquisition |
| **Reactive / reflex** | random onset delay distribution, position distribution | visual reaction time (stimulus onset → movement onset), time to hit |
| **Precision (micro)** | small angular sizes, short distances | microcorrections, submovement count |
| **Dynamic flick** | target speed distribution, velocity direction relative to the flick vector (toward / away / tangential), analytic path (constant velocity or arc), lifetime | endpoint error relative to the target at click time, signed lead/lag along target velocity, time-to-hit vs speed |
| **Smooth tracking** | trajectory model: sum of non-harmonic sinusoids / smooth noise / constant-velocity strafes with seeded reversal times; speed spectrum | RMS angular error, time-on-target, lag (cross-correlation), gain/phase per frequency |
| **Reactive tracking** | tracking with sudden velocity changes (fakeouts) at seeded times | response latency to velocity changes |

All families share **pacing** parameters (spawn interval, time budget per target or per run) as an explicit difficulty axis. Each family is a pure function of `(seed, parameters, time)` in `kf-sim`, so stimuli are reproducible bit-for-bit and difficulty can be swept parametrically (distance, size, speed, onset jitter). Scenario files are KineForge's own format; nothing is imported from Aimlabs or its workshop.

## 6. Legal boundaries (requirement 2.1)

### 6.1 Terms of Service (aimlabs.com/terms-of-service, last updated 2025-11-01)
§12 licence is "solely for your personal and non-commercial purposes" and prohibits, without a "to the extent permitted by law" carve-out: creating derivative works (b); attempting to "discover the source code" or underlying components (d); using content to train ML models (e); "any automated or programmatic method to extract data or output" (f). §15 prohibits reverse engineering, circumventing technological measures, bots/data-mining tools, and commercial use. §13 permits streaming under conditions. Governing law: New York, arbitration.

### 6.2 EU/EEA statutory overlay
Directive 2009/24/EC Art. 5(3) lets a lawful user "observe, study or test" a program while running it normally; Art. 8 makes contrary contract terms null and void. Decompilation (Art. 6) is limited to interoperability — not needed and not used. Outside the EU the ToS governs. **This is not legal advice.**

### 6.3 Permitted methods for this project

| Method | Status |
|---|---|
| Official docs, ToS, public articles, community knowledge | OK |
| Normal play while recording your own screen/camera | OK |
| Human-operated measurements with a real mouse; host-side logging of your own mouse | OK (lowest risk) |
| Calibration maps built in the official Creator Studio (kept private) | OK |
| Host-side environment variables that do not touch the install/prefix (e.g. `PROTON_LOG`; `DXVK_LOG_PATH` set **outside** the install, because DXVK otherwise writes logs into the game directory) | Low risk |
| Synthetic input (uinput / USB-HID microcontroller) + automated capture/analysis | **Grey** under §12(f)/§15 — only after legal review or written consent; KineForge's commercial status matters |
| Training ML on Aimlabs outputs | **Prohibited** (§12(e)) |
| Reading binaries/metadata, asset extraction, memory reading, hooking, network/API calls, bypassing anti-tamper or authentication | **Prohibited** |

### 6.4 Caveat on our own inventory
Listing a plain-text manifest of assembly names is arguably within §12(d)'s "underlying components" wording. That detail stays in the private notes and **no KineForge design decision is based on Aimlabs internals.**

## 7. Experiments to establish unknowns (designed, NOT run)

Procedures: the last column maps each AL-E* to its EXPERIMENT_PLAN.md entry (AL-E0 and AL-E5 are specified only in `research/notes/aimlabs.md` §7). Preconditions: the user launches Aimlabs (writes the prefix), uses guest/offline mode, never submits synthetic-input scores, and obtains sign-off before automated rigs.

| ID | Question | Method | Expected precision | EXPERIMENT_PLAN procedure |
|---|---|---|---|---|
| AL-E0 | Which input path drives mouse-look? Does the raw path carry exact counts under Proton? | Own Win32 WM_INPUT logger in a separate prefix (calibrates Proton); then equal-total slow vs fast injection with libinput adaptive vs flat — speed-independent rotation ⇒ raw path | exact (integer counts) | research/notes/aimlabs.md §7 (E0) only |
| AL-E1 | Yaw per count per sensitivity and profile; linearity; pitch ratio; frame-rate/report-rate invariance | Full-turn image-closure (≈6×10⁻⁵ relative at ~16 k counts/turn) and small-angle optical method; sens {0.25, 0.5, 1, 2, 4} × profiles × FPS caps × rates | per-run CI reported | E3 |
| AL-E2 | FOV mapping, Hor+/Vert−, ADS FOV and ADS sensitivity rule | Pixel displacement of markers at known angles; multiple aspect ratios; compare ADS to 0/56.25/75/100 % monitor-distance predictions | ≈±0.02° H at 1080p | E4 |
| AL-E3 | Scoring | Creator Studio levels varying one scoring parameter at a time; native tasks: ≥40 runs each, results transcribed manually, model comparison by AIC | exact for parametric custom levels; residual-stated model for native tasks — never presented as Aimlabs's formula | E9 |
| AL-E4 | Click-to-photon latency, frame pacing | Photodiode + microcontroller that also injects the click (shared clock) | ≈±0.1 ms | E5 |
| AL-E5 | Report-rate and accumulation behaviour | Equal totals at 125/500/1000 Hz and as few large vs many small reports | exact | research/notes/aimlabs.md §7 (E5) only |

Preferred count source: a USB-HID microcontroller (same USB→evdev→libinput→XI2 path as a real mouse); uinput is acceptable for AL-E0; XTEST-injected motion has unknown raw-event semantics and should not be used.

## 8. Scoring and statistics (requirement 2.4)

**Verified from official glossary (definitions only):** accuracy = shots landed / shots fired; visual reaction time = target appearance → reaction; TTK; crosshair uptime (time on target while tracking). **UNKNOWN:** kills/targets per second definitions, native score formulas, how on-target % is sampled.

There is no evidence of a universal Aimlabs scoring formula; Creator Studio exposes several parametric scoring keys, which indicates task-specific scoring. **KineForge therefore defines its own configurable scoring framework** (in `kf-sim`, versioned as `scoring_version`):

- **Primitive events** (simulated truth): shot (time, angular error to nearest target centre and edge), hit/miss, kill, target spawn/despawn, per-tick on-target state.
- **Score-eligible metrics** (`kf-sim` `score` module; bit-exact, simulation events only): accuracy; hits/kills per second; time to hit; time to acquire (onset → first on-target tick); angular error at shot; on-target ticks / time-on-target; onset → first hit.
- **Analytics-only metrics** (`kf-analytics`; derived, tolerance-based, never part of an official score): velocity-threshold reaction time (onset → movement onset), flick endpoint error along/across, overshoot ratio, submovement count, tracking RMS error and lag, smoothness (normalised jerk, spectral arc length), directional performance per bin, Fitts throughput.
- **Score models** are declarative compositions of score-eligible metrics only (the validator rejects scenario files that reference an analytics-only metric) (e.g. `score = Σ kill_points + accuracy_weight·accuracy + …`) defined in scenario files, with the formula and version stored in every session. The same raw session can be re-scored under any model.

Which external formulas can be verified: only Creator Studio's parametric rules (AL-E3, controlled levels); native-task formulas at best become fitted models with stated residuals.

## 9. Reproducibility (requirement 2.5)

| Source of variation | How it is separated |
|---|---|
| Input-system error | Count-fidelity experiments with a programmable HID source (EXPERIMENT_PLAN E1); every session records input level, clock domain and processing flags |
| Measurement noise (timestamps) | Clock characterisation (E0); timestamp uncertainty recorded per stream |
| Sensitivity effects | Within-subject, counterbalanced blocks with identical seeded stimuli (E7) |
| Frame-rate effects | Frame rate as an explicit factor; KineForge's camera is designed to be frame-rate-independent (to be verified by E2 in CI) — residual effects are perceptual/human, not computational |
| Task difficulty | Parametric task families with recorded parameters; difficulty swept explicitly |
| Random scenario effects | Seeded, value-stable RNG; identical stimulus sequences across conditions |
| Human variation (learning, fatigue) | Warm-up exclusion, multiple sessions/days, mixed-effects models, effect sizes with CIs |

For **external** (Aimlabs) sessions, only host-side input streams and app-reported results exist; target positions and camera state are absent and are never reconstructed and presented as observations (DATA_SPECIFICATION.md §3.2).

## 10. Open items

1. Launch Aimlabs once (user decision) → re-run the read-only settings/log inventory (command in the notes §2.2).
2. Manually read the 403-protected support pages.
3. Decide KineForge's commercial status (affects ToS §15 analysis).
4. Legal review or Statespace consent before any automated injection/capture rig.
5. Explain the official VALORANT→Overwatch example discrepancy (1.35 vs computed 4.24).

## Sources (accessed 2026-11-13)

Aimlabs ToS https://aimlabs.com/terms-of-service · Steam EULA https://store.steampowered.com/eula/714010_eula_0 · Privacy policy https://aimlabs.com/privacy-policy · Sensitivity articles https://aimlabs.com/articles/aimlabs/how-to-configure-and-convert-your-sensitivity-in-aimlabs/ and https://aimlabs.com/articles/aimlabs/did-you-know-aimlabs-has-a-sensitivity-converter-built-in/ · Glossary https://aimlabs.com/articles/?p=3154 · Crosshair uptime https://aimlabs.com/articles/aimlabs/a-quick-explainer-on-crosshair-uptime/ · Creator Studio update https://aimlabs.com/articles/aimlabs/we-have-updated-the-aimlabs-creators-studio/ · Unity docs (FOV, log files, PlayerPrefs, raw input) https://docs.unity3d.com/2022.3/Documentation/ScriptReference/Camera-fieldOfView.html · Proton Wine `experimental_11.0` `dlls/winex11.drv/{mouse.c,window.c}` @ 5b557605 https://github.com/ValveSoftware/wine/tree/experimental_11.0/dlls/winex11.drv · XI2 protocol spec https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/raw/master/specs/XI2proto.txt · xf86-input-libinput https://gitlab.freedesktop.org/xorg/driver/xf86-input-libinput/-/raw/master/src/xf86libinput.c · libinput API https://gitlab.freedesktop.org/libinput/libinput/-/raw/main/src/libinput.h · Source SDK 2013 `in_mouse.cpp`, `view.cpp` https://github.com/ValveSoftware/source-sdk-2013 · Quake III `cl_main.c`, `cl_input.c` https://github.com/id-Software/Quake-III-Arena · Ubisoft R6 FOV/sensitivity (2019-04-18) https://www.ubisoft.com/en-us/game/rainbow-six/siege/news-updates/6kY6b5JByBY3P6vQWWinla/fov-and-input-sensitivity · EU Directive 2009/24/EC Art. 5, 8 https://www.legislation.gov.uk/eudr/2009/24/article/5 · ProtonDB https://www.protondb.com/api/v1/reports/summaries/714010.json · Community constants https://csdb.gg/sensitivity-converter/ · Steam community threads https://steamcommunity.com/app/714010/discussions/0/4289187621809424581
