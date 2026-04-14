#!/usr/bin/env bash
# test-commitlint.sh — exercises QUA-22 acceptance criteria
set -euo pipefail

PASS=0
FAIL=0

run_test() {
  local description="$1"
  local expect_exit="$2" # 0 or nonzero
  local input="$3"

  if output=$(echo "$input" | pnpm exec commitlint 2>&1); then
    actual_exit=0
  else
    actual_exit=$?
  fi

  if [[ "$expect_exit" == "0" && "$actual_exit" -eq 0 ]]; then
    echo "PASS: $description"
    ((PASS++))
  elif [[ "$expect_exit" == "nonzero" && "$actual_exit" -ne 0 ]]; then
    echo "PASS: $description"
    ((PASS++))
  else
    echo "FAIL: $description (expected exit=$expect_exit, got exit=$actual_exit)"
    echo "  output: $output"
    ((FAIL++))
  fi
}

run_test_multiline() {
  local description="$1"
  local expect_exit="$2"
  local input="$3"

  if output=$(printf '%s' "$input" | pnpm exec commitlint 2>&1); then
    actual_exit=0
  else
    actual_exit=$?
  fi

  if [[ "$expect_exit" == "0" && "$actual_exit" -eq 0 ]]; then
    echo "PASS: $description"
    ((PASS++))
  elif [[ "$expect_exit" == "nonzero" && "$actual_exit" -ne 0 ]]; then
    echo "PASS: $description"
    ((PASS++))
  else
    echo "FAIL: $description (expected exit=$expect_exit, got exit=$actual_exit)"
    echo "  output: $output"
    ((FAIL++))
  fi
}

echo "=== QUA-22 Commitlint Acceptance Tests ==="
echo ""

# AC1: existing repo history passes
echo "--- AC1: existing repo history ---"
if output=$(pnpm exec commitlint --from HEAD~1 2>&1); then
  echo "PASS: pnpm exec commitlint --from HEAD~1 exits 0"
  ((PASS++))
else
  echo "FAIL: pnpm exec commitlint --from HEAD~1 should exit 0 (got $?)"
  echo "  output: $output"
  ((FAIL++))
fi

# AC2: missing scope rejected
echo ""
echo "--- AC2: missing scope ---"
run_test "feat: missing scope → rejected" "nonzero" "feat: missing scope"

# AC3: wrong scope format rejected
echo ""
echo "--- AC3: wrong scope format ---"
run_test "feat(login): wrong scope format → rejected" "nonzero" "feat(login): wrong scope format"

# AC4: unknown type rejected
echo ""
echo "--- AC4: unknown type ---"
run_test "hack(QUA-1): unknown type → rejected" "nonzero" "hack(QUA-1): unknown type"

# AC5: missing Project footer rejected
echo ""
echo "--- AC5: missing Project footer ---"
run_test_multiline "feat(QUA-1): add thing (no footer) → rejected" "nonzero" "feat(QUA-1): add thing

"

# AC6: valid message with Project footer accepted
echo ""
echo "--- AC6: valid message with Project footer ---"
run_test_multiline "feat(QUA-1): add thing with Project footer → accepted" "0" "feat(QUA-1): add thing

Project: P-42"

# AC7: bugfix type accepted
echo ""
echo "--- AC7: bugfix type ---"
run_test_multiline "bugfix(QUA-1): fix thing with Project footer → accepted" "0" "bugfix(QUA-1): fix thing

Project: P-42"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
