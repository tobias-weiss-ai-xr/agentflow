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
