# Code Review Skill

Reviews a GitHub PR using a parallel agent team. Spawns specialist reviewers that examine the diff, self-critique findings, and report back. Produces a structured review with findings organized by priority tier and a binary verdict.

## Workflow

### Phase 1: Fetch PR Data

1. Parse the PR URL or number from the user's input
2. Fetch PR metadata:
   ```bash
   gh pr view <N> --repo <owner/repo> --json title,body,baseRefName,headRefName,files,additions,deletions
   ```
3. Fetch the full diff:
   ```bash
   gh pr diff <N> --repo <owner/repo>
   ```
4. If the diff is very large (>5000 lines), also read key files directly to have complete context

### Phase 2: Scope Assessment & Reviewer Selection

Assess the PR size and select reviewers accordingly:

| Size | Changed Lines | Files | Reviewers |
|------|--------------|-------|-----------|
| Small | <50 | <5 | 2: completeness-reviewer, complexity-reviewer |
| Medium | 50-299 | 5-15 | 3: completeness-reviewer, complexity-reviewer, operations-reviewer |
| Large | 300+ | 15+ | 4: feasibility-reviewer, complexity-reviewer, completeness-reviewer, operations-reviewer |

For spec reviews or architecture changes, also consider: api-reviewer, clarity-reviewer, scope-reviewer, product-reviewer.

### Phase 3: Team Setup & Parallel Review

1. Create a team: `team_create` with a descriptive name (e.g., `code-review-pr-<N>`)
2. Create team tasks for each reviewer: `team_task` with operation=create
3. Spawn all reviewers **in parallel** using `task` with:
   - `background: true`
   - `team_id: <team_id>`
   - `subagent_type`: one of the reviewer agent types listed above
   - `prompt`: Include the full diff, PR description, and specific review instructions

IMPORTANT: Spawn ALL reviewers in a single message (multiple tool calls in parallel). Do NOT spawn them one at a time.

Each reviewer's prompt MUST include:
- The complete PR diff (paste it directly into the prompt)
- The PR title and description for context
- Their specific review focus area
- Instructions to use tools (read, grep, glob) to examine the codebase for context
- Instructions to send their findings back to the lead via `send_message` when done

### Phase 4: Collect & Synthesize

After spawning all reviewers, the background agents will complete their reviews and their results will be automatically delivered to you as messages. Once you receive results from all reviewers:

1. Collect all findings from the reviewer messages
2. Deduplicate similar findings (same file + same concern = duplicate)
3. Sort by priority: Critical > High > Medium > Low
4. Determine verdict:
   - **REQUEST_CHANGES** if any critical or high-priority actionable findings exist
   - **APPROVE** otherwise

### Phase 5: Format Report

Output a structured report:

```
## Code Review: PR #<N> -- <title>

**Verdict: APPROVE / REQUEST_CHANGES**

### Critical Findings
- [file:line] Description of the issue and suggested fix

### High Priority
- ...

### Medium Priority
- ...

### Low Priority / Suggestions
- ...

### Summary
<1-2 paragraph synthesis>
```

## Important Notes

- Always use `gh` CLI for GitHub operations (assume the user has it installed and authenticated)
- If the user provides a full PR URL like `https://github.com/owner/repo/pull/N`, extract the owner/repo and PR number from it
- Spawn reviewers in parallel -- all in the same response with multiple tool calls
- Each reviewer should examine the actual codebase (not just the diff) for context
- Wait for ALL reviewers to complete before synthesizing -- their messages will arrive automatically
- If a reviewer fails or times out, note it in the report but proceed with available results
- The review should be actionable: every finding should suggest a specific fix
- After the review is complete, disband the team with `team_delete`
