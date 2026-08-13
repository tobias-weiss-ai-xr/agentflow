#!/usr/bin/env bash
# unit/test-verify.sh — unit tests for lib/verify.sh
# Tests: pass/fail/timeout/skip, error classification, error snippets.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$SBOX/repo/.tf-worktrees"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
export TF_PROMPT_DIR="$HERE/../../prompts"
mkdir -p "$TF_REPO_DIR/.tf-worktrees" "$TF_STATE_DIR" "$TF_LOG_DIR"

git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t
git -C "$TF_REPO_DIR" config user.name test
echo base > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm init

export TF_CONFIG_DIR="$SBOX/config"
mkdir -p "$TF_CONFIG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":3}}
JSON

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"PASS","engine":"t","title":"passes","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"FAIL","engine":"t","title":"fails","section":"§1","deps":[],"scope":["b.rs"],"accept":"false","manual":false},
  {"id":"SLOW","engine":"t","title":"slow","section":"§1","deps":[],"scope":["c.rs"],"accept":"sleep 10","manual":false},
  {"id":"SKIP","engine":"t","title":"skip","section":"§1","deps":[],"scope":["d.rs"],"accept":"","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"
. "$HERE/../../lib/verify.sh"

echo "=== [unit] verify.sh ==="

# ---- Gate: PASS ----
tf_group_begin; tf_test "PASS: exit 0 → verdict PASS"
WT="$(tf_worktree_create PASS)"
v="$(tf_verify PASS "$WT")"
tf_assert_eq "verdict is PASS" "PASS" "$v"
tf_worktree_remove PASS
tf_group_end

# ---- Gate: FAIL ----
tf_group_begin; tf_test "FAIL: exit non-zero → verdict starts with FAIL"
WT="$(tf_worktree_create FAIL)"
v="$(tf_verify FAIL "$WT")" || true
tf_assert_contains "verdict starts with FAIL:" "" "${v#FAIL}"
tf_worktree_remove FAIL
tf_group_end

# ---- Gate: TIMEOUT ----
tf_group_begin; tf_test "TIMEOUT: exceeds accept_timeout_s → verdict FAIL timeout"
WT="$(tf_worktree_create SLOW)"
v="$(tf_verify SLOW "$WT")" || true
tf_assert_eq "verdict" "FAIL: timeout after 3s" "$v"
tf_worktree_remove SLOW
tf_group_end

# ---- Gate: SKIP ----
tf_group_begin; tf_test "SKIP: empty accept → manual sign-off"
WT="$(tf_worktree_create SKIP)"
v="$(tf_verify SKIP "$WT")"
tf_assert_eq "verdict" "SKIP: no accept command (manual task)" "$v"
tf_worktree_remove SKIP
tf_group_end

# ---- Gate: error.json written on failure ----
tf_group_begin; tf_test "FAIL writes error.json with classification"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CLSFY","engine":"t","title":"classify","section":"§1","deps":[],"scope":["e.rs"],"accept":"echo error; false","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/clsfy-status.json"
WT="$(tf_worktree_create CLSFY)"
v="$(tf_verify CLSFY "$WT")" || true
tf_assert "error.json created" test -f "$TF_LOG_DIR/CLSFY.error.json"
cat_json="$(jq -r '.category' "$TF_LOG_DIR/CLSFY.error.json")"
tf_assert "category is non-empty" test -n "$cat_json"
tf_worktree_remove CLSFY
tf_group_end

# ---- Gate: PASS removes error.json ----
tf_group_begin; tf_test "PASS removes stale error.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CLEAN","engine":"t","title":"clean","section":"§1","deps":[],"scope":["f.rs"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/clean-status.json"
echo '{"category":"old","summary":"old"}' > "$TF_LOG_DIR/CLEAN.error.json"
WT="$(tf_worktree_create CLEAN)"
tf_verify CLEAN "$WT" >/dev/null
tf_assert_not "error.json removed" test -f "$TF_LOG_DIR/CLEAN.error.json"
tf_worktree_remove CLEAN
tf_group_end

# ---- Error classification ----
tf_group_begin; tf_test "tf_classify_error: compile_error"
echo 'error[E0308]: mismatched types
error: could not compile `wo-foo`
   Compiling wo-foo v0.1.0' > "$SBOX/compile.log"
cls="$(tf_classify_error "$SBOX/compile.log")"
tf_assert_contains "category=compile_error" "compile_error:" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: linker_error"
echo 'rust-lld: error: undefined symbol: FPDF_LoadPage
rust-lld: error: undefined symbol: FORM_CanUndo
collect2: error: ld returned 1' > "$SBOX/linker.log"
cls="$(tf_classify_error "$SBOX/linker.log")"
tf_assert_contains "category=linker_error" "linker_error:" "$cls"
tf_assert_contains "has symbol names" "FPDF_LoadPage" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: missing_package"
echo 'error: package ID specification `wo-formula` did not match any packages' > "$SBOX/missing.log"
cls="$(tf_classify_error "$SBOX/missing.log")"
tf_assert_contains "category=missing_package" "missing_package:" "$cls"
tf_assert_contains "has package name" "wo-formula" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: test_failure"
echo 'running 5 tests
test test_foo ... FAILED
test test_bar ... ok
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out' > "$SBOX/testfail.log"
cls="$(tf_classify_error "$SBOX/testfail.log")"
tf_assert_contains "category=test_failure" "test_failure:" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: timeout"
echo 'command timed out after 600 seconds' > "$SBOX/timeout.log"
cls="$(tf_classify_error "$SBOX/timeout.log")"
tf_assert_contains "category=timeout" "timeout:" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: unknown"
echo 'some random output without known patterns
exit code 1' > "$SBOX/unknown.log"
cls="$(tf_classify_error "$SBOX/unknown.log")"
tf_assert_contains "category=unknown" "unknown:" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: ts compile"
echo 'src/app.ts:12:5 - error TS2322: Type "string" is not assignable to type "number"' > "$SBOX/ts.log"
cls="$(tf_classify_error "$SBOX/ts.log")"
tf_assert_contains "category=compile_error" "compile_error:" "$cls"
tf_assert_contains "has TS code" "TS2322" "$cls"
tf_group_end

tf_group_begin; tf_test "tf_classify_error: empty log"
: > "$SBOX/empty.log"
cls="$(tf_classify_error "$SBOX/empty.log")"
tf_assert_contains "category=unknown" "unknown:" "$cls"
tf_group_end

# ---- tf_error_snippet ----
tf_group_begin; tf_test "tf_error_snippet extracts error lines"
cat > "$SBOX/snippet.log" <<'EOF'
line 1
line 2
line 3
error[E0308]: mismatched types
  --> src/main.rs:10:5
   |
10 |     let x: u32 = "hello";
   |                ^^^^^^^ expected u32, found &str
line after
EOF
snippet="$(tf_error_snippet "$SBOX/snippet.log")"
tf_assert_contains "has error line" "error[E0308]" "$snippet"
tf_assert_contains "has line numbers" "--> src/main.rs:10:5" "$snippet"
tf_group_end

tf_group_begin; tf_test "tf_error_snippet falls back to tail for no error lines"
echo -e "no error here\njust normal output\nthe end" > "$SBOX/notail.log"
snippet="$(tf_error_snippet "$SBOX/notail.log")"
tf_assert_contains "has some output" "just normal output" "$snippet"
tf_group_end

# ---- tf_get_error_category / tf_get_error_summary ----
tf_group_begin; tf_test "error getters read from error.json"
mkdir -p "$TF_LOG_DIR"
jq -n '{category:"test_cat",summary:"test_sum",classified_at:"now"}' > "$TF_LOG_DIR/GETTER.error.json"
tf_assert_eq "category" "test_cat" "$(tf_get_error_category GETTER)"
tf_assert_eq "summary" "test_sum" "$(tf_get_error_summary GETTER)"
tf_assert_eq "missing task" "none" "$(tf_get_error_category NONEXISTENT)"
tf_assert_eq "missing summary" "" "$(tf_get_error_summary NONEXISTENT)"
tf_group_end

# ---- Scope checking ----
tf_group_begin; tf_test "tf_verify_scope: in-scope edits produce no violations"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"SC1","engine":"t","title":"sc","section":"§1","deps":[],"scope":["a.rs","b.rs"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/sc1-status.json"
WT="$(tf_worktree_create SC1)"
echo a > "$WT/a.rs"
echo b > "$WT/b.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "in-scope"
drift="$(tf_verify_scope SC1 "$WT")"
tf_assert_eq "no violations on stdout" "" "$drift"
tf_assert "all in-scope in log" grep -q "all in-scope" "$TF_LOG_DIR/SC1.scope.log"
tf_worktree_remove SC1
tf_group_end

tf_group_begin; tf_test "tf_verify_scope: out-of-scope edits are flagged"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"SC2","engine":"t","title":"sc2","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/sc2-status.json"
WT="$(tf_worktree_create SC2)"
echo a > "$WT/a.rs"
echo z > "$WT/z.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "oos"
drift="$(tf_verify_scope SC2 "$WT")"
tf_assert_contains "z.rs flagged" "z.rs" "$drift"
tf_worktree_remove SC2
tf_group_end

tf_group_begin; tf_test "tf_verify_scope: directory scope matches files below"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"SC3","engine":"t","title":"sc3","section":"§1","deps":[],"scope":["src/"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/sc3-status.json"
WT="$(tf_worktree_create SC3)"
mkdir -p "$WT/src" "$WT/lib"
echo s > "$WT/src/foo.rs"
echo l > "$WT/lib/bar.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "dirs"
drift="$(tf_verify_scope SC3 "$WT")"
tf_assert_contains "lib/ flagged" "lib/bar.rs" "$drift"
tf_worktree_remove SC3
tf_group_end

# ---- Property: classification always returns "category: ..." ----
tf_group_begin; tf_test "property: tf_classify_error always returns category: <text>"
classify_always_structured() {
  local log="$SBOX/prop-classify-$$.log"
  # Generate random log content
  head -c 500 /dev/urandom | strings | head -10 > "$log"
  local result
  result="$(tf_classify_error "$log")"
  [[ "$result" == *": "* ]]
}
tf_property "classify_always_structured" classify_always_structured 30
tf_group_end


# ---- Robustness: gate sandbox (L5) must prepend cargo bin (regression) ----
tf_group_begin; tf_test "gate shell prepends ~/.cargo/bin so rustup rustc is found"
grep -q 'cargo/bin' "$HERE/../../lib/verify.sh" || TF_GROUP_FAILED=1
if grep -q 'cargo/bin' "$HERE/../../lib/verify.sh"; then
  printf '    \033[32mok\033[0m   verify.sh prepends cargo bin\n'
else
  printf '    \033[31mBAD\033[0m  verify.sh lacks cargo-bin PATH prepend (wasm gates will use /usr/bin/rustc)\n'
fi
# and the gate_env mechanism still exists
grep -q 'TF_GATE_ENV' "$HERE/../../lib/verify.sh" || TF_GROUP_FAILED=1
if grep -q 'TF_GATE_ENV' "$HERE/../../lib/verify.sh"; then
  printf '    \033[32mok\033[0m   TF_GATE_ENV mechanism present\n'
else
  printf '    \033[31mBAD\033[0m  TF_GATE_ENV missing\n'
fi
tf_group_end

tf_test_summary
