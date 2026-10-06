//! Router: measured worker selection (ADR-12). Replays receipts into
//! per-worker (wins, total) stats; picks free workers by UCB1
//! (`mean + sqrt(2·ln(N+1)/(n+1))`). A strictly higher score always wins;
//! a score tie goes to the worker with the cheaper DECLARED cost basis when
//! the two are comparable (see [`crate::cost`]), otherwise to config order —
//! so fresh state with no declared bases reproduces the old first-free
//! behavior exactly.

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
}

impl Router {
    pub fn from_receipts(receipts: &[Receipt]) -> Router {
        let mut r = Router::default();
        for rec in receipts {
            r.record(&rec.worker, rec.outcome == "merged");
        }
        r
    }

    pub fn record(&mut self, worker: &str, won: bool) {
        let e = self.stats.entry(worker.to_string()).or_insert((0, 0));
        e.1 += 1;
        if won {
            e.0 += 1;
        }
    }

    /// Trust rate (wins ÷ attempts) for a worker with history.
    pub fn trust(&self, worker: &str) -> Option<f64> {
        self.stats
            .get(worker)
            .filter(|(_, n)| *n > 0)
            .map(|(w, n)| *w as f64 / *n as f64)
    }

    /// UCB1 pick among eligible (free) workers. Deterministic. A STRICTLY
    /// higher score always wins: measured reliability must outrank a
    /// declared expense assumption, and cost-per-task is confounded by task
    /// difficulty (the hard work goes to the trusted worker), so a cost
    /// term in the score itself would penalise a worker for being given
    /// the hard tasks and starve it — a self-reinforcing bias. Cost is a
    /// tie-break ONLY: when a candidate's score ties the incumbent within
    /// [`SCORE_TIE_EPSILON`], the CHEAPER of the two (by declared cost
    /// basis, [`cost::compare`]) wins; bases that are not comparable (one
    /// priced and one sized, or either declaring nothing) never trigger a
    /// swap, so config order remains the final tie-break.
    pub fn pick<'a, I: IntoIterator<Item = &'a Worker>>(&self, eligible: I) -> Option<&'a Worker> {
        let n_total: u64 = self.stats.values().map(|(_, n)| n).sum();
        let mut best: Option<(f64, Option<Basis>, &Worker)> = None;
        for w in eligible {
            let (wins, n) = self.stats.get(&w.name).copied().unwrap_or((0, 0));
            let mean = wins as f64 / n.max(1) as f64;
            let explore = (2.0 * (n_total as f64 + 1.0).ln() / (n as f64 + 1.0)).sqrt();
            let score = mean + explore;
            // Declared cost basis, consulted ONLY on a tie.
            let w_basis = cost::basis(w.params_b, w.price_per_mtok_usd);
            match best {
                // First candidate becomes the incumbent; a strictly-greater
                // score still replaces it unconditionally (measured trust
                // outranks a declared expense assumption).
                None => best = Some((score, w_basis, w)),
                Some((s, _, _)) if score > s => best = Some((score, w_basis, w)),
                // Tie within the tolerance: keep whichever is CHEAPER, but
                // only when `compare` says the candidate is strictly less —
                // `None` (incomparable bases) or `Equal` keeps the incumbent,
                // so config order remains the final tie-break and a price is
                // never converted into a parameter count.
                Some((s, s_basis, _))
                    if (score - s).abs() <= SCORE_TIE_EPSILON
                        && cost::compare(w_basis, s_basis) == Some(Ordering::Less) =>
                {
                    best = Some((score, w_basis, w));
                }
                _ => {}
            }
        }
        best.map(|(_, _, w)| w)
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
            r.record("a", false);
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
            r.record("a", true);
            r.record("b", false);
        }
        let pool = [worker("a"), worker("b")];
        assert_eq!(r.pick(pool.iter()).unwrap().name, "a");
    }

    #[test]
    fn ties_break_in_config_order() {
        let mut r = Router::default();
        r.record("x", true);
        r.record("y", true);
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
}
