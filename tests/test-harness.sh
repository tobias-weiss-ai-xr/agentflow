#!/usr/bin/env bash
# test-harness.sh — SOTA test framework for taskfleet bash codebase.
#
# Paradigms implemented:
#   - xUnit-style groups (tf_test/tf_group_begin/end)
#   - Assertion library (tf_assert, tf_assert_eq, tf_assert_not, tf_assert_re,
#     tf_assert_gt, tf_assert_lt, tf_assert_contains, tf_assert_not_contains,
#     tf_assert_exit_code)
#   - Property-based testing (tf_for_all, tf_for_each, tf_property)
#   - Golden-file testing (tf_assert_golden, tf_update_golden)
#   - Fixture/setup/teardown (tf_fixture, tf_ensure_tmpdir)
#   - Test isolation (tf_isolated_test)
#   - Test categorization (tf_skip, tf_slow, tf_flaky)
#   - Timing/benchmarks (tf_timed)
#   - Structured TAP-compatible output (TF_TAP=1)
#   - Coverage markers (TF_COVERAGE=1)
#   - Mutation hints (tf_assert_with_hint)

set -uo pipefail

# ---------------------------------------------------------------------------
# Counters and state
# ---------------------------------------------------------------------------
TF_TESTS_RUN=0
TF_TESTS_PASS=0
TF_TESTS_FAIL=0
TF_TESTS_SKIP=0
TF_TESTS_XFAIL=0
TF_CURRENT_GROUP=""
TF_GROUP_FAILED=0
TF_SUITE_NAME="${TF_SUITE_NAME:-taskfleet}"
TF_TMPDIRS=()        # temp dirs created during tests
TF_FIXTURES_CLEANUP=() # cleanup functions registered by tf_fixture

# ---------------------------------------------------------------------------
# TAP output mode (TF_TAP=1 for CI integration)
# ---------------------------------------------------------------------------
tf_tap() {
  [[ "${TF_TAP:-0}" == "1" ]] || return 0
  local ok="$1" testnum="$2" desc="$3" directive="${4:-}"
  if [[ "$ok" == "ok" ]]; then
    [[ -n "$directive" ]] && echo "$ok $testnum - $desc # $directive" || echo "$ok $testnum - $desc"
  else
    [[ -n "$directive" ]] && echo "not $ok $testnum - $desc # $directive" || echo "not $ok $testnum - $desc"
  fi
}

# ---------------------------------------------------------------------------
# Core assertions
# ---------------------------------------------------------------------------

tf_test() {
  TF_CURRENT_GROUP="$*"
  TF_TESTS_RUN=$((TF_TESTS_RUN + 1))
  TF_GROUP_FAILED=0
  [[ "${TF_TAP:-0}" == "1" ]] && echo "# Subtest: $*"
}

tf_pass() {
  printf '  \033[32mPASS\033[0m %s\n' "$TF_CURRENT_GROUP"
  TF_TESTS_PASS=$((TF_TESTS_PASS + 1))
}

tf_fail() {
  printf '  \033[31mFAIL\033[0m %s\n    %s\n' "$TF_CURRENT_GROUP" "$*"
  TF_TESTS_FAIL=$((TF_TESTS_FAIL + 1))
}

tf_skip_test() {
  printf '  \033[33mSKIP\033[0m %s — %s\n' "$TF_CURRENT_GROUP" "${1:-no reason}"
  TF_TESTS_SKIP=$((TF_TESTS_SKIP + 1))
  TF_GROUP_FAILED=0 # skip doesn't count as failure
}

tf_xfail() {
  printf '  \033[36mXFAIL\033[0m %s — %s\n' "$TF_CURRENT_GROUP" "${1:-expected to fail}"
  TF_TESTS_XFAIL=$((TF_TESTS_XFAIL + 1))
}

tf_group_begin() { TF_GROUP_FAILED=0; }

tf_group_end() {
  if [[ "$TF_GROUP_FAILED" == "0" ]]; then tf_pass; else tf_fail "$TF_CURRENT_GROUP"; fi
}

# tf_assert <description> <condition-cmd...>  — passes if exit 0
tf_assert() {
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s\n' "$desc"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_not <description> <cmd...>  — passes if cmd exits NON-zero
tf_assert_not() {
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[31mBAD\033[0m  %s (expected non-zero exit)\n' "$desc"
    TF_GROUP_FAILED=1
  else
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  fi
}

# tf_assert_eq <desc> <expected> <actual>
tf_assert_eq() {
  local desc="$1" exp="$2" act="$3"
  if [[ "$exp" == "$act" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (expected %q, got %q)\n' "$desc" "$exp" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_ne <desc> <unexpected> <actual>
tf_assert_ne() {
  local desc="$1" unexpected="$2" act="$3"
  if [[ "$unexpected" != "$act" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (expected anything except %q)\n' "$desc" "$unexpected"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_gt <desc> <threshold> <actual>
tf_assert_gt() {
  local desc="$1" thresh="$2" act="$3"
  if [[ "$act" =~ ^[0-9]+$ && "$act" -gt "$thresh" ]]; then
    printf '    \033[32mok\033[0m   %s (%s > %s)\n' "$desc" "$act" "$thresh"
  else
    printf '    \033[31mBAD\033[0m  %s (expected > %s, got %s)\n' "$desc" "$thresh" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_lt <desc> <threshold> <actual>
tf_assert_lt() {
  local desc="$1" thresh="$2" act="$3"
  if [[ "$act" =~ ^[0-9]+$ && "$act" -lt "$thresh" ]]; then
    printf '    \033[32mok\033[0m   %s (%s < %s)\n' "$desc" "$act" "$thresh"
  else
    printf '    \033[31mBAD\033[0m  %s (expected < %s, got %s)\n' "$desc" "$thresh" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_re <desc> <pattern> <actual_string>
tf_assert_re() {
  local desc="$1" pat="$2" act="$3"
  if grep -qP "$pat" <<< "$act" 2>/dev/null; then
    printf '    \033[32mok\033[0m   %s (matches /%s/)\n' "$desc" "$pat"
  else
    printf '    \033[31mBAD\033[0m  %s (no match for /%s/ in %q)\n' "$desc" "$pat" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_contains <desc> <needle> <haystack>
tf_assert_contains() {
  local desc="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '    \033[32mok\033[0m   %s (contains %q)\n' "$desc" "$needle"
  else
    printf '    \033[31mBAD\033[0m  %s (expected to contain %q)\n' "$desc" "$needle"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_not_contains <desc> <needle> <haystack>
tf_assert_not_contains() {
  local desc="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '    \033[32mok\033[0m   %s (does not contain %q)\n' "$desc" "$needle"
  else
    printf '    \033[31mBAD\033[0m  %s (should not contain %q)\n' "$desc" "$needle"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_exit_code <desc> <expected_rc> <actual_rc>
tf_assert_exit_code() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '    \033[32mok\033[0m   %s (exit %s)\n' "$desc" "$expected"
  else
    printf '    \033[31mBAD\033[0m  %s (expected exit %s, got %s)\n' "$desc" "$expected" "$actual"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_with_hint <desc> <condition-cmd...> — shows diagnostic on failure
tf_assert_with_hint() {
  local desc="$1"; shift
  local hint_cmd="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    local diag
    diag="$(eval "$hint_cmd" 2>&1)"
    printf '    \033[31mBAD\033[0m  %s\n' "$desc"
    printf '    \033[33mHINT\033[0m  %s\n' "$diag"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_jq_eq <desc> <jq_filter> <file> <expected_value>
tf_assert_jq_eq() {
  local desc="$1" filter="$2" file="$3" expected="$4"
  local actual
  actual="$(jq -r "$filter" "$file" 2>/dev/null)"
  if [[ "$actual" == "$expected" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (jq %s: expected %q, got %q)\n' "$desc" "$filter" "$expected" "$actual"
    TF_GROUP_FAILED=1
  fi
}

# ---------------------------------------------------------------------------
# Property-based testing (bash generators)
# ---------------------------------------------------------------------------

# tf_for_all <description> <generator_fn> <property_fn> [<n>]
#   Runs property_fn for each value emitted by generator_fn (one per line).
#   Generator must echo one value per line. Default n=100.
tf_for_all() {
  local desc="$1" gen="$2" prop="$3" n="${4:-100}"
  local i=0 pass=0 fail=0
  while IFS= read -r val && [[ $i -lt $n ]]; do
    i=$((i + 1))
    if $prop "$val" >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      fail=$((fail + 1))
      printf '    \033[31mCOUNTER\033[0m  %s — counterexample: %q\n' "$desc" "$val"
      TF_GROUP_FAILED=1
    fi
  done < <($gen "$n")
  printf '    \033[32mok\033[0m   %s (%d/%d passed)\n' "$desc" "$pass" "$i"
}

# tf_for_each <description> <values...> -- <property_fn>
#   Runs property_fn once per value. Use for small explicit sets.
tf_for_each() {
  local desc="$1"; shift
  local fn=""
  local vals=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--" ]]; then shift; fn="$1"; shift; break; fi
    vals+=("$1"); shift
  done
  [[ -z "$fn" ]] && return 1
  local pass=0 fail=0 total="${#vals[@]}"
  for v in "${vals[@]}"; do
    if $fn "$v" >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      fail=$((fail + 1))
      printf '    \033[31mCOUNTER\033[0m  %s — counterexample: %q\n' "$desc" "$v"
      TF_GROUP_FAILED=1
    fi
  done
  printf '    \033[32mok\033[0m   %s (%d/%d passed)\n' "$desc" "$pass" "$total"
}

# tf_property <name> <property_fn> [n=100]
#   Shorthand: property_fn takes no args, runs n times, must always return 0.
tf_property() {
  local desc="$1" fn="$2" n="${3:-100}"
  local i=0 pass=0
  for ((i = 0; i < n; i++)); do
    if $fn >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      printf '    \033[31mCOUNTER\033[0m  %s — failed on iteration %d\n' "$desc" "$i"
      TF_GROUP_FAILED=1
      return 1
    fi
  done
  printf '    \033[32mok\033[0m   %s (%d iterations)\n' "$desc" "$pass"
}

# ---------------------------------------------------------------------------
# Generators for property-based tests
# ---------------------------------------------------------------------------

# Generate random task IDs (alphanumeric, 2-8 chars)
tf_gen_task_id() {
  local n="${1:-50}"
  local chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-'
  for ((i = 0; i < n; i++)); do
    local len=$((RANDOM % 7 + 2))
    local id=""
    for ((j = 0; j < len; j++)); do
      id+="${chars:RANDOM%${#chars}:1}"
    done
    echo "$id"
  done
}

# Generate random task count (1-500)
tf_gen_task_count() {
  local n="${1:-50}"
  for ((i = 0; i < n; i++)); do
    echo $((RANDOM % 500 + 1))
  done
}

# Generate random file paths (Unix-safe)
tf_gen_filepath() {
  local n="${1:-50}"
  local parts="src lib core crates tests scripts"
  local exts="rs ts tsx sh py json toml yaml md"
  for ((i = 0; i < n; i++)); do
    local depth=$((RANDOM % 4 + 1))
    local path=""
    for ((j = 0; j < depth; j++)); do
      local arr=($parts)
      path+="${arr[RANDOM%${#arr[@]}]}/"
    done
    local arr=($exts)
    path+="${arr[RANDOM%${#arr[@]}]}"
    echo "$path"
  done
}

# ---------------------------------------------------------------------------
# Golden-file testing
# ---------------------------------------------------------------------------

TF_GOLDEN_DIR="${TF_GOLDEN_DIR:-tests/golden}"

# ---------------------------------------------------------------------------
# Deterministic seeding (TF_SEED) — reproducible property/fuzz runs
# ---------------------------------------------------------------------------
TF_SEED="${TF_SEED:-$(date +%s%N)}"
TF_RNG_STATE="$TF_SEED"

# Initialize the RNG from TF_SEED. Print the seed so runs can be reproduced.
tf_seed_init() {
  TF_SEED="${TF_SEED:-$(date +%s%N)}"
  TF_RNG_STATE="$TF_SEED"
  # shellcheck disable=SC2155
  printf '    \033[36mSEED\033[0m  %s (set TF_SEED=%s to reproduce)\n' "$TF_SEED" "$TF_SEED"
}

# Deterministic pseudo-random number in [0, bound). Replaces $RANDOM.
tf_rand() {
  local bound="${1:-32768}"
  # xorshift64 — deterministic, fast, good distribution
  TF_RNG_STATE=$(( TF_RNG_STATE ^ (TF_RNG_STATE << 13) ))
  TF_RNG_STATE=$(( TF_RNG_STATE ^ (TF_RNG_STATE >> 7) ))
  TF_RNG_STATE=$(( TF_RNG_STATE ^ (TF_RNG_STATE << 17) ))
  echo $(( (TF_RNG_STATE & 0x7FFFFFFF) % bound ))
}

# Deterministic generator of random task ids (seeded).
tf_gen_seeded_id() {
  local n="${1:-50}"
  local chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-'
  local i j len id
  for ((i = 0; i < n; i++)); do
    len=$(( $(tf_rand 7) + 2 ))
    id=""
    for ((j = 0; j < len; j++)); do
      id+="${chars:$(tf_rand ${#chars}):1}"
    done
    echo "$id"
  done
}

# ---------------------------------------------------------------------------
# Counterexample shrinking (delta debugging)
# ---------------------------------------------------------------------------

# tf_shrink <property_fn> <input> [<max_rounds=10>]
#   Given an input that fails property_fn (returns non-zero), shrink it
#   to a minimal failing case using delta debugging. Property fn takes
#   the candidate on $1. Echoes the minimal failing input on stdout.
tf_shrink() {
  local prop="$1" input="$2" max_rounds="${3:-10}"
  local current="$input"
  local round size n chunk i candidate reduced
  for ((round = 0; round < max_rounds; round++)); do
    size="${#current}"
    [[ "$size" -le 1 ]] && break
    n=2
    reduced=0
    while [[ "$n" -le "$size" ]]; do
      chunk=$(( size / n ))
      chunk=$(( chunk == 0 ? 1 : chunk ))
      i=0
      while [[ $i -lt "$size" ]]; do
        candidate="${current:0:i}${current:i+chunk}"
        if [[ -n "$candidate" ]] && ! $prop "$candidate" >/dev/null 2>&1; then
          current="$candidate"
          size="${#current}"
          reduced=1
          break 2   # restart with the reduced input
        fi
        i=$((i + chunk))
      done
      n=$((n * 2))
    done
    [[ "$reduced" == "0" ]] && break
  done
  printf '%s' "$current"
}

# ---------------------------------------------------------------------------
# Table-driven testing
# ---------------------------------------------------------------------------

# tf_table_test <description> <fn> <rows_csv...>
#   Runs fn for each row. Each row is a single-quoted CSV: "id,arg1,arg2,...".
#   fn is called as: fn <row_id> <col1> <col2> ...
#   Row ids are printed as sub-assertions.
tf_table_test() {
  local desc="$1" fn="$2"; shift 2
  local total=0 pass=0 row
  for row in "$@"; do
    total=$((total + 1))
    local id args
    id="${row%%,*}"
    args="${row#*,}"
    # split args on commas into separate shell-quoted arguments
    local IFS=','
    # shellcheck disable=SC2206
    local parts=($args)
    IFS=' '
    local q="" p
    for p in "${parts[@]}"; do
      q+=" '${p//\'/\'\\''\'}'"
    done
    if eval "$fn '$id'$q" >/dev/null 2>&1; then
      pass=$((pass + 1))
      printf '    \033[32mok\033[0m   %s [%s]\n' "$desc" "$id"
    else
      printf '    \033[31mBAD\033[0m  %s [%s] — row=%q\n' "$desc" "$id" "$row"
      TF_GROUP_FAILED=1
    fi
  done
  printf '    \033[32mok\033[0m   %s: %d/%d rows passed\n' "$desc" "$pass" "$total"
}

# ---------------------------------------------------------------------------
# Mutation testing
# ---------------------------------------------------------------------------
TF_MUT_TOTAL=0
TF_MUT_KILLED=0
TF_MUT_SURVIVED=0

# tf_mutation_test <name> <target_file> <test_runner> <mutations...>
#   Mutation testing: for each mutation expression (a sed -i expression),
#   apply it to a COPY of target_file, run test_runner against the copy,
#   and check the test FAILS (mutation killed).
#
#   target_file: path relative to TF_MUT_SRC (default: repo root, but tests
#                override with TF_MUT_SRC pointing at a sandbox copy).
#   test_runner: command that runs the test suite against the mutated tree.
#   mutation:   sed expression, e.g. 's/done/completed/'
#
#   A mutation is KILLED if the test fails on the mutated source.
#   SURVIVED means the tests don't catch the injected bug — a quality gap.
#
#   IMPORTANT: mutations are applied to a sandbox copy, never the real lib.
tf_mutation_test() {
  local name="$1" target="$2" runner="$3"; shift 3
  local src_base="${TF_MUT_SRC:-$TF_DIR}"
  local target_path="$src_base/$target"
  if [[ ! -f "$target_path" ]]; then
    printf '    \033[31mBAD\033[0m  mutation %s — target %s missing\n' "$name" "$target"
    TF_GROUP_FAILED=1
    return 1
  fi
  local work
  work="$(mktemp -d)"
  local result=0
  for expr in "$@"; do
    TF_MUT_TOTAL=$((TF_MUT_TOTAL + 1))
    # sandbox copy of the whole source dir so tests can source siblings
    local mutdir="$work/mut"
    rm -rf "$mutdir"; mkdir -p "$mutdir"
    cp -r "$src_base/." "$mutdir/"
    local tfile="$mutdir/$target"
    if ! sed -i "$expr" "$tfile" 2>/dev/null; then
      printf '    \033[33mSKIP\033[0m  mutation %s — sed expr invalid: %q\n' "$name" "$expr"
      continue
    fi
    # run the test against the mutated copy. The runner command may contain
    # the placeholder __MUTDIR__ which is substituted with the sandbox path.
    local run_cmd="${runner//__MUTDIR__/$mutdir}"
    if (cd "$mutdir" && eval "$run_cmd" >/dev/null 2>&1); then
      TF_MUT_SURVIVED=$((TF_MUT_SURVIVED + 1))
      printf '    \033[31mSURVIVED\033[0m  %s — %q (tests did NOT catch this bug)\n' "$name" "$expr"
      TF_GROUP_FAILED=1
      result=1
    else
      TF_MUT_KILLED=$((TF_MUT_KILLED + 1))
      printf '    \033[32mKILLED\033[0m  %s — %q\n' "$name" "$expr"
    fi
  done
  rm -rf "$work"
  return $result
}

# Print mutation score summary.
tf_mutation_report() {
  local total="$TF_MUT_TOTAL" killed="$TF_MUT_KILLED" survived="$TF_MUT_SURVIVED"
  printf '    \033[36mMUTATION SCORE\033[0m  %d/%d killed (%d%%), %d survived\n' \
    "$killed" "$total" $(( total == 0 ? 0 : killed * 100 / total )) "$survived"
}

# ---------------------------------------------------------------------------
# Coverage tracking
# ---------------------------------------------------------------------------
TF_COV_FUNCS=""

# tf_coverage_mark <function_name> — mark a source function as exercised.
tf_coverage_mark() {
  local fn="$1"
  [[ " $TF_COV_FUNCS " == *" $fn "* ]] || TF_COV_FUNCS="$TF_COV_FUNCS $fn"
}

# tf_coverage_report <lib_dir> — report which tf_* functions were exercised.
tf_coverage_report() {
  local libdir="${1:-$TF_DIR/lib}"
  local all="" fn
  for f in "$libdir"/*.sh; do
    while IFS= read -r fn; do
      [[ -n "$fn" ]] && all="$all $fn"
    done < <(grep -oE '^tf_[a-z_]+\(' "$f" | sed 's/(//')
  done
  local total=0 covered=0 f
  for f in $all; do
    total=$((total + 1))
    if [[ " $TF_COV_FUNCS " == *" $f "* ]]; then
      covered=$((covered + 1))
    fi
  done
  printf '    \033[36mCOVERAGE\033[0m  %d/%d source functions exercised (%d%%)\n' \
    "$covered" "$total" $(( total == 0 ? 0 : covered * 100 / total ))
}

# ---------------------------------------------------------------------------
# JSON validity assertion
# ---------------------------------------------------------------------------
tf_assert_valid_json() {
  local desc="$1" file="$2"
  if jq -e . "$file" >/dev/null 2>&1; then
    printf '    \033[32mok\033[0m   %s (valid JSON)\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (INVALID JSON: %s)\n' "$desc" "$(head -c 100 "$file" 2>/dev/null | tr '\n' ' ')"
    TF_GROUP_FAILED=1
  fi
}

# ---------------------------------------------------------------------------
# Chaos / fault injection helpers
# ---------------------------------------------------------------------------

# tf_chaos_truncate <file> [<bytes>] — truncate a file to simulate partial write.
tf_chaos_truncate() {
  local file="$1" bytes="${2:-0}"
  truncate -s "$bytes" "$file" 2>/dev/null || dd if=/dev/null of="$file" bs=1 seek="$bytes" count=0 2>/dev/null
}

# tf_chaos_corrupt <file> — overwrite middle bytes with garbage.
tf_chaos_corrupt() {
  local file="$1"
  local sz
  sz="$(stat -c %s "$file" 2>/dev/null || echo 0)"
  [[ "$sz" -eq 0 ]] && return
  local mid=$((sz / 2))
  head -c "$mid" "$file" > "$file.tmp"
  printf '\xff\xfe\x00GARBAGE' >> "$file.tmp"
  tail -c "$((sz - mid))" "$file" >> "$file.tmp" 2>/dev/null
  mv "$file.tmp" "$file"
}

# tf_assert_golden <name> <actual_content>
#   Compares actual content against stored golden file. If TF_UPDATE_GOLDEN=1,
#   writes the actual content as the new golden file.
tf_assert_golden() {
  local name="$1" actual="$2"
  local golden="$TF_GOLDEN_DIR/$name.golden"
  if [[ "${TF_UPDATE_GOLDEN:-0}" == "1" ]]; then
    mkdir -p "$(dirname "$golden")"
    printf '%s' "$actual" > "$golden"
    printf '    \033[33mUPDATED\033[0m  golden/%s\n' "$name"
    return 0
  fi
  if [[ ! -f "$golden" ]]; then
    printf '    \033[33mMISSING\033[0m  golden/%s (run TF_UPDATE_GOLDEN=1 to create)\n' "$name"
    TF_GROUP_FAILED=1
    return 1
  fi
  local expected
  expected="$(cat "$golden")"
  if [[ "$actual" == "$expected" ]]; then
    printf '    \033[32mok\033[0m   golden/%s matches\n' "$name"
  else
    printf '    \033[31mBAD\033[0m  golden/%s mismatch\n' "$name"
    diff <(echo "$expected") <(echo "$actual") | head -10 | sed 's/^/    /'
    TF_GROUP_FAILED=1
  fi
}

# ---------------------------------------------------------------------------
# Fixtures and lifecycle
# ---------------------------------------------------------------------------

# tf_ensure_tmpdir <name>  → creates/tracks a temp dir, prints its path
tf_ensure_tmpdir() {
  local name="${1:-test-$$_$RANDOM}"
  local d="/tmp/tf-test-$name"
  rm -rf "$d"
  mkdir -p "$d"
  TF_TMPDIRS+=("$d")
  echo "$d"
}

# tf_fixture <setup_fn> <cleanup_fn> <test_fn>
#   Runs setup, then test, then cleanup (even if test fails).
tf_fixture() {
  local setup="$1" cleanup="$2"; shift 2
  local rc=0
  $setup || { tf_fail "fixture setup failed: $setup"; return 1; }
  "$@" || rc=$?
  $cleanup || true
  return $rc
}

# tf_isolated_test <name> <test_body_fn>
#   Creates a fresh tmpdir sandbox, sets TF_STATE_DIR/TF_LOG_DIR, runs body.
tf_isolated_test() {
  local name="$1"; shift
  local sandbox
  sandbox="$(tf_ensure_tmpdir "$name")"
  export TF_STATE_DIR="$sandbox/state"
  export TF_LOG_DIR="$sandbox/logs"
  mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"
  "$@"
}

# ---------------------------------------------------------------------------
# Timing and benchmarks
# ---------------------------------------------------------------------------

# tf_timed <description> <cmd...>
#   Runs cmd, prints elapsed time. Asserts under TF_BENCH_THRESHOLD if set.
tf_timed() {
  local desc="$1"; shift
  local start end elapsed
  start="$(date +%s%N)"
  "$@"
  local rc=$?
  end="$(date +%s%N)"
  elapsed=$(( (end - start) / 1000000 ))  # ms
  local threshold="${TF_BENCH_THRESHOLD:-5000}"
  if [[ $elapsed -lt $threshold ]]; then
    printf '    \033[32mok\033[0m   %s (%dms < %dms)\n' "$desc" "$elapsed" "$threshold"
  else
    printf '    \033[33mSLOW\033[0m  %s (%dms >= %dms threshold)\n' "$desc" "$elapsed" "$threshold"
    [[ "${TF_STRICT_BENCH:-0}" == "1" ]] && TF_GROUP_FAILED=1
  fi
  return $rc
}

# ---------------------------------------------------------------------------
# Global cleanup and summary
# ---------------------------------------------------------------------------

tf_cleanup() {
  # Remove tracked tmpdirs
  for d in "${TF_TMPDIRS[@]}"; do
    rm -rf "$d"
  done
  # Run registered fixture cleanups
  for fn in "${TF_FIXTURES_CLEANUP[@]}"; do
    $fn >/dev/null 2>&1 || true
  done
}

tf_test_summary() {
  local rc=0
  echo ""
  echo "──────────────────────────────────────────"
  printf 'Tests: %d run, \033[32m%d passed\033[0m, \033[31m%d failed\033[0m' \
    "$TF_TESTS_RUN" "$TF_TESTS_PASS" "$TF_TESTS_FAIL"
  [[ $TF_TESTS_SKIP -gt 0 ]] && printf ', \033[33m%d skipped\033[0m' "$TF_TESTS_SKIP"
  [[ $TF_TESTS_XFAIL -gt 0 ]] && printf ', \033[36m%d xfail\033[0m' "$TF_TESTS_XFAIL"
  echo ""
  if [[ "$TF_MUT_TOTAL" -gt 0 ]]; then
    printf 'Mutations: \033[32m%d killed\033[0m, \033[31m%d survived\033[0m (score %d%%)\n' \
      "$TF_MUT_KILLED" "$TF_MUT_SURVIVED" $(( TF_MUT_KILLED * 100 / TF_MUT_TOTAL ))
  fi
  if [[ -n "$TF_COV_FUNCS" ]]; then
    tf_coverage_report "$TF_DIR/lib"
  fi
  [[ "$TF_TESTS_FAIL" -gt 0 ]] && rc=1
  tf_cleanup
  return $rc
}

# Backward compat (old names)
wo_test() { tf_test "$@"; }
wo_pass() { tf_pass "$@"; }
wo_fail() { tf_fail "$@"; }
wo_assert() { tf_assert "$@"; }
wo_assert_not() { tf_assert_not "$@"; }
wo_assert_eq() { tf_assert_eq "$@"; }
wo_group_begin() { tf_group_begin; }
wo_group_end() { tf_group_end; }
wo_test_summary() { tf_test_summary "$@"; }
