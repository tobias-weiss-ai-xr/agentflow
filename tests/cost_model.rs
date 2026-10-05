//! Contract tests for the pure cost-basis module [`agentflow::cost`].
//!
//! These pin the decision rules the cost report and the router share: a
//! declared PRICE always wins over a SIZE proxy, the two variants are
//! incommensurable (no invented conversion rate), a size ratio is measured
//! against the cheapest declared `Sized` basis, and a declared number is only
//! usable when it is finite and strictly positive.

use agentflow::cost::{basis, compare, estimate_usd, is_usable, size_ratio, Basis};
use std::cmp::Ordering;

#[test]
fn cost_basis_prefers_a_declared_price_over_a_parameter_count() {
    // Both declared: the real price wins over the size proxy.
    assert_eq!(basis(Some(70.0), Some(0.60)), Some(Basis::Priced(0.60)));
    // Only params: a size proxy.
    assert_eq!(basis(Some(32.0), None), Some(Basis::Sized(32.0)));
    // Only price: real money.
    assert_eq!(basis(None, Some(0.15)), Some(Basis::Priced(0.15)));
    // Neither declared: no basis at all.
    assert_eq!(basis(None, None), None);
}

#[test]
fn incommensurable_bases_are_never_compared() {
    let usd = Some(Basis::Priced(0.60));
    let params = Some(Basis::Sized(70.0));

    // A price and a parameter count are different variants: never comparable,
    // in EITHER direction.
    assert_eq!(compare(usd, params), None);
    assert_eq!(compare(params, usd), None);

    // Same variant orders by expense, cheaper is Less.
    assert_eq!(
        compare(Some(Basis::Priced(0.15)), Some(Basis::Priced(0.60))),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare(Some(Basis::Priced(0.60)), Some(Basis::Priced(0.15))),
        Some(Ordering::Greater)
    );
    assert_eq!(
        compare(Some(Basis::Sized(8.0)), Some(Basis::Sized(70.0))),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare(Some(Basis::Sized(70.0)), Some(Basis::Sized(8.0))),
        Some(Ordering::Greater)
    );
    assert_eq!(
        compare(Some(Basis::Sized(70.0)), Some(Basis::Sized(70.0))),
        Some(Ordering::Equal)
    );

    // A missing basis is not comparable to anything.
    assert_eq!(compare(None, usd), None);
    assert_eq!(compare(usd, None), None);

    // NaN never wins: partial_cmp yields None, not Some(Less).
    assert_eq!(
        compare(Some(Basis::Priced(f64::NAN)), Some(Basis::Priced(0.60))),
        None
    );
    assert_eq!(
        compare(Some(Basis::Priced(0.60)), Some(Basis::Priced(f64::NAN))),
        None
    );
    assert_eq!(
        compare(Some(Basis::Sized(f64::NAN)), Some(Basis::Sized(8.0))),
        None
    );
}

#[test]
fn a_size_ratio_is_measured_against_the_cheapest_declared_worker() {
    let declared = [Basis::Sized(400.0), Basis::Sized(32.0), Basis::Priced(1.0)];

    // 400B is 12.5x the cheapest Sized worker (32B).
    assert_eq!(size_ratio(400.0, &declared), Some(12.5));
    // The cheapest declared Sized basis is exactly 1.0.
    assert_eq!(size_ratio(32.0, &declared), Some(1.0));
    // Declaration order does not matter: the minimum still wins.
    assert_eq!(
        size_ratio(64.0, &[Basis::Sized(32.0), Basis::Sized(400.0)]),
        Some(2.0)
    );
    // A Priced entry does NOT become a reference for a Sized worker.
    assert_eq!(size_ratio(400.0, &[Basis::Priced(0.60)]), None);
    // No Sized basis at all: never invent a reference.
    assert_eq!(size_ratio(400.0, &[]), None);
}

#[test]
fn estimate_usd_and_usability_are_exact() {
    // One million tokens at $0.60/Mtok is exactly $0.60.
    assert_eq!(estimate_usd(1_000_000, 0.60), 0.60);
    // Zero tokens costs nothing, whatever the price.
    assert_eq!(estimate_usd(0, 0.60), 0.0);

    // Usable: finite and strictly positive.
    assert!(is_usable(0.5));
    assert!(is_usable(1.0));
    // Unusable: zero, negative, NaN, and infinities.
    assert!(!is_usable(0.0));
    assert!(!is_usable(-1.0));
    assert!(!is_usable(f64::NAN));
    assert!(!is_usable(f64::INFINITY));
    assert!(!is_usable(f64::NEG_INFINITY));
}
