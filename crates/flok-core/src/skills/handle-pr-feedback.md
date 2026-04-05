# Handle PR Feedback Skill

Reads unresolved review comments on a GitHub PR, triages each one, makes code changes, and replies to every comment with the action taken or reason for skipping.

## Workflow

### Phase 1: Fetch Review Comments

Fetch inline review comments:
```bash
gh api repos/<owner>/<repo>/pulls/<N>/comments --jq '.[] | select(.position != null) | {id: .id, path: .path, line: .line, body: .body, user: .user.login}'
```

Also fetch review-level comments:
```bash
gh api repos/<owner>/<repo>/pulls/<N>/reviews --jq '.[] | select(.state == "CHANGES_REQUESTED" or .state == "COMMENTED") | {id: .id, body: .body, user: .user.login}'
```

### Phase 2: Triage Each Comment

For each comment, decide using these criteria:

| Decision | When | Action |
|----------|------|--------|
| **Address** | The comment points out a real bug, missing test, security issue, or reasonable improvement | Apply the fix |
| **Answer** | The comment is a question or asks for clarification | Reply with a clear answer |
| **Respectfully decline** | The comment is a style preference you disagree with, or the suggested change would break something | Reply explaining why, with evidence |
| **Defer** | The comment is valid but out of scope for this PR | Reply acknowledging and suggesting a follow-up |

**Triage rules:**
- Comments from the repo owner or maintainers carry extra weight
- If multiple reviewers raise the same concern, address it
- Security-related comments are always "address"
- Don't dismiss comments just because they're inconvenient
- When declining, provide a specific technical reason, not just "I prefer it this way"

### Phase 3: Apply Fixes

For each comment marked "address":

1. Read the relevant file and understand the full context (not just the commented line)
2. Apply the fix using `edit`
3. Verify the change compiles and tests pass
4. If the fix requires changes in other files (e.g., updating callers), make those too

**Scope discipline for fixes:**
- Fix what the comment asks for. Don't expand the fix into a broader refactor.
- If the comment reveals a deeper issue, note it but don't fix the deeper issue in this pass.
- If two comments conflict with each other, note the conflict and ask the user which to follow.

### Phase 4: Reply to Comments

For every comment (not just addressed ones), reply:

- **If addressed**: Describe what was changed and why the approach was chosen
  ```
  Fixed -- added nil check before accessing `user.email`. Also added a test
  for the nil case in user_service_test.rs.
  ```
- **If answered**: Provide a clear, direct answer
- **If respectfully declined**: Explain the technical reason with evidence
  ```
  Keeping the current approach because changing to X would break the
  existing API contract (see callers in handler.rs:45 and handler.rs:89).
  Happy to discuss further.
  ```
- **If deferred**: Acknowledge the validity and commit to follow-up
  ```
  Good catch -- this is a real issue but fixing it properly requires
  refactoring the auth middleware (out of scope for this PR). Filed as
  issue #<N> to track separately.
  ```

Use the GitHub API to reply:
```bash
gh api repos/<owner>/<repo>/pulls/<N>/comments/<comment_id>/replies -f body="<reply>"
```

### Phase 5: Push Changes

After all fixes are applied and verified:

1. Run the full test suite one final time
2. Commit with a descriptive message: `address PR review feedback: <summary of changes>`
3. Push to the branch

### Output

Provide a structured summary:

```
## PR Feedback Handled: PR #<N>

### Addressed (X comments)
- [file:line] <comment summary> -- <what was changed>

### Answered (X comments)
- <question summary> -- <answer given>

### Declined (X comments)
- <comment summary> -- <reason for declining>

### Deferred (X comments)
- <comment summary> -- <follow-up plan>

### Tests
- All tests pass: yes/no
- New tests added: <list>
```

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "This comment is just a nitpick, I'll ignore it" | Reply to every comment, even nitpicks. Ignoring comments is disrespectful and leaves the reviewer uncertain. |
| "I'll batch all the fixes and push at the end" | Verify after each fix. A fix that breaks tests contaminates all subsequent fixes. |
| "The reviewer doesn't understand my approach" | Maybe. Or maybe your approach has a flaw you can't see. Engage with the substance, not the person. |
| "I'll just do what they say to get the PR merged" | If you disagree, say so respectfully. Blindly applying suggestions you think are wrong creates worse code. |
| "This is too many comments, the reviewer is being unreasonable" | Many comments often means the PR is too large. Consider splitting it. |

## Red Flags

- Comments left without any reply
- "Fixed" as the entire reply (no description of what changed)
- Fixes applied without running tests afterward
- Declining comments without a technical justification
- Addressing comments by making unrelated changes to appease the reviewer
- Pushing all changes in one commit with no description
- Ignoring comments from maintainers or security reviewers

## Verification

After handling all feedback:

- [ ] Every comment has a reply (addressed, answered, declined, or deferred)
- [ ] All applied fixes have passing tests
- [ ] Declined comments have specific technical justifications
- [ ] Deferred comments have a follow-up plan (issue filed or task noted)
- [ ] Changes are committed with a descriptive message
- [ ] The push succeeded and CI is green (or checked)
