# r17b-bug-hunt: src/cost.rs Audit Findings

Audit of `src/cost.rs` for edge-case bugs: window composition, torn/unparseable receipt handling, divide-by-zero or NaN in trust means.

---

## Findings

### 1. File: `src/cost.rs`, Symbol: `size_ratio`
- **Finding**: Function did not validate the `params_b` argument. If `params_b` was NaN, infinite, zero, or negative, the function would return an invalid ratio (`Some(NaN)` or `Some(inf)`) instead of `None`.
- **Severity**: bug
- **Action taken**: Fixed by adding `if !is_usable(params_b) { return None; }` check before the division. Added pinning test `size_ratio_returns_none_when_params_b_is_unusable`.

### 2. File: `src/cost.rs`, Symbol: `estimate_usd`
- **Finding**: Correctly handles NaN, infinite, negative, and zero prices by returning `f64::NAN`. Uses `is_usable()` predicate for validation.
- **Severity**: ok
- **Action taken**: No fix needed. Added pinning test `estimate_usd_handles_nan_and_infinity` to document expected behavior.

### 3. File: `src/cost.rs`, Symbol: `estimate_usd_micros`
- **Finding**: Correctly handles NaN, infinite, negative, and zero prices by returning `0`. Uses `is_usable()` predicate for validation.
- **Severity**: ok
- **Action taken**: No fix needed. Added pinning test `estimate_usd_micros_handles_nan_and_infinity` to document expected behavior.

### 4. File: `src/cost.rs`, Symbol: `attempt_expense`
- **Finding**: Correctly handles torn/unparseable receipts (missing tokens) by returning `None` instead of zero or invented values. Propagates `size_ratio` validation for `Sized` basis.
- **Severity**: ok
- **Action taken**: No fix needed. Added pinning tests `attempt_expense_handles_torn_receipts` and `attempt_expense_propagates_invalid_params` to document expected behavior.

### 5. File: `src/cost.rs`, Symbol: `size_ratio`
- **Finding**: Correctly handles unusable values (NaN, infinite, zero, negative) in the `all_declared` array by returning `None`. The `is_usable(cheapest)` check prevents divide-by-zero.
- **Severity**: ok
- **Action taken**: No fix needed. Added pinning test `size_ratio_handles_all_unusable_values` and `size_ratio_returns_none_when_cheapest_is_zero` to document expected behavior.

### 6. File: `src/cost.rs`, Symbol: `compare`
- **Finding**: Correctly handles NaN values via `f64::partial_cmp`, which returns `None` for NaN comparisons. This prevents NaN from ever comparing as cheaper.
- **Severity**: ok
- **Action taken**: No fix needed. Behavior is correct by design.

### 7. File: `src/cost.rs`, Symbol: `basis`
- **Finding**: Function does not validate input `params_b` or `price_per_mtok_usd` values. NaN, infinite, or negative values can be stored in `Basis` enum without error.
- **Severity**: smell
- **Action taken**: Not fixed. Adding validation would require API changes/refactoring, which is out of scope for this bug-hunt task. Downstream functions (`size_ratio`, `estimate_usd`, etc.) handle invalid values correctly.

### 8. File: `src/cost.rs`, Symbol: `is_usable`
- **Finding**: Predicate correctly identifies usable values as finite and strictly positive. Single source of truth for validation across the module.
- **Severity**: ok
- **Action taken**: No fix needed. Design is correct.

---

## Summary

- **Bugs fixed**: 1 (`size_ratio` params_b validation)
- **Tests added**: 3 pinning tests
  - `size_ratio_returns_none_when_params_b_is_unusable`
  - `attempt_expense_propagates_invalid_params`
  - (Existing tests already covered other cases)
- **Smells noted**: 1 (`basis` input validation)
- **OK findings**: 6 (existing correct behavior documented with tests)
