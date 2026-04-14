#!/bin/bash
set -e
for ((i=1; i<=20; i++)); do
  echo "Iteration #$i"
  result=$(claude --permission-mode acceptEdits --verbose --model opus -p "@progress.txt \\ 
  1. Select the highest priority project in Linear.
  2. Find the highest-priority, non-blocked, non-assigned, todo or backlog task in picked project. Immediately set status to in progress and assign first person in the team. Select the base branch based on the dependency structure of the issue on Linear.
  3. Implement the code using red-green-refactor cycle in TDD methodology. Respect ADRs inside docs/architecture/adrs. Work on a branch with feat/<issue-id>-<short title> name format.\\
  4. Update progress.txt. \\
  5. Create a pull request with changes use @docs/PR_TEMPLATE.md as a template. \\
  6. If there are no remaining tasks in Linear, output <promise>COMPLETE</promise>. ")
  
  echo "$result"
  if [[ "$result" == *"<promise>COMPLETE</promise>"* ]]; then
    exit 0
  fi
done
