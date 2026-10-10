# r17b-bug-hunt: src/cost.rs Audit Findings

Audit of `src/cost.rs` for edge-case bugs — window composition,
torn/unparseable receipt handling, divide-by-zero or NaN in trust means.
Scope is `src/cost.rs` only (the router/window-composition logic in
`src/run.rs` is split to a separate task). Every finding is listed below,
fixed or not, as a numbered list: file, symbol, finding, severity
(`bug` | `smell` | `ok`), action taken.

---

## Findings

### 1. `src/cost.rs` — `size_ratio`
- **Finding**: `params_b` (the worker's own parameter count) was not
  validated. A NaN, infinite, zero, or negative `params_b` flowed straight
  into `params_b / cheapest` and produced an invalid ratio (`Some(NaN)` or
  `Some(inf)`) instead of `None`, poisoning the trust mean.
- **Severity**: bug
- **Action taken**: Fixed — added `if !is_usable(params_b) { return None; }`
  after the `cheapest` guard, symmetric with the existing `is_usable(cheapest)`
  divide-by-zero guard. Pinning test:
  `size_ratio_returns_none_when_params_b_is_unusable`.

### 2. `src/cost.rs` — `estimate_usd`
- **Finding**: Correctly returns `f64::NAN` for a non-usable price (NaN,
  infinite, negative, zero) via the `is_usable` guard; valid prices compute
  `tokens / 1_000_000 * price` with no divide-by-zero (the divisor is a
  constant `1_000_000.0`).
- **Severity**: ok
- **Action taken**: None. Pinned by `estimate_usd_handles_nan_and_infinity`.

### 3. `src/cost.rs` — `estimate_usd_micros`
- **Finding**: Correctly returns `0` for a non-usable price. The saturating
  `as u64` cast cannot see a NaN because the `is_usable` guard runs first
  (price is finite and strictly positive), and huge products saturate to
  `u64::MAX` as the doc comment claims. The `as u64` cast is only reached
  with a usable price, so no NaN/inf can reach it.
- **Severity**: ok
- **Action taken**: None. Pinned by `estimate_usd_micros_handles_nan_and_infinity`.

### 4. `src/cost.rs` — `attempt_expense` (torn / unparseable receipts)
- **Finding**: A torn receipt (declared basis but no recorded `tokens`) is
  correctly folded to `None` (unknown), never to a zero cost or an
  invented rate. The `Priced` arm uses `tokens.map(...)` (None tokens →
  None); the `Sized` arm uses `tokens?` then `size_ratio(...)?`.
- **Severity**: ok
- **Action taken**: None. Pinned by `attempt_expense_handles_torn_receipts`.

### 5. `src/cost.rs` — `size_ratio` (divide-by-zero / unusable `all_declared`)
- **Finding**: The cheapest `Sized` basis is selected with `partial_cmp`
  (so a NaN entry is never chosen as cheapest), and `is_usable(cheapest)`
  guards the division, so a zero/NaN/infinite/negative cheapest yields
  `None` rather than a divide-by-zero or NaN ratio. An `all_declared` with
  no `Sized` basis yields `None` (never invents a reference).
- **Severity**: ok
- **Action taken**: None. Pinned by `size_ratio_returns_none_when_cheapest_is_zero`
  and `size_ratio_handles_all_unusable_values`.

### 6. `src/cost.rs` — `compare`
- **Finding**: Uses `f64::partial_cmp`, which returns `None` for any NaN
  operand, so a NaN basis can never compare as cheaper. Different variants
  (`Priced` vs `Sized`) and missing bases are correctly non-comparable.
- **Severity**: ok
- **Action taken**: None. Behavior is correct by design (pinned by
  `cost_model::incommensurable_bases_are_never_compared` in the contract
  suite).

### 7. `src/cost.rs` — `basis`
- **Finding**: `basis` does not validate its inputs —
  `basis(Some(x), Some(f64::NAN))` returns `Some(Basis::Priced(NaN))` and
  `basis(Some(-1.0), None)` returns `Some(Basis::Sized(-1.0))`, storing
  unusable values in the `Basis` enum.
- **Severity**: smell
- **Action taken**: Not fixed. Adding validation here would change the
  public API and is a refactor, out of scope for a bug-hunt. The downstream
  consumers (`estimate_usd`, `size_ratio`, `compare`, and now
  `attempt_expense`'s `Priced` arm) all defend against unusable values, so
  an unusable `Basis` is rendered harmless rather than silent.

### 8. `src/cost.rs` — `is_usable`
- **Finding**: The single predicate `value.is_finite() && value > 0.0`
  correctly classifies finite-and-strictly-positive numbers as usable and
  everything else (zero, negative, NaN, ±inf) as unusable. It is the one
  source of truth every other guard reuses.
- **Severity**: ok
- **Action taken**: None.

### 9. `src/cost.rs` — `attempt_expense` (Priced arm, unusable declared price)
- **Finding**: The `Basis::Priced(price)` arm called
  `estimate_usd_micros(t, price)` without first validating `price`.
  `estimate_usd_micros` returns `0` for a non-usable price, so an unusable
  declared price (NaN, zero, negative, infinite — reachable because
  `basis` does not validate, see finding 7) combined with recorded tokens
  produced `Some(Expense::Usd { micros: 0, estimated: true })`: a silent
  $0 estimate that understates a worker's expense and misleads the router.
  The `Sized` arm was already symmetric (it rejects unusable params via
  `size_ratio`); the `Priced` arm was not.
- **Severity**: bug
- **Action taken**: Fixed — added an `if !is_usable(price) { return None; }`
  guard at the top of the `Priced` arm, mirroring the `Sized` arm's
  validation. An unusable declared price now yields unknown (`None`), never
  a fabricated $0. Pinning test:
  `attempt_expense_unusable_price_with_tokens_is_unknown`.

### 10. `src/cost.rs` — `attempt_expense` (measured `cost_micros = Some(0)`)
- **Finding**: `Some(micros)` is treated outright as a measured cost
  (`Expense::Usd { micros, estimated: false }`), so a hypothetical
  `Some(0)` would be recorded as a $0 measurement rather than unknown.
- **Severity**: smell
- **Action taken**: Not fixed. This is a contract boundary, not a clear
  bug: the transcript layer (`transcript::cost_micros_from`) already
  filters zero, negative, and non-finite reported costs to `None`
  (`micros.is_finite() && micros > 0.0`) before they reach `cost.rs`, so
  `attempt_expense`'s contract is that `Some(micros)` is already a valid
  positive measurement. Duplicating that filter in `cost.rs` would
  duplicate authority and alter the truthfulness ladder; the correct home
  for the zero-filter is the receipt parser. Documented here, not changed.

### 11. `src/cost.rs` — window composition (not present in this module)
- **Finding**: `src/cost.rs` contains no window-composition logic. The
  composition of receipts across a time window — `row_expense`,
  `task_cost_cell`, `cost_since_windows`, `cost_last` (latest attempt per
  task without double-counting), and the dollar/ratio blending — lives in
  `src/run.rs`, which is outside this task's file scope.
- **Severity**: ok (not applicable to `src/cost.rs`)
- **Action taken**: None. Window-composition edge cases (e.g. a window
  that mixes measured and estimated dollars, or a torn receipt mid-window)
  should be audited against `src/run.rs` in a separate task.

---

## Summary

- **Bugs fixed**: 2
  1. `size_ratio` did not validate `params_b` (finding 1) — fixed with an
     `is_usable(params_b)` guard.
  2. `attempt_expense`'s `Priced` arm did not validate the declared price
     and fabricated a $0 estimate for an unusable price (finding 9) — fixed
     with an `is_usable(price)` guard, symmetric with the `Sized` arm.
- **Smells noted (not fixed)**: 2
  - `basis` stores unusable values without validation (finding 7) — an API
    change/refactor, out of scope; downstream guards render it harmless.
  - `attempt_expense` treats `Some(0)` as a $0 measurement (finding 10) —
    a contract boundary owned by the transcript parser, which already
    filters zero/non-finite costs.
- **OK findings**: 7 (findings 2, 3, 4, 5, 6, 8, 11) — correct behavior,
  each pinned by an existing or new test where applicable.
- **Pinning tests added in `src/cost.rs`**:
  - `attempt_expense_unusable_price_with_tokens_is_unknown` (finding 9).
  - (The prior audit already added `size_ratio_returns_none_when_params_b_is_unusable`,
    `size_ratio_returns_none_when_cheapest_is_zero`,
    `size_ratio_handles_all_unusable_values`,
    `estimate_usd_micros_handles_nan_and_infinity`,
    `estimate_usd_handles_nan_and_infinity`,
    `attempt_expense_handles_torn_receipts`, and
    `attempt_expense_propagates_invalid_params`.)
- **Window composition**: not in `src/cost.rs`; deferred to a `src/run.rs`
  audit task.
