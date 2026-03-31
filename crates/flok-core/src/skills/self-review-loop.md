# Self-Review Loop Skill

Iterative self-review loop for PRs. Launches a fresh review each turn, evaluates feedback, applies fixes, and re-reviews. Loops until only minor/nit feedback remains or 5 turns complete.

## Workflow

### Setup

1. Determine the base branch (default: `main` or `master`)
2. Get the current diff: `git diff origin/<base>...HEAD`

### Loop (max 5 turns)

For each turn:

1. **Review**: Load the `code-review` skill and run a full review against the current diff
2. **Triage**: For each finding:
   - **Address**: Critical bugs, high-priority suggestions, security issues
   - **Skip**: Nitpicks, style preferences, questions, thoughts, low-priority risks
3. **Fix**: For each finding marked "address":
   - Read the relevant file
   - Apply the fix using `edit` or `write`
   - Verify the fix compiles/passes basic checks
4. **Check for oscillation**: If >50% of changed files overlap with 2 turns ago, stop (the fixes are going back and forth)
5. **Re-evaluate**: Get the new diff and check if the remaining findings are all minor

### Stop Conditions

Stop the loop when ANY of these are true:
- The review comes back clean (APPROVE with no critical/high findings)
- 5 turns have been completed
- Oscillation detected (fixing the same files repeatedly)
- Only nitpicks and low-priority suggestions remain

### Output

Report the final state:
- How many turns were needed
- What was fixed
- What was intentionally skipped (and why)
- The final review verdict
