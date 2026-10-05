//! Router: measured worker selection (ADR-12). Replays receipts into
//! per-worker (wins, total) stats; picks free workers by UCB1
//! (`mean + sqrt(2·ln(N+1)/(n+1))`), ties broken in config order — so fresh
//! state reproduces the old first-free behavior exactly.

use crate::config::Worker;
use crate::state::Receipt;
use std::collections::HashMap;

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

    /// UCB1 pick among eligible (free) workers. Deterministic: first
    /// eligible worker wins ties (config order at the call site).
    pub fn pick<'a, I: IntoIterator<Item = &'a Worker>>(&self, eligible: I) -> Option<&'a Worker> {
        let n_total: u64 = self.stats.values().map(|(_, n)| n).sum();
        let mut best: Option<(f64, &Worker)> = None;
        for w in eligible {
            let (wins, n) = self.stats.get(&w.name).copied().unwrap_or((0, 0));
            let mean = wins as f64 / n.max(1) as f64;
            let explore = (2.0 * (n_total as f64 + 1.0).ln() / (n as f64 + 1.0)).sqrt();
            let score = mean + explore;
            // strictly-greater keeps the FIRST maximum (config order)
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, w));
            }
        }
        best.map(|(_, w)| w)
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
