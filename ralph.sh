#!/bin/bash
set -e
for ((i=1; i<=20; i++)); do
  echo "Iteration #$i"
  result=$(claude --permission-mode acceptEdits -p "@progress.txt \\ 
  1. Pick highest priority project in Linear.
  1. Find the highest-priority, non-blocked task in picked project. Immediately set status to in progress and assign first person in the team.
  2. Implement the code using red-green-refactor cycle in TDD methodology. Respect ADRs inside docs/architecture/adrs. Work on a branch with feat/<issue-id>-<short title> name format.\\
  3. Update progress.txt. \\
  4. Create a pull request with changes. \\
  If there are no remaining tasks in Linear, output <promise>COMPLETE</promise>. ")
  
  echo "$result"
  if [[ "$result" == *"<promise>COMPLETE</promise>"* ]]; then
    exit 0
  fi
done
