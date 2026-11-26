//! EXPERIMENT_PLAN E2 — camera rotation and frame-rate independence. The
//! same recorded input is replayed at simulated frame rates 30–1000 Hz and
//! with random batchings: the camera after every event, the final camera
//! and every simulation record (camera, targets, shots) must be bit
//! identical, with directional gain and the pitch clamp active, and the
//! unit totals must equal the stream applied event by event.

mod common;

use common::*;
use kf_core::InputEvent;
use kf_core::input::{button, button_state, event_flags};
use kf_sim::{Camera, Outcome, SimRecord};

const RATES: [u32; 8] = [30, 60, 144, 165, 240, 360, 500, 1000];

fn bits(r: &[SimRecord]) -> Vec<String> {
    // Debug of f64 is round-trip exact; compare through it so a NaN or a
    // −0.0 would also show up as a difference.
    r.iter().map(|x| format!("{x:?}")).collect()
}

#[test]
fn identical_at_every_frame_rate_and_batching() {
    for v in variants() {
        let stream = record_stream(&v);
        let reference = run_live(&v.cfg, &stream, &cutoffs_at(1000, stream.end_ns), &[]);
        let ref_records = bits(&sim_records(&reference.records));
        let ref_attempts = format!("{:?}", reference.sim.attempts());
        let shots = reference
            .records
            .iter()
            .filter(|r| matches!(r, SimRecord::Shot(s) if s.hit))
            .count();
        assert!(
            shots >= 5,
            "{}: the bot should hit targets ({shots})",
            v.name
        );
        // The corpus must keep exercising each way a trial ends.
        let first = reference.sim.attempts()[0];
        // Shots are exactly the left-button downs applied while the trial
        // ran: right clicks, wheel and absolute packets (all in the corpus)
        // fire none. τ = t_ns here (no pauses); the last shot may be the
        // kill that completed the trial at its end instant.
        let p = 1_000_000_000 / i64::from(v.cfg.sim_hz);
        let end = (first.start_tick as i64 + 1) * p + first.duration_ns;
        let downs: Vec<u32> = stream
            .events
            .iter()
            .filter_map(|e| match e {
                InputEvent::Button(b)
                    if b.button == button::LEFT
                        && b.state & button_state::DOWN != 0
                        && b.t_ns.0 < end =>
                {
                    Some(b.seq)
                }
                _ => None,
            })
            .collect();
        let shot_seqs: Vec<u32> = reference
            .records
            .iter()
            .filter_map(|r| match r {
                SimRecord::Shot(s) => Some(s.seq),
                _ => None,
            })
            .collect();
        assert!(shot_seqs.len() - downs.len() <= 1, "{}", v.name);
        assert_eq!(shot_seqs[..downs.len()], downs[..], "{}", v.name);
        for kind in [0u8, 1, 2] {
            let present = stream.events.iter().any(|e| match (kind, e) {
                (0, InputEvent::Motion(m)) => m.flags & event_flags::ABSOLUTE != 0,
                (1, InputEvent::Wheel(_)) => true,
                (2, InputEvent::Button(b)) => b.button != button::LEFT,
                _ => false,
            });
            assert!(
                present,
                "{}: corpus lacks ignored event kind {kind}",
                v.name
            );
        }
        match v.name {
            "counts-1k" => assert_eq!((first.outcome, first.kills), (Some(Outcome::Complete), 32)),
            "gain-8k" => assert_eq!(first.outcome, Some(Outcome::Ended)),
            _ => {
                assert_eq!(first.outcome, Some(Outcome::Complete));
                assert!(
                    first.timeouts > 0 && first.duration_ns == 7_000_000_000,
                    "{first:?}"
                );
            }
        }

        let mut schedules: Vec<(String, Vec<i64>)> = RATES
            .iter()
            .map(|&fps| (format!("{fps} Hz"), cutoffs_at(fps, stream.end_ns)))
            .collect();
        for seed in 0..3 {
            schedules.push((
                format!("random batching {seed}"),
                cutoffs_random(seed, 300, 45_000, stream.end_ns),
            ));
        }
        for (name, cutoffs) in schedules {
            let run = run_live(&v.cfg, &stream, &cutoffs, &[]);
            let (a, b) = (run.sim.camera(), reference.sim.camera());
            assert_eq!(
                (
                    a.yaw_units(),
                    a.pitch_units(),
                    a.yaw_rad().to_bits(),
                    a.pitch_rad().to_bits()
                ),
                (
                    b.yaw_units(),
                    b.pitch_units(),
                    b.yaw_rad().to_bits(),
                    b.pitch_rad().to_bits()
                ),
                "{} at {name}: final camera differs",
                v.name
            );
            let got = bits(&sim_records(&run.records));
            if let Some(i) =
                (0..got.len().max(ref_records.len())).find(|&i| got.get(i) != ref_records.get(i))
            {
                panic!(
                    "{} at {name}: record {i} differs:\n  got {:?}\n  ref {:?}",
                    v.name,
                    got.get(i),
                    ref_records.get(i)
                );
            }
            assert_eq!(
                format!("{:?}", run.sim.attempts()),
                ref_attempts,
                "{} at {name}",
                v.name
            );
        }
    }
}

/// The camera depends only on the event sequence: applying the stream
/// event by event to a bare camera gives the same totals as the full
/// simulation, and without gain or clamping the totals are plain sums.
#[test]
fn unit_totals_equal_the_stream() {
    for v in variants() {
        let stream = record_stream(&v);
        let run = run_live(&v.cfg, &stream, &cutoffs_at(240, stream.end_ns), &[]);
        let mut cam = Camera::new(v.cfg.sensitivity, v.cfg.value_scale, v.cfg.gain).unwrap();
        let (mut sx, mut sy, mut clamped) = (0i64, 0i64, false);
        for e in &stream.events {
            if let InputEvent::Motion(m) = e
                && m.flags & event_flags::ABSOLUTE == 0
            {
                cam.apply_motion(m.dx, m.dy, m.t_ns, m.src_ts_ns);
                sx += i64::from(m.dx);
                sy += i64::from(m.dy);
                clamped |= cam.pitch_units().abs() == cam.pitch_limit_units();
            }
        }
        let sim_cam = run.sim.camera();
        assert_eq!(
            (cam.yaw_units(), cam.pitch_units()),
            (sim_cam.yaw_units(), sim_cam.pitch_units()),
            "{}",
            v.name
        );
        if v.cfg.gain.is_none() {
            assert_eq!(
                cam.yaw_units(),
                sx,
                "{}: yaw total is the plain sum",
                v.name
            );
            if !clamped {
                assert_eq!(cam.pitch_units(), sy, "{}", v.name);
            }
        }
        assert_eq!(
            clamped, v.slam_pitch,
            "{}: clamp exercised as intended",
            v.name
        );
    }
}
