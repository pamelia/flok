# Spec Review Skill

Orchestrates a parallel spec review using an agent team. Specialists each review the spec from their domain, self-critique findings, and report back. Produces a structured review with a binary approval verdict.

## Workflow

### Phase 1: Read the Spec

1. Read the spec file (the user will provide the path or it will be in `docs/specs/`)
2. Assess the spec's scope and complexity
3. Select appropriate reviewer specialists

### Phase 2: Select Reviewers

Based on the spec content, select 3-5 specialists from:

| Specialist | When to Include |
|-----------|----------------|
| feasibility-reviewer | Always -- checks technical feasibility |
| complexity-reviewer | Always -- checks for over-engineering |
| completeness-reviewer | Always -- checks for missing pieces |
| api-reviewer | When the spec defines APIs or interfaces |
| clarity-reviewer | When the spec has complex requirements |
| scope-reviewer | When the spec is large or has delivery risk |
| product-reviewer | When the spec has user-facing impact |
| operations-reviewer | When the spec has deployment/infra concerns |

### Phase 3: Parallel Review

1. Create a team: `team_create` with name `spec-review-<spec-name>`
2. Spawn all selected reviewers in parallel with `task(background: true, team_id: ...)`
3. Each reviewer's prompt should include the full spec content and instructions to:
   - Review from their specialist perspective
   - Self-critique: remove speculative findings
   - Send findings back to lead via `send_message`

### Phase 4: Synthesize

After all reviewers report back:
1. Deduplicate overlapping findings
2. Sort by priority
3. Determine verdict: APPROVE or REQUEST_CHANGES

### Phase 5: Output

```
## Spec Review: <spec name>

**Verdict: APPROVE / REQUEST_CHANGES**

### Findings by Priority
...

### Summary
...
```
