//! The simulation and its frame procedure (ARCHITECTURE.md §4,
//! DATA_SPECIFICATION.md §2.1, §2.3, §6.4, §9.1). One function,
//! [`Sim::run_frame`], runs a frame live and in replay; the only
//! difference is where the frame's tick count comes from.
//!
//! **Time.** Tick k ends at the instant (k+1)·p (p = 1e9 / sim_hz ns); its
//! work (time limit, timeouts, spawns) happens at that instant, so the
//! simulation time after n ticks is n·p. While running, the clock keeps an
//! anchor (session time up to which elapsed time is accounted) and the
//! sub-tick remainder `acc` (elapsed but not yet stepped, < p). A running
//! frame adds `cutoff − anchor`; the live tick count is ⌊(acc + elapsed)/p⌋
//! (capped at `max_catch_up_ticks`, excess dropped); replay uses the
//! recorded count and updates the clock the same way. Paused frames step
//! no ticks; `pause` drops the anchor, `resume` sets it to the control's
//! `t_ns`.
//!
//! **Events inside a frame.** Each event has the simulation time
//! τ = first·p + acc + (t_ns − anchor) (t_ns clamped to [anchor, cutoff],
//! τ kept below the instant after the frame's last tick). Before an event
//! is applied, the frame's ticks whose instants are ≤ τ are stepped, so an
//! event applies at tick ⌊τ/p⌋ and without pauses or capped catch-up the
//! simulation does not depend on how events were batched into frames.

use crate::camera::{Camera, CameraError};
use crate::flick::{FlickConfig, FlickGen};
use crate::geom::{self, V3};
use crate::hash::Fnv;
use crate::rng::Xoshiro256pp;
use crate::score::{Attempt, Outcome};
use kf_core::input::{button, button_state, event_flags};
use kf_core::records::{
    control_action as act, control_phase, frame_flags, target_event, target_shape,
};
use kf_core::{
    Drain, Frame, InputEvent, LinearSensitivity, Nanos, SimCamera, SimControl, SimShot, SimTarget,
    SourceId, ValueScale,
};

/// Highest `sim_hz`: a recorded frame may claim at most `sim_hz` ticks, and
/// replay time is proportional to the ticks a log claims.
pub const MAX_SIM_HZ: u32 = 10_000;

/// What drives a simulation; immutable for a session (DATA_SPECIFICATION.md
/// §2.3 item 6).
#[derive(Clone, Debug, PartialEq)]
pub struct SimConfig {
    /// Divides 1e9 (header `simulation.sim_hz`).
    pub sim_hz: u32,
    /// Header `simulation.rng.seed` (xoshiro256++ seeded by splitmix64).
    pub seed: u64,
    pub sensitivity: LinearSensitivity,
    /// The driving source's `value_scale` (switches keep it).
    pub value_scale: ValueScale,
    /// Directional gain (only for `units = counts`, `value_scale = 1`).
    pub gain: Option<kf_dirgain::Profile>,
    /// Header `simulation.input_source`.
    pub driving_source: SourceId,
    /// Live clock only: the most ticks one frame steps (at most `sim_hz`,
    /// one second); the rest of a long stall is dropped (`frame.flags`
    /// CATCH_UP_CAPPED). Not recorded; replay uses the recorded tick counts.
    pub max_catch_up_ticks: u32,
    pub scenario: Scenario,
}

impl SimConfig {
    /// Defaults for everything but the sensitivity and source: 1000 Hz,
    /// catch-up capped at 250 ticks, the default static flick.
    pub fn new(sensitivity: LinearSensitivity, value_scale: ValueScale, seed: u64) -> SimConfig {
        SimConfig {
            sim_hz: 1000,
            seed,
            sensitivity,
            value_scale,
            gain: None,
            driving_source: 0,
            max_catch_up_ticks: 250,
            scenario: Scenario::StaticFlick(FlickConfig::default()),
        }
    }
}

/// The task the simulation runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Scenario {
    StaticFlick(FlickConfig),
}

/// A configuration the simulation cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// `sim_hz` must divide 1e9 and be at most [`MAX_SIM_HZ`].
    SimHz(u32),
    Camera(CameraError),
    Scenario(&'static str),
}

/// A non-input state change requested for a frame (becomes a `sim_control`
/// record when applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Control {
    pub t_ns: Nanos,
    /// A `sim_control.action` code.
    pub action: u8,
    /// A `sim_control.reason` code.
    pub reason: u16,
    /// `switch_source` only.
    pub new_source: Option<SourceId>,
}

/// A frame to run. `ticks: None` = live (the simulation's clock decides);
/// `Some(n)` = replay (the recorded `frame.ticks`, authoritative).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameIn {
    pub frame: u64,
    pub t_start_ns: Nanos,
    pub t_input_cutoff_ns: Nanos,
    pub ticks: Option<u32>,
}

/// Everything a frame produces: the records of its frame message
/// (DATA_SPECIFICATION.md §5.2) except annotations and frame timings.
#[derive(Clone, Debug, PartialEq)]
pub enum SimRecord {
    Frame(Frame),
    Drain(Drain),
    Control(SimControl),
    Camera(SimCamera),
    Target(SimTarget),
    Shot(SimShot),
}

/// A frame the simulation refuses (live: a caller bug; replay: the log is
/// not replayable). Control and frame checks happen before any state
/// changes; an event error leaves the frame partly applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Frame numbers must be consecutive.
    FrameNumber { expected: u64, got: u64 },
    /// A recorded frame steps ticks while the simulation is paused.
    TicksWhilePaused { ticks: u32 },
    /// A recorded frame steps more than `sim_hz` ticks (one second; the live
    /// clock never does).
    TooManyTicks { ticks: u32 },
    /// An unknown `sim_control.action`.
    UnknownAction(u8),
    /// `switch_source` while running, or without a new source.
    BadSwitch,
    /// An event of a source other than the driving one.
    WrongSource { expected: SourceId, got: SourceId },
    /// A gap annotation passed as an event.
    GapEvent,
}

/// Trial state; `paused` is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialState {
    /// No trial started yet.
    Idle,
    Running {
        trial: u32,
        start_tick: u64,
    },
    /// The last attempt ended (complete, aborted or restarted-into).
    Done,
    /// `end` was applied: no trial starts again and input is ignored
    /// (pause, resume and source switches still apply).
    Ended,
}

/// A live target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub id: u32,
    /// Centre at the spawn instant.
    pub pos_m: V3,
    /// Constant velocity (static flick: zero).
    pub vel_m_s: V3,
    pub radius_m: f64,
    pub spawn_tick: u64,
    /// Lifetime end (the target times out in this tick's work).
    pub despawn_tick: Option<u64>,
}

impl Target {
    /// Centre at simulation time `tau_ns`, clamped to the target's life
    /// [spawn instant, despawn instant]: `pos + vel × (Δt_ns / 1e9)`.
    pub fn pos_at(&self, tau_ns: i128, period: i128) -> V3 {
        let spawn = (i128::from(self.spawn_tick) + 1) * period;
        let mut t = if tau_ns < spawn { spawn } else { tau_ns };
        if let Some(d) = self.despawn_tick {
            let end = (i128::from(d) + 1) * period;
            if t > end {
                t = end;
            }
        }
        let dt_s = (t - spawn) as f64 / 1e9;
        geom::add(self.pos_m, geom::scale(self.vel_m_s, dt_s))
    }
}

pub struct Sim {
    cfg: SimConfig,
    flick: FlickConfig,
    period: i128,
    lifetime_ticks: Option<u64>,
    time_limit_ticks: Option<u64>,
    camera: Camera,
    driving: SourceId,
    next_tick: u64,
    last_frame: Option<u64>,
    paused: bool,
    anchor: Option<i64>,
    acc_ns: i64,
    last_camera: (i64, i64),
    state: TrialState,
    trials_started: u32,
    /// Generator of trial `trials_started` (the next new trial) and of the
    /// latest started one (restarts reuse it): one jump per new trial.
    next_trial_rng: Xoshiro256pp,
    last_trial_rng: Xoshiro256pp,
    stimuli: Option<FlickGen>,
    targets: Vec<Target>,
    schedule: Vec<u64>,
    scheduled: u32,
    resolved: u32,
    next_target_id: u32,
    attempt: Option<Attempt>,
    attempts: Vec<Attempt>,
    hash: Fnv,
}

/// ⌈ms × hz / 1000⌉ ticks, `None` for 0 (unlimited).
fn ms_to_ticks(ms: u32, hz: u32) -> Option<u64> {
    (ms > 0).then(|| (u64::from(ms) * u64::from(hz)).div_ceil(1000))
}

impl Sim {
    pub fn new(cfg: SimConfig) -> Result<Sim, ConfigError> {
        if cfg.sim_hz == 0 || cfg.sim_hz > MAX_SIM_HZ || 1_000_000_000 % cfg.sim_hz != 0 {
            return Err(ConfigError::SimHz(cfg.sim_hz));
        }
        let camera =
            Camera::new(cfg.sensitivity, cfg.value_scale, cfg.gain).map_err(ConfigError::Camera)?;
        let Scenario::StaticFlick(flick) = cfg.scenario;
        flick.validate().map_err(ConfigError::Scenario)?;
        let cap = flick.concurrent as usize;
        Ok(Sim {
            period: i128::from(1_000_000_000 / cfg.sim_hz),
            lifetime_ticks: ms_to_ticks(flick.lifetime_ms, cfg.sim_hz),
            time_limit_ticks: ms_to_ticks(flick.time_limit_ms, cfg.sim_hz),
            driving: cfg.driving_source,
            camera,
            next_tick: 0,
            last_frame: None,
            paused: false,
            anchor: Some(0),
            acc_ns: 0,
            last_camera: (0, 0),
            state: TrialState::Idle,
            trials_started: 0,
            next_trial_rng: Xoshiro256pp::from_seed(cfg.seed),
            last_trial_rng: Xoshiro256pp::from_seed(cfg.seed),
            stimuli: None,
            targets: Vec::with_capacity(cap),
            schedule: Vec::with_capacity(cap),
            scheduled: 0,
            resolved: 0,
            next_target_id: 0,
            attempt: None,
            attempts: Vec::new(),
            hash: Fnv::new(),
            flick,
            cfg,
        })
    }

    pub fn config(&self) -> &SimConfig {
        &self.cfg
    }

    /// The next tick to step (= the next frame's `first_tick`).
    pub fn next_tick(&self) -> u64 {
        self.next_tick
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn driving_source(&self) -> SourceId {
        self.driving
    }

    pub fn trial_state(&self) -> TrialState {
        self.state
    }

    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    /// Finished attempts, in order.
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// The running attempt, if any.
    pub fn current_attempt(&self) -> Option<&Attempt> {
        self.attempt.as_ref()
    }

    /// Chain head over every stepped tick, extended by the current state
    /// (so changes after the last tick count): `fnv(head ‖ state)`.
    pub fn state_hash(&self) -> u64 {
        let mut h = self.hash;
        self.hash_state(&mut h);
        h.0
    }

    /// Runs one frame (DATA_SPECIFICATION.md §9.1): controls `before`
    /// (phase 0), then the driving source's `events` in `seq` order with the
    /// frame's ticks interleaved by event time, the remaining ticks, then
    /// `after` (phase 1). Appends the frame's records to `out` (`frame` and
    /// `drain` last).
    pub fn run_frame<I>(
        &mut self,
        f: FrameIn,
        before: &[Control],
        events: I,
        after: &[Control],
        out: &mut Vec<SimRecord>,
    ) -> Result<(), FrameError>
    where
        I: IntoIterator<Item = InputEvent>,
    {
        if let Some(prev) = self.last_frame {
            let expected = prev.wrapping_add(1);
            if f.frame != expected {
                return Err(FrameError::FrameNumber {
                    expected,
                    got: f.frame,
                });
            }
        }
        // Validate every control (tracking pause state through the lists)
        // and the recorded tick count before changing anything.
        let mut paused = self.paused;
        let mut paused_for_ticks = paused;
        for (i, c) in before.iter().chain(after).enumerate() {
            match c.action {
                act::PAUSE => paused = true,
                act::RESUME => paused = false,
                act::SWITCH_SOURCE if !paused || c.new_source.is_none() => {
                    return Err(FrameError::BadSwitch);
                }
                a if a > act::SWITCH_SOURCE => return Err(FrameError::UnknownAction(a)),
                _ => {}
            }
            if i + 1 == before.len() {
                paused_for_ticks = paused;
            }
        }
        if before.is_empty() {
            paused_for_ticks = self.paused;
        }
        if let Some(t) = f.ticks {
            if paused_for_ticks && t > 0 {
                return Err(FrameError::TicksWhilePaused { ticks: t });
            }
            if t > self.cfg.sim_hz {
                return Err(FrameError::TooManyTicks { ticks: t });
            }
        }

        self.last_frame = Some(f.frame);
        let first = self.next_tick;
        for c in before {
            self.control(c, f.frame, control_phase::BEFORE_DRAIN, out);
        }
        let drain_source = self.driving;

        // Tick count and clock.
        let p = self.period;
        let cutoff = f.t_input_cutoff_ns.0;
        let running_clock = if self.paused { None } else { self.anchor };
        let elapsed = running_clock.map_or(0, |a| cutoff.saturating_sub(a).max(0));
        let (ticks, capped) = match (f.ticks, running_clock) {
            (Some(t), _) => (t, false),
            (None, None) => (0, false),
            (None, Some(_)) => {
                let due = (i128::from(self.acc_ns) + i128::from(elapsed)) / p;
                let cap = self.cfg.max_catch_up_ticks.min(self.cfg.sim_hz);
                if due > i128::from(cap) {
                    (cap, true)
                } else {
                    (due as u32, false)
                }
            }
        };
        let end_tick = first + u64::from(ticks);
        let acc_before = i128::from(self.acc_ns);
        let tau_max = (i128::from(end_tick) + 1) * p - 1;

        // Events, with the frame's ticks interleaved by event time.
        let mut drained: Option<u32> = None;
        for ev in events {
            let (source, seq, t_ns) = match ev {
                InputEvent::Motion(m) => (m.source, m.seq, m.t_ns),
                InputEvent::Button(b) => (b.source, b.seq, b.t_ns),
                InputEvent::Wheel(w) => (w.source, w.seq, w.t_ns),
                InputEvent::Gap(_) => return Err(FrameError::GapEvent),
            };
            if source != self.driving {
                return Err(FrameError::WrongSource {
                    expected: self.driving,
                    got: source,
                });
            }
            drained = Some(seq);
            // Discarded while paused (they still advance the drain).
            let Some(anchor) = running_clock else {
                continue;
            };
            if self.state == TrialState::Ended {
                continue;
            }
            let t = t_ns.0.clamp(anchor, cutoff.max(anchor));
            let mut tau = i128::from(first) * p + acc_before + (i128::from(t) - i128::from(anchor));
            if tau > tau_max {
                tau = tau_max;
            }
            while self.next_tick < end_tick && (i128::from(self.next_tick) + 1) * p <= tau {
                self.step_tick(out);
            }
            match ev {
                InputEvent::Motion(m) if m.flags & event_flags::ABSOLUTE == 0 => {
                    self.camera.apply_motion(m.dx, m.dy, m.t_ns, m.src_ts_ns);
                }
                InputEvent::Button(b)
                    if b.button == button::LEFT
                        && b.state & button_state::DOWN != 0
                        && matches!(self.state, TrialState::Running { .. }) =>
                {
                    self.shoot(tau, b.t_ns, b.seq, out);
                }
                _ => {}
            }
        }
        while self.next_tick < end_tick {
            self.step_tick(out);
        }
        if let Some(anchor) = running_clock {
            let rest = acc_before + i128::from(cutoff.saturating_sub(anchor).max(0))
                - i128::from(ticks) * p;
            self.acc_ns = rest.clamp(0, p - 1) as i64;
            self.anchor = Some(cutoff);
        }

        for c in after {
            self.control(c, f.frame, control_phase::AFTER_DRAIN, out);
        }

        let mut flags = 0;
        if running_clock.is_none() {
            flags |= frame_flags::PAUSED;
        }
        if capped {
            flags |= frame_flags::CATCH_UP_CAPPED;
        }
        out.push(SimRecord::Frame(Frame {
            frame: f.frame,
            t_start_ns: f.t_start_ns,
            t_input_cutoff_ns: f.t_input_cutoff_ns,
            first_tick: first,
            ticks,
            flags,
        }));
        if let Some(last_seq) = drained {
            out.push(SimRecord::Drain(Drain {
                frame: f.frame,
                source: drain_source,
                last_seq,
            }));
        }
        Ok(())
    }

    /// Applies one (validated) control and records it with the tick at
    /// which it takes effect (`first_tick` for phase 0, `first_tick +
    /// ticks` for phase 1).
    fn control(&mut self, c: &Control, frame: u64, phase: u8, out: &mut Vec<SimRecord>) {
        out.push(SimRecord::Control(SimControl {
            tick: self.next_tick,
            t_ns: c.t_ns,
            frame,
            action: c.action,
            phase,
            reason: c.reason,
            new_source: c.new_source,
        }));
        // Simulation time now: stepped ticks plus the sub-tick remainder (≥ the
        // τ of every event already applied).
        let now = i128::from(self.next_tick) * self.period + i128::from(self.acc_ns);
        let ended = self.state == TrialState::Ended;
        match c.action {
            // After `end` nothing starts again; abort/end find no running trial.
            act::START_TRIAL | act::RESTART_TRIAL if ended => {}
            act::START_TRIAL => {
                let trial = self.trials_started;
                self.start(trial, now, Outcome::Aborted, out);
            }
            act::RESTART_TRIAL => {
                let trial = match (self.state, self.attempt, self.attempts.last()) {
                    (TrialState::Running { trial, .. }, ..) => trial,
                    (_, _, Some(a)) => a.trial,
                    _ => self.trials_started,
                };
                self.start(trial, now, Outcome::Restarted, out);
            }
            act::PAUSE if !self.paused => {
                self.paused = true;
                self.anchor = None;
            }
            act::RESUME if self.paused => {
                self.paused = false;
                self.anchor = Some(c.t_ns.0);
            }
            act::ABORT_TRIAL => self.end_trial(Outcome::Aborted, now, out),
            act::END => {
                self.end_trial(Outcome::Ended, now, out);
                self.state = TrialState::Ended;
            }
            act::SWITCH_SOURCE => {
                if let Some(s) = c.new_source {
                    self.driving = s;
                    self.camera.reset_gain_state();
                }
            }
            // pause while paused, resume while running, next_block (the
            // experiment module is M10): no effect.
            _ => {}
        }
    }

    /// Starts trial `trial` at the next tick; a running attempt ends as
    /// `replaced` first.
    fn start(&mut self, trial: u32, now: i128, replaced: Outcome, out: &mut Vec<SimRecord>) {
        self.end_trial(replaced, now, out);
        // Trial i's generator is the seed's stream jumped i times
        // (FlickGen::new); `trial` is always the next new trial or the
        // latest started one.
        let rng = if trial == self.trials_started {
            self.last_trial_rng = self.next_trial_rng;
            self.next_trial_rng.jump();
            self.trials_started += 1;
            self.last_trial_rng
        } else {
            self.last_trial_rng
        };
        let start_tick = self.next_tick;
        self.state = TrialState::Running { trial, start_tick };
        self.stimuli = Some(FlickGen::from_rng(rng, &self.flick));
        let first = self.flick.concurrent.min(self.flick.targets);
        self.schedule.clear();
        self.schedule.extend((0..first).map(|_| start_tick));
        self.scheduled = first;
        self.resolved = 0;
        let ordinal = (self.attempts.len()) as u32;
        self.attempt = Some(Attempt::new(ordinal, trial, start_tick));
    }

    /// Ends the running attempt (if any) at simulation time `tau`: despawns
    /// what is alive (records at the current tick) and files the attempt.
    fn end_trial(&mut self, outcome: Outcome, tau: i128, out: &mut Vec<SimRecord>) {
        let TrialState::Running { start_tick, .. } = self.state else {
            return;
        };
        let tick = self.next_tick;
        let mut alive = core::mem::take(&mut self.targets);
        for t in &alive {
            self.target_record(t, tick, target_event::DESPAWN, tau, out);
        }
        alive.clear();
        self.targets = alive; // keeps its capacity: no allocation
        self.schedule.clear();
        self.state = TrialState::Done;
        if let Some(mut a) = self.attempt.take() {
            let onset = (i128::from(start_tick) + 1) * self.period;
            a.duration_ns = i64::try_from((tau - onset).max(0)).unwrap_or(i64::MAX);
            a.outcome = Some(outcome);
            // A filed attempt never changes again: fold it into the chain
            // once (the running one is hashed with the state every tick).
            hash_attempt(&mut self.hash, &a);
            self.attempts.push(a);
        }
    }

    fn target_record(&self, t: &Target, tick: u64, event: u8, tau: i128, out: &mut Vec<SimRecord>) {
        out.push(SimRecord::Target(SimTarget {
            tick,
            target: t.id,
            event,
            shape: target_shape::SPHERE,
            pos_m: t.pos_at(tau, self.period),
            vel_m_s: t.vel_m_s,
            radius_m: t.radius_m,
        }));
    }

    /// A kill or timeout frees a slot: the next target (if any are left) is
    /// scheduled at `tick`; the trial completes when every target resolved.
    fn resolved_one(&mut self, tick: u64, tau: i128, out: &mut Vec<SimRecord>) {
        self.resolved += 1;
        if self.scheduled < self.flick.targets {
            self.schedule.push(tick);
            self.scheduled += 1;
        }
        if self.resolved == self.flick.targets {
            self.end_trial(Outcome::Complete, tau, out);
        }
    }

    /// One tick k: camera record, then (running trial) time limit,
    /// timeouts, spawns; then the hash chain.
    fn step_tick(&mut self, out: &mut Vec<SimRecord>) {
        let k = self.next_tick;
        let cam = (self.camera.yaw_units(), self.camera.pitch_units());
        if cam != self.last_camera {
            self.last_camera = cam;
            out.push(SimRecord::Camera(SimCamera {
                tick: k,
                yaw_units: cam.0,
                pitch_units: cam.1,
                yaw_rad: self.camera.yaw_rad(),
                pitch_rad: self.camera.pitch_rad(),
            }));
        }
        if let TrialState::Running { start_tick, .. } = self.state {
            let instant = (i128::from(k) + 1) * self.period;
            if self.time_limit_ticks.is_some_and(|d| k >= start_tick + d) {
                self.end_trial(Outcome::Complete, instant, out);
            } else {
                let mut i = 0;
                while i < self.targets.len() {
                    if self.targets[i].despawn_tick == Some(k) {
                        let t = self.targets.remove(i);
                        self.target_record(&t, k, target_event::DESPAWN, instant, out);
                        if let Some(a) = &mut self.attempt {
                            a.timeouts += 1;
                        }
                        self.resolved_one(k, instant, out);
                    } else {
                        i += 1;
                    }
                }
                while matches!(self.state, TrialState::Running { .. })
                    && self.schedule.first().is_some_and(|&s| s <= k)
                {
                    self.schedule.remove(0);
                    self.spawn(k, out);
                }
            }
        }
        self.next_tick = k + 1;
        let mut h = self.hash;
        self.hash_state(&mut h);
        self.hash = h;
    }

    fn spawn(&mut self, k: u64, out: &mut Vec<SimRecord>) {
        let Some(stimuli) = &mut self.stimuli else {
            return;
        };
        let spec = stimuli.next(&self.flick);
        let t = Target {
            id: self.next_target_id,
            pos_m: spec.pos,
            vel_m_s: [0.0; 3],
            radius_m: spec.radius_m,
            spawn_tick: k,
            despawn_tick: self.lifetime_ticks.map(|l| k + l),
        };
        self.next_target_id += 1;
        if let Some(a) = &mut self.attempt {
            a.spawned += 1;
        }
        out.push(SimRecord::Target(SimTarget {
            tick: k,
            target: t.id,
            event: target_event::SPAWN,
            shape: target_shape::SPHERE,
            pos_m: t.pos_m,
            vel_m_s: t.vel_m_s,
            radius_m: t.radius_m,
        }));
        self.targets.push(t);
    }

    /// A shot at simulation time `tau` along the camera's current forward
    /// direction (the camera right after the last applied motion with a
    /// lower seq, DATA_SPECIFICATION.md §2.3 item 5). Hit target: smallest
    /// entry distance (ties: lowest id). Reference target: the hit one, else
    /// the alive target with the smallest angular error (ties: lowest id).
    fn shoot(&mut self, tau: i128, t_ns: Nanos, seq: u32, out: &mut Vec<SimRecord>) {
        let tick = self.next_tick;
        let f = self.camera.forward();
        let eye = [0.0; 3];
        // (index, distance) of the hit and (index, angle) of the nearest target.
        let mut hit: Option<(usize, f64)> = None;
        let mut near: Option<(usize, f64)> = None;
        for (i, t) in self.targets.iter().enumerate() {
            let c = t.pos_at(tau, self.period);
            if let Some(d) = geom::ray_sphere(eye, f, c, t.radius_m)
                && hit.is_none_or(|(_, best)| d < best)
            {
                hit = Some((i, d));
            }
            let a = geom::angle_between(f, c);
            if near.is_none_or(|(_, best)| a < best) {
                near = Some((i, a));
            }
        }
        let reference = hit.or(near).map(|(i, _)| i);
        let (ang_err_rad, d_yaw_rad, d_pitch_rad) = match reference {
            Some(i) => {
                let c = self.targets[i].pos_at(tau, self.period);
                let (dy, dp) =
                    geom::relative_angles(c, self.camera.yaw_rad(), self.camera.pitch_rad());
                (geom::angle_between(f, c), dy, dp)
            }
            None => (0.0, 0.0, 0.0),
        };
        out.push(SimRecord::Shot(SimShot {
            tick,
            t_ns,
            target: reference.map(|i| self.targets[i].id),
            seq,
            ang_err_rad,
            d_yaw_rad,
            d_pitch_rad,
            hit: hit.is_some(),
        }));
        if let Some(a) = &mut self.attempt {
            a.shots += 1;
            match reference {
                Some(_) => a.ang_err_sum_rad += ang_err_rad,
                None => a.shots_without_target += 1,
            }
        }
        if let Some((i, _)) = hit {
            let t = self.targets.remove(i);
            self.target_record(&t, tick, target_event::KILL, tau, out);
            if let Some(a) = &mut self.attempt {
                a.hits += 1;
                a.kills += 1;
                let spawn = (i128::from(t.spawn_tick) + 1) * self.period;
                a.ttk_sum_ns = a
                    .ttk_sum_ns
                    .saturating_add(i64::try_from((tau - spawn).max(0)).unwrap_or(i64::MAX));
            }
            self.resolved_one(tick, tau, out);
        }
    }

    /// Fixed serialisation of the whole state (hash.rs rules).
    fn hash_state(&self, h: &mut Fnv) {
        h.u64(self.next_tick);
        h.opt_u64(self.last_frame);
        h.i64(self.camera.yaw_units());
        h.i64(self.camera.pitch_units());
        match self.camera.gain() {
            None => h.u8(0),
            Some(e) => {
                h.u8(1);
                h.bytes(&e.profile.serialize());
                for v in [
                    e.rem_x_q16_16,
                    e.rem_y_q16_16,
                    e.dir_x_q16_16,
                    e.dir_y_q16_16,
                ] {
                    h.i64(i64::from(v));
                }
                h.u8(e.last_octant);
                h.u32(e.last_scale_q16_16);
                h.u32(e.last_motion_ms);
                h.u16(e.sensor_dpi);
                h.u8(e.clock_known);
                h.u32(e.clock_ms);
            }
        }
        h.u16(self.driving);
        h.bool(self.paused);
        h.opt_i64(self.anchor);
        h.i64(self.acc_ns);
        h.i64(self.last_camera.0);
        h.i64(self.last_camera.1);
        match self.state {
            TrialState::Idle => h.u8(0),
            TrialState::Running { trial, start_tick } => {
                h.u8(1);
                h.u32(trial);
                h.u64(start_tick);
            }
            TrialState::Done => h.u8(2),
            TrialState::Ended => h.u8(3),
        }
        h.u32(self.trials_started);
        match &self.stimuli {
            None => h.u8(0),
            Some(g) => {
                h.u8(1);
                g.hash_into(h);
            }
        }
        h.u64(self.targets.len() as u64);
        for t in &self.targets {
            h.u32(t.id);
            for v in t.pos_m.iter().chain(&t.vel_m_s) {
                h.f64(*v);
            }
            h.f64(t.radius_m);
            h.u64(t.spawn_tick);
            h.opt_u64(t.despawn_tick);
        }
        h.u64(self.schedule.len() as u64);
        for &s in &self.schedule {
            h.u64(s);
        }
        h.u32(self.scheduled);
        h.u32(self.resolved);
        h.u32(self.next_target_id);
        match &self.attempt {
            None => h.u8(0),
            Some(a) => {
                h.u8(1);
                hash_attempt(h, a);
            }
        }
        // Filed attempts were folded into the chain when filed.
        h.u64(self.attempts.len() as u64);
    }
}

/// An attempt's fields (hash.rs rules); the outcome as a tag (0 running, 1
/// complete, 2 aborted, 3 restarted, 4 ended).
fn hash_attempt(h: &mut Fnv, a: &Attempt) {
    h.u32(a.ordinal);
    h.u32(a.trial);
    h.u8(match a.outcome {
        None => 0,
        Some(Outcome::Complete) => 1,
        Some(Outcome::Aborted) => 2,
        Some(Outcome::Restarted) => 3,
        Some(Outcome::Ended) => 4,
    });
    h.u64(a.start_tick);
    h.i64(a.duration_ns);
    for v in [
        a.shots,
        a.hits,
        a.kills,
        a.timeouts,
        a.spawned,
        a.shots_without_target,
    ] {
        h.u64(v);
    }
    h.i64(a.ttk_sum_ns);
    h.f64(a.ang_err_sum_rad);
}
