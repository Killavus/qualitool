#!/usr/bin/env bash
# test-commit-msg-hook.sh — exercises QUA-24 acceptance criteria
# Tests that the husky commit-msg hook blocks/allows real git commits.
set -euo pipefail

PASS=0
FAIL=0

# Record starting HEAD so we can detect whether a commit was created.
start_head=$(git rev-parse HEAD)

assert_commit_rejected() {
  local description="$1"
  shift
  local head_before
  head_before=$(git rev-parse HEAD)

  if git commit "$@" 2>/dev/null; then
    echo "FAIL: $description — commit was NOT rejected (new HEAD: $(git rev-parse HEAD))"
    # Undo the unwanted commit so later tests aren't affected.
    git reset --soft HEAD~1 >/dev/null 2>&1 || true
    ((FAIL++))
    return
  fi

  local head_after
  head_after=$(git rev-parse HEAD)
  if [[ "$head_before" == "$head_after" ]]; then
    echo "PASS: $description"
    ((PASS++))
  else
    echo "FAIL: $description — HEAD moved even though commit should have been rejected"
    git reset --soft HEAD~1 >/dev/null 2>&1 || true
    ((FAIL++))
  fi
}

assert_commit_accepted() {
  local description="$1"
  shift
  local head_before
  head_before=$(git rev-parse HEAD)

  if git commit "$@" 2>/dev/null; then
    local head_after
    head_after=$(git rev-parse HEAD)
    if [[ "$head_before" != "$head_after" ]]; then
      echo "PASS: $description"
      ((PASS++))
      # Undo the test commit so we leave the branch clean.
      git reset --soft HEAD~1 >/dev/null 2>&1 || true
    else
      echo "FAIL: $description — git commit exited 0 but HEAD didn't move"
      ((FAIL++))
    fi
  else
    echo "FAIL: $description — commit was rejected (should have been accepted)"
    ((FAIL++))
  fi
}

echo "=== QUA-24 commit-msg hook integration tests ==="
echo ""

# --- AC1: hook file exists ---
echo "--- AC1: .husky/commit-msg exists ---"
if [[ -f .husky/commit-msg ]]; then
  echo "PASS: .husky/commit-msg exists"
  ((PASS++))
else
  echo "FAIL: .husky/commit-msg does not exist"
  ((FAIL++))
fi

# --- AC2: missing scope rejected ---
echo ""
echo "--- AC2: missing scope → commit rejected ---"
assert_commit_rejected \
  "git commit -m 'fix: no scope' is rejected" \
  --allow-empty -m "fix: no scope"

# --- AC3: missing Project footer rejected ---
echo ""
echo "--- AC3: missing Project footer → commit rejected ---"
assert_commit_rejected \
  "git commit -m 'feat(QUA-1): test' (no Project footer) is rejected" \
  --allow-empty -m "feat(QUA-1): test"

# --- AC4: fully valid message accepted ---
echo ""
echo "--- AC4: valid message → commit accepted ---"
assert_commit_accepted \
  "git commit with valid message succeeds" \
  --allow-empty -m "$(printf 'feat(QUA-1): test\n\nProject: P-1')"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
