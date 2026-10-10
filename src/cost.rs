//! Pure cost-basis model shared by the cost report and the router.
//!
//! A worker may DECLARE how its expense is measured: either a real price in
//! USD per million tokens (`Basis::Priced`) or the model's parameter count in
//! billions as a size proxy (`Basis::Sized`). This module turns that loose
//! declaration into a comparable notion of expense, and it is the single place
//! the assumption "when no provider reports a price, a bigger model is more
//! expensive" lives.
//!
//! **`Basis::Priced` wins.** Of the two variants, `Priced` is the real-money
//! measure and outranks the `Sized` proxy: when a worker declares both a
//! price and a size, the price takes precedence, and a price is never
//! converted into (or compared against) a parameter count.
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

/// The KIND of measurement a basis is, as a stable lowercase identifier:
/// `"priced"` for real USD per million tokens, `"sized"` for the `params_b`
/// proxy. Machine-readable — a consumer (a probe, a report) can tell money
/// from a ratio without re-deriving the variant match — and allocation-free
/// (`&'static str`). The label names the VARIANT alone, never the magnitude
/// (which is the value the basis carries), and is one of exactly two
/// spellings so it can be compared across versions.
pub fn basis_label(b: &Basis) -> &'static str {
    match b {
        Basis::Priced(_) => "priced",
        Basis::Sized(_) => "sized",
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
/// Returns NaN if the price is not usable (NaN, infinite, or non-positive).
pub fn estimate_usd(tokens: u64, price_per_mtok_usd: f64) -> f64 {
    if !is_usable(price_per_mtok_usd) {
        return f64::NAN;
    }
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
    let cheapest = cheapest?;
    // Avoid divide-by-zero: if cheapest is not usable (zero, NaN, or infinite),
    // we cannot compute a meaningful ratio.
    if !is_usable(cheapest) {
        return None;
    }
    // Also validate params_b: if it's not usable, the ratio would be invalid.
    if !is_usable(params_b) {
        return None;
    }
    Some(params_b / cheapest)
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
            Some(Basis::Priced(price)) => {
                // An unusable declared price (NaN, zero, negative, infinite)
                // yields no estimate — never a silent $0 — mirroring the
                // Sized branch, which rejects unusable params via size_ratio.
                if !is_usable(price) {
                    return None;
                }
                tokens.map(|t| Expense::Usd {
                    micros: estimate_usd_micros(t, price),
                    estimated: true,
                })
            }
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
/// Returns 0 if the price is not usable (NaN, infinite, or non-positive).
fn estimate_usd_micros(tokens: u64, price_per_mtok_usd: f64) -> u64 {
    if !is_usable(price_per_mtok_usd) {
        return 0;
    }
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

    /// Pinning test: size_ratio must not divide by zero.
    #[test]
    fn size_ratio_returns_none_when_cheapest_is_zero() {
        // Zero cheapest value would cause divide-by-zero.
        let all = [Basis::Sized(0.0), Basis::Sized(8.0)];
        assert_eq!(size_ratio(400.0, &all), None);
        // All zero values.
        let all_zero = [Basis::Sized(0.0), Basis::Sized(0.0)];
        assert_eq!(size_ratio(400.0, &all_zero), None);
    }

    /// Pinning test: estimate_usd_micros must handle NaN and infinity.
    #[test]
    fn estimate_usd_micros_handles_nan_and_infinity() {
        // NaN price returns 0.
        assert_eq!(estimate_usd_micros(1_000_000, f64::NAN), 0);
        // Infinity price returns 0.
        assert_eq!(estimate_usd_micros(1_000_000, f64::INFINITY), 0);
        assert_eq!(estimate_usd_micros(1_000_000, f64::NEG_INFINITY), 0);
        // Negative price returns 0.
        assert_eq!(estimate_usd_micros(1_000_000, -1.0), 0);
        // Zero price returns 0.
        assert_eq!(estimate_usd_micros(1_000_000, 0.0), 0);
        // Valid price works normally.
        assert_eq!(estimate_usd_micros(1_000_000, 2.0), 2_000_000);
    }

    /// Pinning test: estimate_usd must handle NaN and infinity.
    #[test]
    fn estimate_usd_handles_nan_and_infinity() {
        // NaN price returns NaN.
        assert!(estimate_usd(1_000_000, f64::NAN).is_nan());
        // Infinity price returns NaN (not a valid usable price).
        assert!(estimate_usd(1_000_000, f64::INFINITY).is_nan());
        assert!(estimate_usd(1_000_000, f64::NEG_INFINITY).is_nan());
        // Negative price returns NaN.
        assert!(estimate_usd(1_000_000, -1.0).is_nan());
        // Zero price returns NaN.
        assert!(estimate_usd(1_000_000, 0.0).is_nan());
        // Valid price works normally.
        assert_eq!(estimate_usd(1_000_000, 2.0), 2.0);
    }

    /// Pinning test: attempt_expense handles torn/unparseable receipts.
    #[test]
    fn attempt_expense_handles_torn_receipts() {
        let all = [Basis::Sized(4.0)];
        // Torn receipt: has basis but no tokens → unknown, not zero.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(2.0)), None, &all),
            None
        );
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(8.0)), None, &all),
            None
        );
        // Valid receipt with tokens works.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(2.0)), Some(500_000), &all),
            Some(Expense::Usd { micros: 1_000_000, estimated: true })
        );
    }

    /// Pinning test: size_ratio handles all unusable values.
    #[test]
    fn size_ratio_handles_all_unusable_values() {
        // All NaN values.
        let all_nan = [Basis::Sized(f64::NAN), Basis::Sized(f64::NAN)];
        assert_eq!(size_ratio(400.0, &all_nan), None);
        // All infinite values.
        let all_inf = [Basis::Sized(f64::INFINITY), Basis::Sized(f64::INFINITY)];
        assert_eq!(size_ratio(400.0, &all_inf), None);
        // All negative values.
        let all_neg = [Basis::Sized(-1.0), Basis::Sized(-8.0)];
        assert_eq!(size_ratio(400.0, &all_neg), None);
        // Mix of unusable values.
        let all_mixed = [Basis::Sized(0.0), Basis::Sized(f64::NAN), Basis::Sized(-1.0)];
        assert_eq!(size_ratio(400.0, &all_mixed), None);
    }

    /// Pinning test: size_ratio must return None when params_b is unusable.
    /// BUG FIX: Previously, if params_b was NaN/infinite/zero/negative, the
    /// function would return an invalid ratio instead of None.
    #[test]
    fn size_ratio_returns_none_when_params_b_is_unusable() {
        let all = [Basis::Sized(4.0)];
        // NaN params_b returns None.
        assert_eq!(size_ratio(f64::NAN, &all), None);
        // Infinite params_b returns None.
        assert_eq!(size_ratio(f64::INFINITY, &all), None);
        assert_eq!(size_ratio(f64::NEG_INFINITY, &all), None);
        // Zero params_b returns None.
        assert_eq!(size_ratio(0.0, &all), None);
        // Negative params_b returns None.
        assert_eq!(size_ratio(-1.0, &all), None);
        // Valid params_b works normally.
        assert_eq!(size_ratio(8.0, &all), Some(2.0));
    }

    /// Pinning test: attempt_expense propagates size_ratio validation.
    /// When a Sized basis has invalid params, attempt_expense returns None.
    #[test]
    fn attempt_expense_propagates_invalid_params() {
        let all = [Basis::Sized(4.0)];
        // NaN params in Sized basis → None.
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(f64::NAN)), Some(100), &all),
            None
        );
        // Zero params in Sized basis → None.
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(0.0)), Some(100), &all),
            None
        );
        // Infinite params in Sized basis → None.
        assert_eq!(
            attempt_expense(None, Some(Basis::Sized(f64::INFINITY)), Some(100), &all),
            None
        );
    }

    /// Pinning test: an unusable declared PRICE (NaN, zero, negative,
    /// infinite) with recorded tokens yields UNKNOWN, not a silent $0
    /// estimate. `estimate_usd_micros` returns 0 for an unusable price, so
    /// without this guard the Priced branch would fabricate a $0 estimated
    /// cost that understates a worker's expense and misleads the router.
    /// This mirrors the Sized branch, which already rejects unusable params.
    #[test]
    fn attempt_expense_unusable_price_with_tokens_is_unknown() {
        let all = [Basis::Sized(4.0)];
        // NaN price → unknown, not a $0 estimate.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(f64::NAN)), Some(500_000), &all),
            None
        );
        // Zero price → unknown, not a $0 estimate.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(0.0)), Some(500_000), &all),
            None
        );
        // Negative price → unknown, not a negative-dollar estimate.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(-1.0)), Some(500_000), &all),
            None
        );
        // Infinite price → unknown.
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(f64::INFINITY)), Some(500_000), &all),
            None
        );
        // An unusable price with NO tokens is also unknown (and was already so
        // because tokens.map on None yields None).
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(f64::NAN)), None, &all),
            None
        );
        // A usable price still estimates normally (regression guard).
        assert_eq!(
            attempt_expense(None, Some(Basis::Priced(2.0)), Some(500_000), &all),
            Some(Expense::Usd {
                micros: 1_000_000,
                estimated: true
            })
        );
    }
}
