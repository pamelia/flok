# Handle PR Feedback Skill

Reads unresolved review comments on a GitHub PR, triages each one, makes code changes, and replies to every comment with the action taken or reason for skipping.

## Workflow

### Phase 1: Fetch Review Comments

```bash
gh api repos/<owner>/<repo>/pulls/<N>/comments --jq '.[] | select(.position != null) | {id: .id, path: .path, line: .line, body: .body, user: .user.login}'
```

Also fetch review-level comments:
```bash
gh api repos/<owner>/<repo>/pulls/<N>/reviews --jq '.[] | select(.state == "CHANGES_REQUESTED" or .state == "COMMENTED") | {id: .id, body: .body, user: .user.login}'
```

### Phase 2: Triage Each Comment

For each comment, decide:
- **Address**: The comment points out a real issue, missing test, bug, or reasonable improvement
- **Skip**: The comment is a question (answer it), a nitpick you disagree with, or already addressed

### Phase 3: Apply Fixes

For each comment marked "address":
1. Read the relevant file
2. Understand the context around the commented line
3. Apply the fix using `edit`
4. Verify the change makes sense

### Phase 4: Reply to Comments

For each comment:
- If addressed: reply with what was changed and why
- If skipped: reply with a clear explanation of why (respectfully)

Use the GitHub API to reply:
```bash
gh api repos/<owner>/<repo>/pulls/<N>/comments/<comment_id>/replies -f body="<reply>"
```

### Phase 5: Push Changes

```bash
git add -A && git commit -m "address PR review feedback" && git push
```

### Output

Summary of:
- How many comments were addressed vs skipped
- What changes were made
- Any comments that need human judgment
