//! Pure cost-basis model shared by the cost report and the router.
//!
//! A worker may DECLARE how its expense is measured: either a real price in
//! USD per million tokens (`Basis::Priced`) or the model's parameter count in
//! billions as a size proxy (`Basis::Sized`). This module turns that loose
//! declaration into a comparable notion of expense, and it is the single place
//! the assumption "when no provider reports a price, a bigger model is more
//! expensive" lives.
//!
//! **No invented conversion rate.** A `$/Mtok` price and a parameter count are
//! incommensurable, so [`compare`] refuses to order them and [`Basis`] is an
//! enum rather than one `f64` weight. There is deliberately no function that
//! converts dollars into billions of parameters (or back); inventing such a
//! rate would silently change dispatch. Likewise a model's *name* is never
//! consulted — names like `flash`, `mini`, or `pro` are marketing, and
//! inferring size from them would break the day a provider renames a model.
//!
//! The module is pure: no I/O, no environment, no config access, and no
//! dependencies beyond `std`.

use std::cmp::Ordering;

/// How a worker's expense is measured, when it declares a basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Basis {
    /// Real money: US dollars per million tokens.
    Priced(f64),
    /// A size proxy: the model's parameter count, in billions.
    Sized(f64),
}

/// The declared basis of a worker. A declared PRICE beats a SIZE proxy
/// (real money is not a proxy). `None` when neither is declared.
pub fn basis(params_b: Option<f64>, price_per_mtok_usd: Option<f64>) -> Option<Basis> {
    match (params_b, price_per_mtok_usd) {
        (_, Some(price)) => Some(Basis::Priced(price)),
        (Some(params), None) => Some(Basis::Sized(params)),
        (None, None) => None,
    }
}

/// Order two workers by expense: `Some(Less)` when `a` is the CHEAPER.
///
/// `None` when they are NOT comparable, which happens when (i) either side
/// has no basis, or (ii) the two are different VARIANTS — a `$/Mtok` price
/// and a parameter count are incommensurable and must never be converted
/// into one another. Uses [`f64::partial_cmp`], so a NaN can never compare as
/// cheaper.
pub fn compare(a: Option<Basis>, b: Option<Basis>) -> Option<Ordering> {
    match (a?, b?) {
        (Basis::Priced(x), Basis::Priced(y)) => x.partial_cmp(&y),
        (Basis::Sized(x), Basis::Sized(y)) => x.partial_cmp(&y),
        _ => None,
    }
}

/// Dollars for `tokens` at a price in USD per million tokens.
pub fn estimate_usd(tokens: u64, price_per_mtok_usd: f64) -> f64 {
    tokens as f64 / 1_000_000.0 * price_per_mtok_usd
}

/// A `Sized` worker's expense RATE relative to the CHEAPEST declared `Sized`
/// basis in `all_declared` (the cheapest is exactly `1.0`). `None` when
/// `all_declared` holds no `Sized` basis — never invent a reference.
pub fn size_ratio(params_b: f64, all_declared: &[Basis]) -> Option<f64> {
    let mut cheapest: Option<f64> = None;
    for b in all_declared {
        if let Basis::Sized(params) = b {
            cheapest = Some(match cheapest {
                Some(c) => match params.partial_cmp(&c) {
                    Some(Ordering::Less) => *params,
                    _ => c,
                },
                None => *params,
            });
        }
    }
    Some(params_b / cheapest?)
}

/// True when a declared number is usable: finite and strictly positive.
/// This is the single predicate load-time validation uses.
pub fn is_usable(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// What ONE attempt cost, folded from everything known about it — the
/// cost model's single truthfulness ladder:
///
/// * a provider-REPORTED cost (`cost_micros`) is a measurement of real
///   money and outranks every declared basis for the attempt it belongs
///   to ([`Expense::Usd`] with `estimated: false`);
/// * else a declared price plus recorded tokens yields ESTIMATED dollars
///   (`estimated: true`) — an assumption, marked as one by the report;
/// * else a declared `params_b` yields a relative [`Expense::Ratio`] — a
///   proxy rate, never dollars, and incommensurable with them;
/// * else nothing: unknown is unknown, never zero, never invented.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Expense {
    /// Dollars, as integer MICRO-USD (no float drift in the ledger).
    /// `estimated: false` is measured (the provider's own report);
    /// `estimated: true` is derived from a declared price.
    Usd { micros: u64, estimated: bool },
    /// A relative rate (the `params_b` proxy), never a `$`.
    Ratio(f64),
}

/// Fold one attempt's [`Expense`], most-truthful-first (see [`Expense`]).
/// A declared price needs recorded tokens to become dollars; a `Sized`
/// basis needs them too (its row weight is token-weighted, and a receipt
/// with no tokens is unknown, not zero).
pub fn attempt_expense(
    cost_micros: Option<u64>,
    basis: Option<Basis>,
    tokens: Option<u64>,
    all_declared: &[Basis],
) -> Option<Expense> {
    match cost_micros {
        // Measured money wins outright — even over a declared price on the
        // same worker, and even on a worker that declares nothing at all.
        Some(micros) => Some(Expense::Usd {
            micros,
            estimated: false,
        }),
        None => match basis {
            Some(Basis::Priced(price)) => tokens.map(|t| Expense::Usd {
                micros: estimate_usd_micros(t, price),
                estimated: true,
            }),
            Some(Basis::Sized(params)) => {
                tokens?; // no tokens recorded → unknown, not a rate
                Some(Expense::Ratio(size_ratio(params, all_declared)?))
            }
            None => None,
        },
    }
}

/// Estimated dollars for `tokens` at a price in USD per million tokens,
/// as integer micro-USD: `tokens × price` (the per-million division and
/// the micro multiplication cancel). Saturating, like every cast here.
fn estimate_usd_micros(tokens: u64, price_per_mtok_usd: f64) -> u64 {
    (tokens as f64 * price_per_mtok_usd).round().max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The truthfulness ladder, top to bottom: a MEASUREMENT beats a
    /// DECLARATION, a price needs tokens, a size proxy is never money,
    /// and unknown stays unknown.
    #[test]
    fn a_reported_cost_outranks_every_declared_basis() {
        let all = [Basis::Sized(4.0), Basis::Sized(8.0)];
        // Measured money, verbatim — even on a worker declaring a price
        // (the measurement wins) and even on one declaring nothing.
        for basis in [Some(Basis::Priced(2.0)), Some(Basis::Sized(8.0)), None] {
            assert_eq!(
                attempt_expense(Some(12_300), basis, Some(500_000), &all),
                Some(Expense::Usd {
                    micros: 12_300,
                    estimated: false
                }),
                "measured beats {basis:?}"
            );
        }
        // Measured money needs NO tokens — it is not derived from them.
        assert_eq!(
            attempt_expense(Some(12_300), None, None, &all),
            Some(Expense::Usd {
                micros: 12_300,
                estimated: false
            })
        );
    }

    #[test]
    fn a_declared_price_with_tokens_yields_estimated_dollars() {
        let all = [Basis::Sized(4.0)];
        // 500k tokens at $2/Mtok = $1.0000 = 1,000,000 micro-USD — the same
        // figure `estimate_usd` produces, kept as an integer.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(2.0)), Some(500_000), &all),
            Some(Expense::Usd {
                micros: 1_000_000,
                estimated: true
            })
        );
        assert_eq!(estimate_usd(500_000, 2.0), 1.0);
        // No tokens recorded → no estimate: unknown, never a zero-dollar run.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(2.0)), None, &all),
            None
        );
    }

    #[test]
    fn a_sized_basis_yields_a_ratio_and_needs_tokens() {
        let all = [Basis::Sized(4.0), Basis::Sized(8.0)];
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(8.0)), Some(1), &all),
            Some(Expense::Ratio(2.0))
        );
        // A receipt with no tokens is unknown, not a rate.
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(8.0)), None, &all),
            None
        );
        // No declared Sized basis to relate to: never invent a reference.
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(8.0)), Some(1), &[]),
            None
        );
        // Nothing declared, nothing measured: nothing.
        assert_eq!(attempt_expense(None, None, Some(1), &all), None);
    }
}
