//! `kf-cli replay`: re-simulates a session from its raw log, frame by frame
//! (DATA_SPECIFICATION.md §9.1), and compares every regenerated
//! `sim_control`, `sim_camera`, `sim_target` and `sim_shot` record with the
//! logged one. Memory stays bounded: the log is read group by group.

use kf_core::{SimCamera, SimShot, SimTarget, SourceId, Units, ValueScale};
use kf_sim::replay::{Item, Mismatch, Replay, Report};
use kf_sim::{Attempt, FlickConfig, Scenario, SimConfig, SimRecord};
use kf_telemetry::format::Kind;
use kf_telemetry::json::{SessionHeader, SourceDeclaration};
use kf_telemetry::reader::GroupReader;
use kf_telemetry::record::Record;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;

/// The `simulation.rng`, `sensitivity.directional_gain`, `scenario` and
/// `scoring` header fields mapped to a simulation (DATA_SPECIFICATION.md §7;
/// FORMAT.md, `session_header`).
pub fn sim_config(h: &SessionHeader, driving: &SourceDeclaration) -> Result<SimConfig, String> {
    let sim = h
        .simulation
        .as_ref()
        .ok_or("the header has no simulation")?;
    let sens = h
        .sensitivity
        .as_ref()
        .ok_or("the header has no sensitivity")?;
    if sens.model != "linear" {
        return Err(format!("sensitivity model {:?} (only linear)", sens.model));
    }
    let rng = sim.rest.get("rng").ok_or("simulation.rng is missing")?;
    if rng.get("algorithm") != Some(&json!("xoshiro256++"))
        || rng.get("seeding") != Some(&json!("splitmix64"))
    {
        return Err(format!(
            "simulation.rng {rng} is not xoshiro256++ seeded by splitmix64"
        ));
    }
    let seed = rng
        .get("seed")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix("0x"))
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .ok_or("simulation.rng.seed must be a \"0x…\" hex string")?;
    let value_scale = ValueScale::from_u16(driving.value_scale).ok_or("bad value_scale")?;
    let gain = match sens.rest.get("directional_gain") {
        None | Some(Value::Null) => None,
        Some(g) => {
            if g.get("policy") != Some(&json!("carry_capped")) {
                return Err(format!(
                    "directional_gain policy {:?} (only carry_capped)",
                    g.get("policy")
                ));
            }
            if driving.units != Units::Counts || value_scale != ValueScale::X1 {
                return Err("directional gain on a source that is not counts × 1".into());
            }
            let hex = g.get("profile").and_then(Value::as_str).unwrap_or("");
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
                .collect();
            let arr: [u8; kf_dirgain::PROFILE_LEN] = bytes
                .try_into()
                .ok()
                .filter(|_| hex.len() == 2 * kf_dirgain::PROFILE_LEN)
                .ok_or("directional_gain.profile must be 56 hex digits")?;
            Some(
                kf_dirgain::Profile::deserialize(&arr)
                    .ok_or("directional_gain.profile is invalid")?,
            )
        }
    };
    let scenario = h.rest.get("scenario").ok_or("the header has no scenario")?;
    if scenario.get("id") != Some(&json!(kf_sim::flick::SCENARIO_ID))
        || scenario.get("version") != Some(&json!(kf_sim::flick::SCENARIO_VERSION))
    {
        return Err(format!(
            "scenario {} version {} is not supported",
            scenario.get("id").unwrap_or(&Value::Null),
            scenario.get("version").unwrap_or(&Value::Null)
        ));
    }
    let flick: FlickConfig =
        serde_json::from_value(scenario.get("config").cloned().unwrap_or_default())
            .map_err(|e| format!("scenario.config: {e}"))?;
    let scoring = h.rest.get("scoring");
    if scoring.and_then(|s| s.get("model")) != Some(&json!("kf.flick.v1"))
        || scoring.and_then(|s| s.get("scoring_version")) != Some(&json!(1))
    {
        return Err(format!(
            "scoring {} is not kf.flick.v1 version 1",
            scoring.unwrap_or(&Value::Null)
        ));
    }
    Ok(SimConfig {
        sim_hz: sim.sim_hz,
        seed,
        sensitivity: kf_core::LinearSensitivity {
            yaw_rad_per_unit: sens.yaw_rad_per_unit,
            pitch_rad_per_unit: sens.pitch_rad_per_unit,
            sens_x: sens.sens_x,
            sens_y: sens.sens_y,
            invert_y: sens.invert_y,
        },
        value_scale,
        gain,
        driving_source: sim.input_source,
        max_catch_up_ticks: 0, // live only
        scenario: Scenario::StaticFlick(flick),
    })
}

fn item(r: &Record) -> Option<Item> {
    Some(match r {
        Record::Motion(m) => Item::Event(kf_core::InputEvent::Motion(*m)),
        Record::Button(b) => Item::Event(kf_core::InputEvent::Button(*b)),
        Record::Wheel(w) => Item::Event(kf_core::InputEvent::Wheel(*w)),
        Record::Frame(f) => Item::Frame(*f),
        Record::Drain(d) => Item::Drain(*d),
        Record::SimControl(c) => Item::Control(*c),
        Record::SimCamera(c) => Item::Logged(SimRecord::Camera(*c)),
        Record::SimTarget(t) => Item::Logged(SimRecord::Target(*t)),
        Record::SimShot(s) => Item::Logged(SimRecord::Shot(*s)),
        Record::Annotation(a) => Item::Annotation(a.clone()),
        _ => return None,
    })
}

/// A replay and what recorded the session (DATA_SPECIFICATION.md §9.4: a
/// replay by another build is allowed but reported).
pub struct Replayed {
    pub report: Report,
    /// The header's `app` object.
    pub recorded_by: Value,
    /// `sensitivity.directional_gain.engine`, if any.
    pub gain_engine: Value,
}

/// Replays the log in `r`. `Err` = it cannot be replayed at all (no
/// simulation, unsupported configuration, unreadable header). A read error
/// (CRC mismatch, torn tail) or a missing terminal `session_end` after an
/// otherwise identical replay is reported as an error: what follows was
/// never compared. A recovered session directory ends with its synthetic
/// `session_end` and replays normally.
pub fn replay<R: Read>(r: R) -> Result<Replayed, String> {
    let mut groups = GroupReader::new(r).map_err(|e| e.to_string())?;
    let mut header: Option<SessionHeader> = None;
    let mut decls: BTreeMap<SourceId, SourceDeclaration> = BTreeMap::new();
    let mut rep: Option<Replay> = None;
    for g in groups.by_ref() {
        for rec in g.records() {
            if let Record::Json { kind, value } = rec {
                match kind {
                    Kind::SessionHeader => {
                        header = Some(
                            serde_json::from_value(value.clone())
                                .map_err(|e| format!("header: {e}"))?,
                        );
                    }
                    Kind::SourceDeclare => {
                        if let Ok(d) = serde_json::from_value::<SourceDeclaration>(value.clone()) {
                            decls.insert(d.id, d);
                        }
                    }
                    _ => {}
                }
            }
        }
        if rep.is_none()
            && let Some(h) = &header
            && let Some(sim) = &h.simulation
            && let Some(d) = decls.get(&sim.input_source)
        {
            rep =
                Some(Replay::new(sim_config(h, d)?).map_err(|e| format!("configuration: {e:?}"))?);
        }
        let Some(r) = &mut rep else {
            if g.records().any(|r| matches!(r, Record::Frame(_))) {
                return Err(
                    "frames before the header's simulation and the driving source's declaration"
                        .into(),
                );
            }
            continue;
        };
        for rec in g.records() {
            if let Some(i) = item(rec) {
                r.push(i);
            }
        }
        r.end_group();
        if r.stopped() {
            break;
        }
    }
    let mut report = rep
        .map(Replay::finish)
        .ok_or("no simulation to replay (no simulation header or no frames)")?;
    if report.identical() {
        let (end, _) = groups.ending();
        report.error = match (end.error, end.session_end) {
            (Some(e), _) => Some(format!(
                "reading stopped: {e} (records after it were not compared; `kf-cli recover` \
                 a dead session, then replay its directory)"
            )),
            (None, None) => Some(
                "no terminal session_end: the log is incomplete (`kf-cli recover` the \
                 session, then replay its directory)"
                    .into(),
            ),
            (None, Some(_)) => None,
        };
    }
    let h = header.as_ref();
    Ok(Replayed {
        report,
        recorded_by: h
            .and_then(|h| h.rest.get("app"))
            .cloned()
            .unwrap_or_default(),
        gain_engine: h
            .and_then(|h| h.sensitivity.as_ref())
            .and_then(|s| s.rest.get("directional_gain"))
            .and_then(|g| g.get("engine"))
            .cloned()
            .unwrap_or_default(),
    })
}

fn attempt_json(a: &Attempt) -> Value {
    json!({
        "ordinal": a.ordinal,
        "trial": a.trial,
        "outcome": a.outcome,
        "start_tick": a.start_tick,
        "duration_ns": a.duration_ns,
        "shots": a.shots,
        "hits": a.hits,
        "misses": a.misses(),
        "kills": a.kills,
        "timeouts": a.timeouts,
        "spawned": a.spawned,
        "shots_without_target": a.shots_without_target,
        "accuracy": a.accuracy(),
        "kills_per_s": a.kills_per_s(),
        "mean_ttk_ns": a.mean_ttk_ns(),
        "mean_ang_err_rad": a.mean_ang_err_rad(),
        "score": a.score(),
    })
}

fn record_json(r: &Option<SimRecord>) -> Value {
    match r {
        None => Value::Null,
        Some(r) => Value::String(match r {
            SimRecord::Camera(SimCamera { tick, .. })
            | SimRecord::Target(SimTarget { tick, .. })
            | SimRecord::Shot(SimShot { tick, .. }) => format!("tick {tick}: {r:?}"),
            other => format!("{other:?}"),
        }),
    }
}

/// The report as JSON (`kf-cli replay` output).
pub fn report_json(x: &Replayed) -> Value {
    let r = &x.report;
    let mismatch = r.mismatch.as_ref().map(|m: &Mismatch| {
        json!({
            "kind": m.kind,
            "index": m.index,
            "logged": record_json(&m.logged),
            "replayed": record_json(&m.replayed),
        })
    });
    json!({
        "identical": r.identical(),
        "frames": r.frames,
        "compared": {
            "sim_control": r.compared[0],
            "sim_camera": r.compared[1],
            "sim_target": r.compared[2],
            "sim_shot": r.compared[3],
        },
        "mismatch": mismatch,
        "error": r.error,
        "undrained_events": r.undrained,
        "state_hash": format!("{:#018x}", r.state_hash),
        "scoring": {"model": "kf.flick.v1", "scoring_version": 1},
        "recorded_by": x.recorded_by,
        "gain_engine": x.gain_engine,
        "replayed_by": {"name": "kf-cli", "version": env!("CARGO_PKG_VERSION")},
        "attempts": r.attempts.iter().map(attempt_json).collect::<Vec<_>>(),
        "running_attempt": r.current.as_ref().map(attempt_json),
    })
}
