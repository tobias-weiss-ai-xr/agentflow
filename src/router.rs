//! Router: measured worker selection (ADR-12). Replays receipts into
//! per-worker (wins, total, mean-duration) stats; picks free workers by
//! UCB1 (`prior-mean + sqrt(2·ln(N+1)/(n+1))`). A strictly higher score always
//! wins; on a score tie the cheaper DECLARED cost basis wins when the two
//! are comparable (see [`crate::cost`]); when the costs do not decide the
//! worker with the strictly lower MEAN duration over its verdict attempts
//! wins; otherwise config order — so fresh state with no declared bases
//! reproduces the old first-free behavior exactly.

use crate::config::Worker;
use crate::cost::{self, Basis};
use crate::state::Receipt;
use std::cmp::Ordering;
use std::collections::HashMap;

/// Tie tolerance for the score comparison: two scores whose difference is
/// at most this are a TIE, not a strict win. This is a numerical tie
/// tolerance — the score is a floating-point sum, so "identical" stats can
/// still differ in the last bits — and it is NOT a tuning knob: widening it
/// would let a cheaper worker displace a measurably better one, which is
/// exactly what this tie-break must never do.
const SCORE_TIE_EPSILON: f64 = 1e-9;

#[derive(Debug, Default)]
pub struct Router {
    /// worker name -> (wins, total attempts)
    stats: HashMap<String, (u64, u64)>,
    /// worker name -> (Σ wall-clock seconds, verdict attempts) — the
    /// running mean duration, held as sum + count so it stays O(1) and
    /// allocation-free on every update.
    durations: HashMap<String, (f64, u64)>,
}

impl Router {
    pub fn from_receipts(receipts: &[Receipt]) -> Router {
        let mut r = Router::default();
        for rec in receipts {
            // The duration is folded in ONLY for verdict attempts: an
            // `interrupted` receipt's `wall_clock_s` is a placeholder (0.0,
            // or time-since-dispatch for a healed one), never a measurement.
            // The trust totals keep their existing semantics — the run loop
            // pre-filters interrupted receipts before replaying (see
            // `run_loop`), exactly as before this statistic existed.
            let dur = rec.counts_as_verdict().then_some(rec.wall_clock_s);
            r.record(&rec.worker, rec.outcome == "merged", dur);
        }
        r
    }

    /// Record one completed attempt. `won` updates the trust stats;
    /// `wall_clock_s` is the attempt's MEASURED duration when there is one.
    /// `None` means "no measurement" (an `interrupted` receipt): the attempt
    /// still counts toward the trust totals, but a placeholder must never
    /// move the mean — no duration is not a (zero) duration.
    pub fn record(&mut self, worker: &str, won: bool, wall_clock_s: Option<f64>) {
        let e = self.stats.entry(worker.to_string()).or_insert((0, 0));
        e.1 += 1;
        if won {
            e.0 += 1;
        }
        if let Some(secs) = wall_clock_s {
            let d = self.durations.entry(worker.to_string()).or_insert((0.0, 0));
            d.0 += secs;
            d.1 += 1;
        }
    }

    /// Trust rate (wins ÷ attempts) for a worker with history.
    pub fn trust(&self, worker: &str) -> Option<f64> {
        self.stats
            .get(worker)
            .filter(|(_, n)| *n > 0)
            .map(|(w, n)| *w as f64 / *n as f64)
    }

    /// Mean wall-clock seconds over the worker's VERDICT attempts
    /// ([`Receipt::counts_as_verdict`]): the statistic the routing
    /// tie-break reads and the cost report's MEAN_S column prints, so the
    /// choice is inspectable. `None` when the worker has no verdict
    /// attempt — a missing duration is never treated as zero.
    pub fn mean_duration_s(&self, worker: &str) -> Option<f64> {
        self.durations
            .get(worker)
            .filter(|(_, n)| *n > 0)
            .map(|(sum, n)| sum / *n as f64)
    }

    /// UCB1 pick among eligible (free) workers. Deterministic. A STRICTLY
    /// higher score always wins: measured reliability must outrank every
    /// declared or incidental assumption. Both remaining tie-breaks — a
    /// DECLARED cost, then MEAN DURATION — are consulted ONLY on a score
    /// tie, never as score terms, for the same reason: cost-per-task and
    /// wall-clock-per-task are both confounded by task difficulty (the
    /// hard work goes to the trusted worker), so either as a score term
    /// would penalise a worker for being given the hard tasks and starve
    /// it — a self-reinforcing bias. UCB1's exploration term still
    /// guarantees an unpicked worker is eventually tried, so no tie-break
    /// can starve one. On a tie within [`SCORE_TIE_EPSILON`] the ordering
    /// is trust > declared price > duration > config order: the CHEAPER of
    /// the two declared bases ([`cost::compare`]) wins when comparable;
    /// when the costs do not decide — bases incomparable or equal — the
    /// worker with the strictly LOWER [`mean_duration_s`] wins; otherwise
    /// config order. A worker with no duration history never displaces
    /// the incumbent, and a missing duration is never treated as zero.
    pub fn pick<'a, I: IntoIterator<Item = &'a Worker>>(&self, eligible: I) -> Option<&'a Worker> {
        let n_total: u64 = self.stats.values().map(|(_, n)| n).sum();
        let mut best: Option<(f64, Option<Basis>, Option<f64>, &Worker)> = None;
        for w in eligible {
            let (wins, n) = self.stats.get(&w.name).copied().unwrap_or((0, 0));
            // Laplace-smoothed trust mean: (wins+1)/(n+2). A worker with a
            // single loss scores 1/3, not 0; a single win 2/3, not 1 — with
            // N=1 the raw rate is 0-or-1 noise. Converges to the empirical
            // rate as n grows (n=100, 50 wins: 51/102 ≈ 0.5). `trust()` keeps
            // returning the RAW rate for the cost report; this smoothed mean
            // is the routing term only.
            let mean = (wins as f64 + 1.0) / (n as f64 + 2.0);
            let explore = (2.0 * (n_total as f64 + 1.0).ln() / (n as f64 + 1.0)).sqrt();
            let score = mean + explore;
            // Declared cost basis and mean duration, consulted ONLY on a tie.
            let w_basis = cost::basis(w.params_b, w.price_per_mtok_usd);
            let w_dur = self.mean_duration_s(&w.name);
            match best {
                // First candidate becomes the incumbent; a strictly-greater
                // score still replaces it unconditionally (measured trust
                // outranks every declared or incidental assumption).
                None => best = Some((score, w_basis, w_dur, w)),
                Some((s, _, _, _)) if score > s => best = Some((score, w_basis, w_dur, w)),
                // Tie within the tolerance: keep whichever is CHEAPER, but
                // only when `compare` says the candidate is strictly less —
                // `None` (incomparable bases) or `Equal` defers to the
                // duration tie-break below, so a price is never converted
                // into a parameter count.
                Some((s, s_basis, s_dur, _)) if (score - s).abs() <= SCORE_TIE_EPSILON => {
                    match cost::compare(w_basis, s_basis) {
                        Some(Ordering::Less) => best = Some((score, w_basis, w_dur, w)),
                        // Costs do not decide: the strictly LOWER mean
                        // duration wins — never above measured trust, never
                        // above a declared price. A missing duration (`None`
                        // on either side) never swaps: no history is not
                        // zero seconds, and a worker with no duration
                        // history never displaces the incumbent — so config
                        // order remains the final tie-break.
                        None | Some(Ordering::Equal) => {
                            if let (Some(cand), Some(inc)) = (w_dur, s_dur) {
                                if cand < inc {
                                    best = Some((score, w_basis, w_dur, w));
                                }
                            }
                        }
                        // The incumbent is cheaper: it stays.
                        Some(Ordering::Greater) => {}
                    }
                }
                _ => {}
            }
        }
        best.map(|(_, _, _, w)| w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(name: &str) -> Worker {
        Worker {
            name: name.to_string(),
            output: "text".into(),
            args: Vec::new(),
            params_b: None,
            price_per_mtok_usd: None,
            ..Default::default()
        }
    }

    fn receipt(worker: &str, outcome: &str) -> Receipt {
        Receipt {
            task: "t".into(),
            attempt: 1,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens: None,
            ts: 0,
            outcome: outcome.into(),
            error: None,
            cost_micros: None,
        }
    }

    #[test]
    fn fresh_state_picks_first_configured_worker() {
        let r = Router::default();
        let pool = [worker("a"), worker("b")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "a");
    }

    #[test]
    fn unexplored_worker_beats_failing_one() {
        let mut r = Router::default();
        for _ in 0..3 {
            r.record("a", false, Some(1.0));
        }
        let pool = [worker("a"), worker("b")];
        assert_eq!(
            r.pick(pool.iter()).unwrap().name,
            "b",
            "explore term dominates 0/3"
        );
    }

    #[test]
    fn reliable_worker_wins_at_equal_counts() {
        let mut r = Router::default();
        for _ in 0..3 {
            r.record("a", true, Some(1.0));
            r.record("b", false, Some(1.0));
        }
        let pool = [worker("a"), worker("b")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "a");
    }

    #[test]
    fn laplace_prior_changes_small_n_ordering() {
        // Pins the Laplace-smoothed mean (wins+1)/(n+2), not the raw rate.
        // a: 3/5 (above the midpoint → prior pulls it DOWN: 4/7 ≈ 0.571 raw 0.6)
        // b: 0/2 (below the midpoint → prior pulls it UP: 1/4 = 0.25 raw 0)
        // n_total = 7:
        //   explore(a) = sqrt(2·ln8/6) ≈ 0.832 → raw 1.432 vs prior 1.403
        //   explore(b) = sqrt(2·ln8/3) ≈ 1.177 → raw 1.177 vs prior 1.427
        // A raw-rate router keeps the high-mean favourite (a); the prior lets
        // the recovering underdog (b) edge out — the exact small-N regime the
        // change targets. This test FAILS with the raw mean.
        let mut r = Router::default();
        for _ in 0..5 {
            r.record("a", true, Some(1.0));
        }
        for _ in 0..2 {
            r.record("a", false, Some(1.0));
        }
        for _ in 0..2 {
            r.record("b", false, Some(1.0));
        }
        let pool = [worker("a"), worker("b")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "b");
    }

    #[test]
    fn trust_stays_raw_while_pick_uses_the_prior() {
        let mut r = Router::default();
        for _ in 0..3 {
            r.record("a", true, Some(1.0));
            r.record("a", false, Some(1.0));
        }
        assert_eq!(r.trust("a"), Some(0.5));
    }

    #[test]
    fn ties_break_in_config_order() {
        let mut r = Router::default();
        // Equal durations on purpose: an equal mean is not "strictly lower",
        // so this score tie must still fall through to config order.
        r.record("x", true, Some(1.0));
        r.record("y", true, Some(1.0));
        let pool = [worker("x"), worker("y")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "x");
    }

    #[test]
    fn replay_and_trust_math() {
        let receipts = vec![
            receipt("w1", "merged"),
            receipt("w1", "failed"),
            receipt("w1", "merged"),
            receipt("w2", "merged"),
        ];
        let r = Router::from_receipts(&receipts);
        assert_eq!(r.trust("w1"), Some(2.0 / 3.0));
        assert_eq!(r.trust("w2"), Some(1.0));
        assert_eq!(r.trust("ghost"), None);
        // w2 (1/1) beats w1 (2/3) at the same total.
        let pool = [worker("w1"), worker("w2")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "w2");
    }

    #[test]
    fn mean_duration_is_over_verdict_attempts_only() {
        let mut r = Router::default();
        r.record("w1", true, Some(2.0));
        r.record("w1", false, Some(6.0));
        assert_eq!(r.mean_duration_s("w1"), Some(4.0));
        assert_eq!(r.mean_duration_s("ghost"), None, "no history → None");
        // A placeholder duration never enters the mean.
        r.record("w1", false, None);
        assert_eq!(
            r.mean_duration_s("w1"),
            Some(4.0),
            "an unmeasured attempt does not move the mean"
        );
        // An interrupted-only worker has no verdict attempt → None, never 0.
        let mut intr = receipt("w2", "interrupted");
        intr.wall_clock_s = 999.0;
        let r2 = Router::from_receipts(&[intr]);
        assert_eq!(r2.mean_duration_s("w2"), None);
        assert_eq!(r2.trust("w2"), Some(0.0), "replay keeps trust semantics");
        // And a mixed history keeps the interrupted placeholder out of the
        // mean: (2.0 + 6.0) / 2, not (2.0 + 6.0 + 999.0) / 3.
        let mut merged = receipt("w1", "merged");
        merged.wall_clock_s = 2.0;
        let mut failed = receipt("w1", "failed");
        failed.wall_clock_s = 6.0;
        let mut lost = receipt("w1", "interrupted");
        lost.wall_clock_s = 999.0;
        let r3 = Router::from_receipts(&[merged, failed, lost]);
        assert_eq!(r3.mean_duration_s("w1"), Some(4.0));
    }

    #[test]
    fn duration_breaks_a_tie_only_after_cost_fails_to() {
        // Tied trust (1/1 each), both bases undeclared (incomparable), so
        // only the duration can decide — the faster worker wins even from
        // second position in the pool.
        let mut r = Router::default();
        r.record("slow", true, Some(10.0));
        r.record("fast", true, Some(2.0));
        let pool = [worker("slow"), worker("fast")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "fast");

        // A cheaper declared price outranks a faster worker: cost is the
        // earlier tie-break, so it decides before duration is consulted.
        let mut cheap = worker("cheap");
        cheap.price_per_mtok_usd = Some(0.5);
        let mut dear = worker("dear");
        dear.price_per_mtok_usd = Some(5.0);
        let pool = [dear, cheap];
        assert_eq!(
            r.pick(pool.iter()).unwrap().name,
            "cheap",
            "the cheaper price wins the tie even though it is the slower worker"
        );

        // And the reverse pool keeps the cheap incumbent: a score-tied
        // candidate with the more expensive comparable basis never
        // displaces it (cost already decided — Greater — so duration is
        // never consulted).
        let mut cheap2 = worker("cheap");
        cheap2.price_per_mtok_usd = Some(0.5);
        let mut dear2 = worker("dear");
        dear2.price_per_mtok_usd = Some(5.0);
        let pool = [cheap2, dear2];
        assert_eq!(
            r.pick(pool.iter()).unwrap().name,
            "cheap",
            "the incumbent cheaper price stays"
        );
    }
}
