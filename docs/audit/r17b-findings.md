# r17b-bug-hunt: src/cost.rs Audit Findings

## Findings

1. **File:** `src/cost.rs`, **Symbol:** `size_ratio`, **Finding:** Divide-by-zero when `cheapest` parameter is 0.0 — the function divides `params_b / cheapest` without validating that `cheapest > 0.0`, producing infinity. **Severity:** bug, **Action taken:** Fixed by checking `cheapest.is_finite() && cheapest > 0.0` before division; returns `None` if cheapest is not usable. Added pinning test `size_ratio_returns_none_when_cheapest_is_zero`.

2. **File:** `src/cost.rs`, **Symbol:** `estimate_usd_micros`, **Finding:** NaN/infinity propagation — if `price_per_mtok_usd` is NaN or infinite, the result becomes NaN, and casting NaN to u64 silently produces 0. **Severity:** bug, **Action taken:** Fixed by validating input with `is_usable()` before computation; returns 0 for unusable prices. Added pinning test `estimate_usd_micros_handles_nan_and_infinity`.

3. **File:** `src/cost.rs`, **Symbol:** `estimate_usd`, **Finding:** NaN/infinity propagation — if `price_per_mtok_usd` is NaN or infinite, the result is NaN or infinity without any indication. **Severity:** bug, **Action taken:** Fixed by returning `f64::NAN` explicitly for unusable prices to make the error visible. Added pinning test `estimate_usd_handles_nan_and_infinity`.

4. **File:** `src/cost.rs`, **Symbol:** `basis`, **Finding:** No input validation — accepts any f64 values including NaN, infinity, and negative numbers, which could lead to invalid Basis values propagating through the system. **Severity:** smell, **Action taken:** Not fixed (out of scope for bug-only fixes); documented that callers should validate inputs with `is_usable()` before calling.

5. **File:** `src/cost.rs`, **Symbol:** `attempt_expense`, **Finding:** Torn/unparseable receipt handling — correctly returns `None` when tokens are missing for both priced and sized bases. **Severity:** ok, **Action taken:** No change needed; behavior is correct. Added pinning test `attempt_expense_handles_torn_receipts`.

6. **File:** `src/cost.rs`, **Symbol:** `compare`, **Finding:** NaN handling in ordering — correctly uses `partial_cmp` which returns `None` for NaN values, preventing NaN from being ordered as cheaper. **Severity:** ok, **Action taken:** No change needed; behavior is correct. Existing test `incommensurable_bases_are_never_compared` covers this.

7. **File:** `src/cost.rs`, **Symbol:** `size_ratio`, **Finding:** Window composition — when multiple `Sized` bases are declared, correctly finds the minimum. However, if all declared values are unusable (NaN, zero, negative), returns `None` which is correct. **Severity:** ok, **Action taken:** No change needed; added pinning test `size_ratio_handles_all_unusable_values`.
