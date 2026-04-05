# Self-Review Loop Skill

Iterative self-review loop for PRs. Launches a fresh review each turn, evaluates feedback, applies fixes, and re-reviews. Loops until only minor/nit feedback remains or 5 turns complete.

## Workflow

### Setup

1. Determine the base branch (default: `main` or `master`)
2. Get the current diff: `git diff origin/<base>...HEAD`
3. Count changed lines to gauge scope

### Loop (max 5 turns)

For each turn:

1. **Review**: Load the `code-review` skill and run a full review against the current diff
2. **Triage**: Categorize each finding using the decision criteria below
3. **Fix**: Apply fixes for "address" findings, one at a time
4. **Verify**: After each fix, run the project's test/build commands to confirm nothing broke
5. **Check for oscillation**: Detect if fixes are going back and forth (see below)
6. **Re-evaluate**: Get the new diff and assess whether remaining findings are all minor

### Triage Decision Criteria

For each finding from the review, decide:

| Decision | When | Examples |
|----------|------|---------|
| **Address** | Real bugs, security issues, missing error handling, high-priority suggestions | Null dereference, missing validation, resource leak, incorrect logic |
| **Skip** | Style preferences, nitpicks, low-priority risks, speculative concerns | Variable naming opinion, formatting, "consider adding" suggestions |
| **Defer** | Valid but out-of-scope for this PR, requires broader changes | Architectural refactors, cross-cutting concerns, unrelated tech debt |

**Rules for triage:**
- Critical and high-priority findings are always "address"
- If a finding requires touching files not in the current diff, it's "defer"
- If two reviewers flag the same concern, it's more likely worth addressing
- When in doubt, address it -- false negatives are worse than false positives in reviews

### Oscillation Detection

The loop is oscillating when fixes undo previous fixes. Detect this by tracking:

1. **File overlap**: If >50% of files changed in turn N overlap with files changed in turn N-2, the loop is oscillating
2. **Finding recurrence**: If a finding from turn N-2 reappears in turn N after being "fixed", stop
3. **Net change direction**: If the diff size is growing rather than shrinking across turns, the fixes are adding complexity

When oscillation is detected, stop the loop and report the conflicting changes. The human needs to make a judgment call.

### Stop Conditions

Stop the loop when ANY of these are true:
- The review comes back clean (APPROVE with no critical/high findings)
- 5 turns have been completed
- Oscillation detected (fixing the same files repeatedly)
- Only nitpicks and low-priority suggestions remain
- The fixes would require changes outside the scope of this PR

### Output

Report the final state with this structure:

```
## Self-Review Results

**Turns completed:** N of 5
**Final verdict:** APPROVE / REQUEST_CHANGES (requires human judgment)

### Fixes Applied
- [Turn 1] file:line -- Description of fix
- [Turn 2] file:line -- Description of fix

### Intentionally Skipped
- [Finding] -- Reason for skipping (nitpick / out-of-scope / style preference)

### Deferred
- [Finding] -- Reason for deferring (requires broader changes / separate PR)

### Remaining Concerns
- [Any findings that couldn't be resolved within the loop]
```

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "The review is being too picky, I'll skip most findings" | Picky reviews catch real bugs. Address critical and high findings; only skip genuine nitpicks. |
| "I'll just do one more turn to get it perfect" | Diminishing returns after 3-4 turns. If it's not clean by turn 5, it needs human judgment. |
| "The oscillation is making progress, not going in circles" | If the same files keep changing, the approach is wrong. Stop and rethink. |
| "These findings are all style issues" | Re-read them. 'Style' findings sometimes mask correctness issues (e.g., confusing naming hides a logic bug). |

## Red Flags

- Skipping all findings from a turn without individually evaluating each one
- Addressing findings without running tests after each fix
- The diff growing larger with each turn instead of converging
- Ignoring oscillation and continuing to loop
- Applying fixes that touch files outside the PR's scope
- No structured output at the end -- just "looks good now"

## Verification

After the loop completes:

- [ ] Every finding was individually triaged (address/skip/defer) with a reason
- [ ] All applied fixes have passing tests
- [ ] The final diff is reviewed and makes sense as a coherent change
- [ ] Skipped and deferred findings are documented with rationale
- [ ] No oscillation occurred (or was caught and stopped early)
