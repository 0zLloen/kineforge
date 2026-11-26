//! Scoring model `kf.flick.v1`, `scoring_version` 1 (AIMLABS_RESEARCH.md
//! §8): bit-exact metrics from simulation events only, folded per attempt.
//! Re-scoring a log means replaying it.

/// How an attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Outcome {
    /// Every target resolved, or the time limit reached.
    Complete,
    /// `abort_trial`, or a new `start_trial` while it ran.
    Aborted,
    /// `restart_trial` while it ran.
    Restarted,
    /// The session's `end` while it ran.
    Ended,
}

/// One attempt at a trial: a `start_trial` or `restart_trial` and what
/// followed. Times are simulation time (ns); ratios and means are `None`
/// when their denominator is zero, never NaN.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Attempt {
    /// 0, 1, 2, … in session order (restarts get their own ordinal).
    pub ordinal: u32,
    /// Trial index: which target sequence (a restart reuses it).
    pub trial: u32,
    /// `None` while running.
    pub outcome: Option<Outcome>,
    /// Tick at which the trial started (its first targets spawn there).
    pub start_tick: u64,
    /// From the first spawn instant to the end (0 while running).
    pub duration_ns: i64,
    pub shots: u64,
    pub hits: u64,
    pub kills: u64,
    /// Targets that despawned at the end of their lifetime.
    pub timeouts: u64,
    pub spawned: u64,
    /// Shots fired while no target was alive (they count as misses).
    pub shots_without_target: u64,
    /// Σ (kill time − spawn instant), in kill order.
    pub ttk_sum_ns: i64,
    /// Σ angular error over shots that had a reference target, in shot order.
    pub ang_err_sum_rad: f64,
}

impl Attempt {
    pub(crate) fn new(ordinal: u32, trial: u32, start_tick: u64) -> Attempt {
        Attempt {
            ordinal,
            trial,
            outcome: None,
            start_tick,
            duration_ns: 0,
            shots: 0,
            hits: 0,
            kills: 0,
            timeouts: 0,
            spawned: 0,
            shots_without_target: 0,
            ttk_sum_ns: 0,
            ang_err_sum_rad: 0.0,
        }
    }

    pub fn misses(&self) -> u64 {
        self.shots - self.hits
    }

    /// hits / shots.
    pub fn accuracy(&self) -> Option<f64> {
        (self.shots > 0).then(|| self.hits as f64 / self.shots as f64)
    }

    /// (kills × 1e9) / duration_ns.
    pub fn kills_per_s(&self) -> Option<f64> {
        (self.duration_ns > 0).then(|| (self.kills as f64 * 1e9) / self.duration_ns as f64)
    }

    /// ttk_sum_ns / kills.
    pub fn mean_ttk_ns(&self) -> Option<f64> {
        (self.kills > 0).then(|| self.ttk_sum_ns as f64 / self.kills as f64)
    }

    /// Mean angular error over shots with a reference target.
    pub fn mean_ang_err_rad(&self) -> Option<f64> {
        let n = self.shots - self.shots_without_target;
        (n > 0).then(|| self.ang_err_sum_rad / n as f64)
    }

    /// `kf.flick.v1` score, only for complete attempts: 100 × kills/s ×
    /// accuracy, as `⌊100 × kills × hits × 10⁹ / (shots × duration_ns)⌋` in
    /// integers (0 when there were no shots or no time).
    pub fn score(&self) -> Option<u64> {
        if self.outcome != Some(Outcome::Complete) {
            return None;
        }
        let den = u128::from(self.shots) * u128::try_from(self.duration_ns).unwrap_or(0);
        if den == 0 {
            return Some(0);
        }
        let num = 100 * u128::from(self.kills) * u128::from(self.hits) * 1_000_000_000;
        Some(u64::try_from(num / den).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_and_score() {
        let mut a = Attempt::new(0, 0, 10);
        assert_eq!(
            (
                a.accuracy(),
                a.kills_per_s(),
                a.mean_ttk_ns(),
                a.mean_ang_err_rad()
            ),
            (None, None, None, None)
        );
        assert_eq!(a.score(), None, "running attempts have no score");
        a.outcome = Some(Outcome::Complete);
        assert_eq!(a.score(), Some(0));
        a.shots = 4;
        a.hits = 3;
        a.kills = 3;
        a.shots_without_target = 1;
        a.duration_ns = 2_000_000_000;
        a.ttk_sum_ns = 900_000_000;
        a.ang_err_sum_rad = 0.06;
        assert_eq!(a.accuracy(), Some(0.75));
        assert_eq!(a.kills_per_s(), Some(1.5));
        assert_eq!(a.mean_ttk_ns(), Some(300_000_000.0));
        assert_eq!(a.mean_ang_err_rad(), Some(0.06 / 3.0));
        assert_eq!(a.misses(), 1);
        // 100 × 1.5 kills/s × 0.75 = 112.5 → 112.
        assert_eq!(a.score(), Some(112));
        a.outcome = Some(Outcome::Aborted);
        assert_eq!(a.score(), None);
    }
}
