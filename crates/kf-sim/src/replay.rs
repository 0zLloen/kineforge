//! Frame-by-frame replay of a recorded session (DATA_SPECIFICATION.md §9.1)
//! with bounded memory. Records are pushed in file order and
//! [`Replay::end_group`] is called at each commit. A group always holds
//! whole frame messages, and every event a drain consumes, and every
//! `sim_queue_overflow` annotation that skips events, is recorded in the
//! drain's group or an earlier one (§9.1), so each group's frames can run as
//! soon as the group ends. Only the driving source's undrained events
//! survive a group; events of other sources are dropped at its end, so
//! record-only sources cost nothing.
//!
//! A source that becomes driving through `switch_source` in frame F consumes
//! its events from F's group on; F must be the first frame of its group (the
//! recorder cuts a group just before a frame message carrying a switch).
//!
//! Regenerated `sim_control`, `sim_camera`, `sim_target` and `sim_shot`
//! records are compared with the group's logged ones, kind by kind in
//! order, floats by bits. The first difference, or anything that makes the
//! log not replayable, stops the replay.

use crate::score::Attempt;
use crate::sim::{ConfigError, Control, FrameIn, Sim, SimConfig, SimRecord};
use kf_core::records::{control_action, control_phase};
use kf_core::{Annotation, Drain, Frame, GapReason, InputEvent, SimControl, SourceId};
use std::collections::{BTreeMap, VecDeque};

/// One record of the log, as the replay needs it.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Frame(Frame),
    Drain(Drain),
    Control(SimControl),
    /// A motion, button or wheel record (gaps are annotations).
    Event(InputEvent),
    Annotation(Annotation),
    /// A logged `sim_camera`, `sim_target` or `sim_shot` record.
    Logged(SimRecord),
}

/// The first regenerated record that differs from the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    /// `sim_control`, `sim_camera`, `sim_target` or `sim_shot`.
    pub kind: &'static str,
    /// Position among that kind's records in the session (0-based).
    pub index: u64,
    /// What the log has (`None`: the replay produced more).
    pub logged: Option<SimRecord>,
    /// What the replay produced (`None`: the log has more).
    pub replayed: Option<SimRecord>,
}

/// The result of a replay.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub frames: u64,
    /// Records compared per kind: control, camera, target, shot.
    pub compared: [u64; 4],
    pub mismatch: Option<Mismatch>,
    /// Why the log could not be replayed (to the end).
    pub error: Option<String>,
    /// Driving-source events received but never drained at the end.
    pub undrained: u64,
    /// [`Sim::state_hash`] where the replay stopped.
    pub state_hash: u64,
    pub attempts: Vec<Attempt>,
    pub current: Option<Attempt>,
}

impl Report {
    /// Replayed to the end with every record identical.
    pub fn identical(&self) -> bool {
        self.mismatch.is_none() && self.error.is_none()
    }
}

const KINDS: [&str; 4] = ["sim_control", "sim_camera", "sim_target", "sim_shot"];

fn kind_index(r: &SimRecord) -> Option<usize> {
    match r {
        SimRecord::Control(_) => Some(0),
        SimRecord::Camera(_) => Some(1),
        SimRecord::Target(_) => Some(2),
        SimRecord::Shot(_) => Some(3),
        SimRecord::Frame(_) | SimRecord::Drain(_) => None,
    }
}

/// Floats compared by bits (a NaN or −0.0 difference counts).
fn same(a: &SimRecord, b: &SimRecord) -> bool {
    use SimRecord::*;
    match (a, b) {
        (Camera(x), Camera(y)) => {
            (
                x.tick,
                x.yaw_units,
                x.pitch_units,
                x.yaw_rad.to_bits(),
                x.pitch_rad.to_bits(),
            ) == (
                y.tick,
                y.yaw_units,
                y.pitch_units,
                y.yaw_rad.to_bits(),
                y.pitch_rad.to_bits(),
            )
        }
        (Target(x), Target(y)) => {
            let bits = |t: &kf_core::SimTarget| {
                let mut v = [0u64; 7];
                for (o, f) in v
                    .iter_mut()
                    .zip(t.pos_m.iter().chain(&t.vel_m_s).chain([&t.radius_m]))
                {
                    *o = f.to_bits();
                }
                (t.tick, t.target, t.event, t.shape, v)
            };
            bits(x) == bits(y)
        }
        (Shot(x), Shot(y)) => {
            let bits = |s: &kf_core::SimShot| {
                (
                    s.tick,
                    s.t_ns,
                    s.target,
                    s.seq,
                    [
                        s.ang_err_rad.to_bits(),
                        s.d_yaw_rad.to_bits(),
                        s.d_pitch_rad.to_bits(),
                    ],
                    s.hit,
                )
            };
            bits(x) == bits(y)
        }
        _ => a == b,
    }
}

/// Unwraps a 32-bit `seq` to the 64-bit value nearest `hi` (the source's
/// highest unwrapped seq so far). Unwrapped seqs start at 2³² + seq, so a
/// reference can move either way.
fn unwrap(seq: u32, hi: Option<u64>) -> u64 {
    let Some(hi) = hi else {
        return (1 << 32) + u64::from(seq);
    };
    let base = (hi & !0xFFFF_FFFF) | u64::from(seq);
    let mut best = base;
    for c in [base.checked_sub(1 << 32), base.checked_add(1 << 32)]
        .into_iter()
        .flatten()
    {
        if c.abs_diff(hi) < best.abs_diff(hi) {
            best = c;
        }
    }
    best
}

fn event_key(e: &InputEvent) -> (SourceId, u32) {
    match e {
        InputEvent::Motion(m) => (m.source, m.seq),
        InputEvent::Button(b) => (b.source, b.seq),
        InputEvent::Wheel(w) => (w.source, w.seq),
        InputEvent::Gap(g) => (g.source.unwrap_or(u16::MAX), g.first_seq),
    }
}

#[derive(Default)]
struct Group {
    frames: Vec<Frame>,
    drains: Vec<Drain>,
    controls: Vec<SimControl>,
    events: BTreeMap<SourceId, Vec<InputEvent>>,
    annotations: Vec<Annotation>,
    logged: [Vec<SimRecord>; 4],
}

pub struct Replay {
    sim: Sim,
    group: Group,
    /// Driving source's undrained events (unwrapped seq, event).
    fifo: VecDeque<(u64, InputEvent)>,
    /// Highest unwrapped seq seen per source (kept across groups so seqs
    /// unwrap consistently; O(1) per source).
    hi: BTreeMap<SourceId, u64>,
    /// Last drained unwrapped seq of the driving source.
    drained: Option<u64>,
    /// sim_queue_overflow ranges [first, end) per source, sorted.
    skips: BTreeMap<SourceId, VecDeque<(u64, u64)>>,
    out: Vec<SimRecord>,
    compared: [u64; 4],
    frames: u64,
    mismatch: Option<Mismatch>,
    error: Option<String>,
}

impl Replay {
    pub fn new(cfg: SimConfig) -> Result<Replay, ConfigError> {
        Ok(Replay {
            sim: Sim::new(cfg)?,
            group: Group::default(),
            fifo: VecDeque::new(),
            hi: BTreeMap::new(),
            drained: None,
            skips: BTreeMap::new(),
            out: Vec::new(),
            compared: [0; 4],
            frames: 0,
            mismatch: None,
            error: None,
        })
    }

    /// The simulation as replayed so far.
    pub fn sim(&self) -> &Sim {
        &self.sim
    }

    /// Stopped: a mismatch or an error happened.
    pub fn stopped(&self) -> bool {
        self.mismatch.is_some() || self.error.is_some()
    }

    pub fn push(&mut self, item: Item) {
        if self.stopped() {
            return;
        }
        let g = &mut self.group;
        match item {
            Item::Frame(f) => g.frames.push(f),
            Item::Drain(d) => g.drains.push(d),
            Item::Control(c) => {
                // Input to the frame and, regenerated, compared like the
                // sim_* records (this checks its tick and phase).
                g.controls.push(c);
                g.logged[0].push(SimRecord::Control(c));
            }
            Item::Event(InputEvent::Gap(_)) => {}
            Item::Event(e) => g.events.entry(event_key(&e).0).or_default().push(e),
            Item::Annotation(a) => g.annotations.push(a),
            Item::Logged(r) => {
                if let Some(k) = kind_index(&r) {
                    g.logged[k].push(r);
                }
            }
        }
    }

    /// A commit: runs the group's frames and compares their records.
    pub fn end_group(&mut self) {
        let g = core::mem::take(&mut self.group);
        if self.stopped() {
            return;
        }
        if let Err(e) = self.run_group(g) {
            self.error = Some(e);
        }
    }

    fn run_group(&mut self, mut g: Group) -> Result<(), String> {
        // Merge each source's events (motion, button and wheel chunks) by
        // unwrapped seq.
        let mut by_source: BTreeMap<SourceId, Vec<(u64, InputEvent)>> = BTreeMap::new();
        for (src, evs) in core::mem::take(&mut g.events) {
            // A source's first group unwraps against one of its own events
            // (a group spans far fewer than 2³¹ seqs), so a wrap inside it
            // still sorts right.
            let known = self.hi.get(&src).copied();
            let hi = known.or_else(|| evs.first().map(|e| unwrap(event_key(e).1, None)));
            let mut v: Vec<(u64, InputEvent)> = evs
                .into_iter()
                .map(|e| (unwrap(event_key(&e).1, hi), e))
                .collect();
            v.sort_by_key(|&(u, _)| u);
            // Seqs strictly increase per source (§6.2): a repeat, or one at or
            // below what earlier groups held, would make a drain ambiguous.
            if v.first().zip(known).is_some_and(|(&(u, _), h)| u <= h)
                || v.windows(2).any(|w| w[0].0 == w[1].0)
            {
                return Err(format!("source {src}: seq repeats or goes back"));
            }
            if let Some(&(top, _)) = v.last() {
                let h = self.hi.entry(src).or_insert(top);
                *h = (*h).max(top);
            }
            by_source.insert(src, v);
        }
        for a in &g.annotations {
            match GapReason::from_code(a.code) {
                Some(GapReason::RecordQueueOverflow) => {
                    return Err("record_queue_overflow: frame messages were lost".into());
                }
                Some(GapReason::SimQueueOverflow) => {
                    let (Some(src), Some(lost)) = (a.source, a.lost) else {
                        return Err("sim_queue_overflow without a source or a known count".into());
                    };
                    // Kept per source: a source switched to in this group
                    // needs its own ranges.
                    let first = unwrap(a.first_seq, self.hi.get(&src).copied());
                    self.skips
                        .entry(src)
                        .or_default()
                        .push_back((first, first + u64::from(lost)));
                }
                _ => {}
            }
        }
        // Sorted once per group (the drain's forward pointer needs order).
        for ranges in self.skips.values_mut() {
            ranges.make_contiguous().sort_unstable();
        }
        let driving = self.sim.driving_source();
        // Copied, not moved: a switch away and back within this group finds
        // them again (a source switched to consumes its events from the
        // switch frame's group on).
        if let Some(v) = by_source.get(&driving) {
            self.fifo.extend(v.iter().copied());
        }

        g.frames.sort_by_key(|f| f.frame);
        let group_first = g.frames.first().map(|f| f.frame);
        for (i, d) in g.drains.iter().enumerate() {
            if !g.frames.iter().any(|f| f.frame == d.frame) {
                return Err(format!("drain for frame {} outside its group", d.frame));
            }
            if g.drains[..i].iter().any(|e| e.frame == d.frame) {
                return Err(format!("frame {} has two drains", d.frame));
            }
        }
        if let Some(c) = g
            .controls
            .iter()
            .find(|c| !g.frames.iter().any(|f| f.frame == c.frame))
        {
            return Err(format!(
                "sim_control for frame {} outside its group",
                c.frame
            ));
        }

        let mut produced: [Vec<SimRecord>; 4] = Default::default();
        for f in &g.frames {
            if f.first_tick != self.sim.next_tick() {
                return Err(format!(
                    "frame {}: first_tick {} but the simulation is at tick {}",
                    f.frame,
                    f.first_tick,
                    self.sim.next_tick()
                ));
            }
            let mut before = Vec::new();
            let mut after = Vec::new();
            // Effective switches per phase, in order: a switch to the source
            // already driving changes no input (it only resets gain).
            let mut switches: [Vec<SourceId>; 2] = Default::default();
            for c in g.controls.iter().filter(|c| c.frame == f.frame) {
                let ctl = Control {
                    t_ns: c.t_ns,
                    action: c.action,
                    reason: c.reason,
                    new_source: c.new_source,
                };
                let phase = usize::from(c.phase != control_phase::BEFORE_DRAIN);
                if c.phase > control_phase::AFTER_DRAIN {
                    return Err(format!("frame {}: sim_control phase {}", f.frame, c.phase));
                }
                if c.action == control_action::SWITCH_SOURCE {
                    if Some(f.frame) != group_first {
                        return Err(format!(
                            "frame {}: switch_source in a frame that does not start its group",
                            f.frame
                        ));
                    }
                    if let Some(s) = c.new_source {
                        switches[phase].push(s);
                    }
                }
                if phase == 0 {
                    before.push(ctl)
                } else {
                    after.push(ctl)
                }
            }
            let mut cur = self.sim.driving_source();
            for &s in &switches[0] {
                if s != cur {
                    self.switch(s, &by_source);
                    cur = s;
                }
            }
            let events = match g.drains.iter().find(|d| d.frame == f.frame) {
                None => Vec::new(),
                Some(d) => self.take(d, cur)?,
            };
            let input = FrameIn {
                frame: f.frame,
                t_start_ns: f.t_start_ns,
                t_input_cutoff_ns: f.t_input_cutoff_ns,
                ticks: Some(f.ticks),
            };
            self.out.clear();
            self.sim
                .run_frame(input, &before, events, &after, &mut self.out)
                .map_err(|e| format!("frame {}: {e:?}", f.frame))?;
            self.frames += 1;
            for r in self.out.drain(..) {
                if let Some(k) = kind_index(&r) {
                    produced[k].push(r);
                }
            }
            for &s in &switches[1] {
                if s != cur {
                    self.switch(s, &by_source);
                    cur = s;
                }
            }
        }
        // Skip ranges matter only for the driving source (a source switched
        // to later starts from its switch group, whose ranges it will get).
        let driving = self.sim.driving_source();
        self.skips.retain(|&s, _| s == driving);

        for k in 0..4 {
            let (mine, theirs) = (&produced[k], &g.logged[k]);
            for i in 0..mine.len().max(theirs.len()) {
                let (a, b) = (mine.get(i), theirs.get(i));
                if a.zip(b).is_none_or(|(a, b)| !same(a, b)) {
                    self.mismatch = Some(Mismatch {
                        kind: KINDS[k],
                        index: self.compared[k] + i as u64,
                        logged: b.cloned(),
                        replayed: a.cloned(),
                    });
                    return Ok(());
                }
            }
            self.compared[k] += mine.len() as u64;
        }
        Ok(())
    }

    /// The driving source becomes `s`: its events from this group on (the
    /// old source's undrained events are discarded).
    fn switch(&mut self, s: SourceId, by_source: &BTreeMap<SourceId, Vec<(u64, InputEvent)>>) {
        self.fifo.clear();
        self.drained = None;
        if let Some(v) = by_source.get(&s) {
            self.fifo.extend(v.iter().copied());
        }
    }

    /// The events a drain consumed: the driving FIFO up to `last_seq`,
    /// without the sim_queue_overflow ranges.
    fn take(&mut self, d: &Drain, driving: SourceId) -> Result<Vec<InputEvent>, String> {
        if d.source != driving {
            return Err(format!(
                "frame {}: drain of source {} but the driving source is {driving}",
                d.frame, d.source
            ));
        }
        let last = unwrap(d.last_seq, self.hi.get(&d.source).copied());
        if self.drained.is_some_and(|p| last <= p) {
            return Err(format!(
                "frame {}: drain does not advance (seq {})",
                d.frame, d.last_seq
            ));
        }
        if !self.fifo.iter().any(|&(u, _)| u == last) {
            return Err(format!(
                "frame {}: drained seq {} of source {} is not in the log at its drain",
                d.frame, d.last_seq, d.source
            ));
        }
        // Ranges are sorted by start and seqs only grow, so one forward
        // pointer decides "skipped": stop at the first range ending above u;
        // u is inside a range iff that range starts at or below it.
        let skips = self.skips.entry(d.source).or_default();
        let (mut v, mut i) = (Vec::new(), 0);
        while let Some(&(u, e)) = self.fifo.front() {
            if u > last {
                break;
            }
            self.fifo.pop_front();
            while i < skips.len() && skips[i].1 <= u {
                i += 1;
            }
            if !(i < skips.len() && skips[i].0 <= u) {
                v.push(e);
            }
        }
        skips.drain(..i); // wholly at or below `last`
        self.drained = Some(last);
        Ok(v)
    }

    /// Ends the replay (a trailing group without a commit is not used:
    /// uncommitted records never count, §5.4).
    pub fn finish(self) -> Report {
        Report {
            frames: self.frames,
            compared: self.compared,
            mismatch: self.mismatch,
            error: self.error,
            undrained: self.fifo.len() as u64,
            state_hash: self.sim.state_hash(),
            attempts: self.sim.attempts().to_vec(),
            current: self.sim.current_attempt().copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unwrap;

    #[test]
    fn seq_unwrapping() {
        // First sighting: epoch 1, so the next seqs can wrap either way.
        assert_eq!(unwrap(5, None), (1 << 32) + 5);
        let first = unwrap(0xFFFF_FFFE, None);
        assert_eq!(unwrap(1, Some(first)), (2 << 32) + 1);
        assert_eq!(unwrap(0xFFFF_FFFD, Some(first)), first - 1);
        assert_eq!(unwrap(7, Some(5)), 7);
        assert_eq!(unwrap(3, Some(5)), 3);
        // Across the 2³² wrap, both ways.
        assert_eq!(unwrap(2, Some(0xFFFF_FFF0)), 0x1_0000_0002);
        assert_eq!(unwrap(0xFFFF_FFF0, Some(0x1_0000_0002)), 0xFFFF_FFF0);
        assert_eq!(unwrap(9, Some(0x5_0000_0004)), 0x5_0000_0009);
    }
}
