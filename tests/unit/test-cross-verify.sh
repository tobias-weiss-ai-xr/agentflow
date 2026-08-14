#!/usr/bin/env bash
# unit/test-cross-verify.sh — unit tests for cross-vendor verify gate.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_BRANCH_PREFIX="agent"
export TF_RECEIPT_DIR="$SBOX/receipts"
export TF_PROMPT_DIR="$SBOX/prompts"
export TF_LIB_DIR="$SBOX/lib"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR" "$TF_PROMPT_DIR" "$TF_LIB_DIR"

# Create a dummy prompt template
echo '{{TASK_ID}}: {{TASK_TITLE}}' > "$TF_PROMPT_DIR/worker.md"

# Workers with a primary and a verify worker
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[
  {"name":"primary","provider":"openai","model":"gpt-4o","enabled":true},
  {"name":"verifier","provider":"anthropic","model":"claude-sonnet-4-20250514","enabled":true}
],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5,"verify_cross_vendor_timeout_s":3}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/receipt.sh"
. "$HERE/../../lib/dispatch.sh"

init_repo() {
  rm -rf "$TF_REPO_DIR"
  mkdir -p "$TF_REPO_DIR/src"
  (cd "$TF_REPO_DIR" && git init -q && git checkout -b main -q)
  echo "fn main() {}" > "$TF_REPO_DIR/src/main.rs"
  (cd "$TF_REPO_DIR" && git add -A && git commit -q -m "init")
}

tf_reset() {
  STATUS_JSON="$SBOX/state/reset-$$.json"
  rm -f "$TF_RECEIPT_DIR"/*.ndjson
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV1","engine":"t","title":"Cross verify test","section":"§1","deps":[],"scope":["src/main.rs"],"accept":"true","manual":false}
]}
JSON
  tf_status_init
  init_repo
}

echo "=== [unit] cross-vendor verify ==="

# ---- tf_task_field reads verify_worker ----
tf_group_begin; tf_test "tf_task_field reads verify_worker when set"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV1","engine":"t","title":"T","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false,"verify_worker":"verifier"}
]}
JSON
tf_status_init
vw="$(tf_task_field CV1 .verify_worker)"
tf_assert_eq "verify_worker is verifier" "verifier" "$vw"
tf_group_end

# ---- verify_worker absent returns null ----
tf_group_begin; tf_test "verify_worker absent returns null/empty"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV1","engine":"t","title":"T","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
vw="$(tf_task_field CV1 .verify_worker)"
tf_assert "verify_worker is null or empty" test -z "$vw" -o "$vw" = "null"
tf_group_end

# ---- tf_cross_vendor_verify with mocked pi (PASS) ----
tf_group_begin; tf_test "tf_cross_vendor_verify returns PASS when pi exits 0"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV1","engine":"t","title":"T","section":"§1","deps":[],"scope":["src/main.rs"],"accept":"true","manual":false,"verify_worker":"verifier"}
]}
JSON
tf_status_init

cat > "$SBOX/pi" <<'SCRIPT'
#!/usr/bin/env bash
echo "Review passed"
exit 0
SCRIPT
chmod +x "$SBOX/pi"
export PATH="$SBOX:$PATH"

mkdir -p "$TF_REPO_DIR/../wt-cv1/src"
wt="$TF_REPO_DIR/../wt-cv1"
(cd "$wt" && git init -q && git checkout -b tf/CV1 -q)
echo "fn main() { println!(\"hello\"); }" > "$wt/src/main.rs"
(cd "$wt" && git add -A && git commit -q -m "feat")

result="$(tf_cross_vendor_verify CV1 "$wt" verifier)"
tf_assert_eq "returns PASS" "PASS" "$result"
tf_assert "cross-verify log exists" test -f "$TF_LOG_DIR/CV1.cross-verify.log"
tf_group_end

# ---- tf_cross_vendor_verify with mocked pi (FAIL) ----
tf_group_begin; tf_test "tf_cross_vendor_verify returns FAIL when pi exits non-zero"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV2","engine":"t","title":"T","section":"§1","deps":[],"scope":["src/main.rs"],"accept":"true","manual":false,"verify_worker":"verifier"}
]}
JSON
tf_status_init

cat > "$SBOX/pi" <<'SCRIPT'
#!/usr/bin/env bash
echo "Review failed"
exit 1
SCRIPT
chmod +x "$SBOX/pi"

mkdir -p "$TF_REPO_DIR/../wt-cv2/src"
wt="$TF_REPO_DIR/../wt-cv2"
(cd "$wt" && git init -q && git checkout -b tf/CV2 -q)
echo "fn bad() {}" > "$wt/src/main.rs"
(cd "$wt" && git add -A && git commit -q -m "bad feat")

result="$(tf_cross_vendor_verify CV2 "$wt" verifier)"
tf_assert_re "returns FAIL" "^FAIL:" "$result"
tf_group_end

# ---- tf_cross_vendor_verify timeout handling ----
tf_group_begin; tf_test "tf_cross_vendor_verify returns FAIL on timeout"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV3","engine":"t","title":"T","section":"§1","deps":[],"scope":["src/main.rs"],"accept":"true","manual":false,"verify_worker":"verifier"}
]}
JSON
tf_status_init

cat > "$SBOX/pi" <<'SCRIPT'
#!/usr/bin/env bash
sleep 60
exit 0
SCRIPT
chmod +x "$SBOX/pi"

mkdir -p "$TF_REPO_DIR/../wt-cv3/src"
wt="$TF_REPO_DIR/../wt-cv3"
(cd "$wt" && git init -q && git checkout -b tf/CV3 -q)
echo "fn main() {}" > "$wt/src/main.rs"
(cd "$wt" && git add -A && git commit -q -m "init")

result="$(tf_cross_vendor_verify CV3 "$wt" verifier 2>&1)"
tf_assert_contains "FAIL mentions timeout" "timed out" "$result"
tf_group_end

# ---- cross-vendor verify prompt contains task info ----
tf_group_begin; tf_test "cross-vendor verify prompt contains key sections"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV4","engine":"t","title":"Implement parser","section":"§2","deps":[],"scope":["src/parser.rs"],"accept":"cargo test","manual":false,"verify_worker":"verifier","acceptance_prose":"Parser must handle nested parentheses"}
]}
JSON
tf_status_init

cat > "$SBOX/pi" <<'SCRIPT'
#!/usr/bin/env bash
exit 0
SCRIPT
chmod +x "$SBOX/pi"

mkdir -p "$TF_REPO_DIR/../wt-cv4/src"
wt="$TF_REPO_DIR/../wt-cv4"
(cd "$wt" && git init -q && git checkout -b tf/CV4 -q)
echo "struct Parser;" > "$wt/src/parser.rs"
(cd "$wt" && git add -A && git commit -q -m "parser")

tf_cross_vendor_verify CV4 "$wt" verifier >/dev/null 2>&1 || true

prompt_file="$TF_STATE_DIR/CV4.cross-verify.md"
tf_assert "prompt file exists" test -f "$prompt_file"
tf_assert_contains "prompt has task spec" "Parser must handle nested parentheses" "$(cat "$prompt_file")"
tf_assert_contains "prompt has review instructions" "code reviewer" "$(cat "$prompt_file")"
tf_group_end

# ---- cross-vendor verify log has correct headers ----
tf_group_begin; tf_test "cross-vendor verify log has worker details"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"CV5","engine":"t","title":"T","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false,"verify_worker":"verifier"}
]}
JSON
tf_status_init

cat > "$SBOX/pi" <<'SCRIPT'
#!/usr/bin/env bash
echo "looks good"
exit 0
SCRIPT
chmod +x "$SBOX/pi"

mkdir -p "$TF_REPO_DIR/../wt-cv5"
wt="$TF_REPO_DIR/../wt-cv5"
(cd "$wt" && git init -q && git checkout -b tf/CV5 -q)
echo "fn main() {}" > "$wt/src/main.rs"
mkdir -p "$wt/src"
(cd "$wt" && git add -A && git commit -q -m "init")

tf_cross_vendor_verify CV5 "$wt" verifier >/dev/null 2>&1 || true
log_content="$(cat "$TF_LOG_DIR/CV5.cross-verify.log" 2>/dev/null || echo '')"
tf_assert_contains "log has verify_worker" "verifier" "$log_content"
tf_assert_contains "log has provider" "anthropic" "$log_content"
tf_assert_contains "log has model" "claude" "$log_content"
tf_group_end

# Restore PATH
export PATH="$(echo "$PATH" | tr ':' '\n' | grep -v "^$SBOX$" | tr '\n' ':')"

tf_test_summary
