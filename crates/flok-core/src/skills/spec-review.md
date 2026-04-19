# Spec Review Skill

Orchestrates a parallel spec review using an agent team. Specialists each review the spec from their domain, self-critique findings, and report back. Includes a cross-review phase where specialists challenge each other's findings. Produces a structured review with a binary approval verdict.

## Workflow

### Phase 1: Read the Spec

1. Read the spec file (the user will provide the path or it will be in `docs/specs/`)
2. Assess the spec's scope and complexity:
   - How many systems/components does it touch?
   - Does it define APIs or interfaces?
   - Does it have user-facing impact?
   - Does it have deployment/infra concerns?
3. Identify the spec's core claims -- what it promises to deliver and how

### Phase 2: Select Reviewers

Based on the spec content, select 3-5 specialists from:

| Specialist | When to Include | What They Check |
|-----------|----------------|-----------------|
| feasibility-reviewer | Always | Technical soundness, can this actually be built as described? |
| complexity-reviewer | Always | Over-engineering, could this be simpler? |
| completeness-reviewer | Always | Missing error handling, edge cases, migration paths |
| api-reviewer | When the spec defines APIs or interfaces | API surface, breaking changes, naming consistency |
| clarity-reviewer | When the spec has complex requirements | Ambiguity, contradictions, testable acceptance criteria |
| scope-reviewer | When the spec is large or has delivery risk | Feature deferral, hidden complexity, timeline risk |
| product-reviewer | When the spec has user-facing impact | User value, UX issues, root cause alignment |
| operations-reviewer | When the spec has deployment/infra concerns | Deployment safety, observability, security |

After selecting specialists, you will spawn EACH selected specialist ONCE PER CONFIGURED PROVIDER. For example, if 4 specialists are selected and 2 providers (Anthropic, OpenAI) are configured, you will spawn 8 sub-agents in total.

### Phase 3: Parallel Review

1. Create a team: `team_create` with name `spec-review-<spec-name>`
2. Spawn all selected reviewers in parallel with `task(background: true, team_id: ...)`
3. Each reviewer's prompt should include the full spec content and instructions to:
   - Review from their specialist perspective
   - For each finding, state: the specific concern, where in the spec it appears, the impact, and a suggested resolution
   - Self-critique: remove speculative findings that lack evidence from the spec
   - Send findings back to lead via `send_message`

### Cross-Coverage Multi-Model Fan-Out

Check your system prompt for the "Available Providers" section. For each configured provider, you MUST spawn every selected specialist.

Example for 2 selected specialists (feasibility, clarity) + 2 providers (Anthropic, OpenAI):

```
task(subagent_type: "feasibility-reviewer", model: "opus", background: true, team_id: "...", prompt: "...")
task(subagent_type: "feasibility-reviewer", model: "gpt-5.4", background: true, team_id: "...", prompt: "...")
task(subagent_type: "clarity-reviewer", model: "opus", background: true, team_id: "...", prompt: "...")
task(subagent_type: "clarity-reviewer", model: "gpt-5.4", background: true, team_id: "...", prompt: "...")
```

All agents run in parallel. If only ONE provider is configured, spawn each specialist once without a `model` parameter.

IMPORTANT: Spawn ALL reviewers in a single message. Do NOT spawn them one at a time.

### Phase 4: Cross-Review

After all reviewers report back, check for conflicting findings:

- If two specialists disagree (e.g., feasibility says "too simple" while complexity says "too complex"), note the tension and present both perspectives
- If multiple specialists flag the same concern from different angles, elevate its priority
- Remove duplicate findings (same section + same concern = duplicate)

**Cross-review challenges are single-model**: when challenging a finding, spawn the challenger on the same model the original reviewer used. Only Phase 3 (initial review) uses cross-coverage multi-model fan-out.

### Phase 5: Synthesize & Output

1. Deduplicate overlapping findings
2. Sort by priority: Critical > High > Medium > Low
3. Determine verdict:
   - **REQUEST_CHANGES** if any critical or high-priority findings exist that would cause implementation failure or user harm
   - **APPROVE** otherwise (minor gaps can be addressed during implementation)

### Cross-Model Synthesis

Each finding arrives tagged with its reviewer AND the model that produced it. Synthesize as follows:

1. **Group findings by topic**: Findings from different models about the same spec section and concern are the SAME finding.

2. **Classify confidence**:
   - **HIGH CONFIDENCE**: Flagged by the same specialist on BOTH providers. Report first.
   - **MODEL-SPECIFIC**: Flagged only by one provider. Report, but label with the model (e.g., "[Only flagged by anthropic/opus-4.7]").
   - **CONTRADICTORY**: One provider says "this is a bug", the other says "this is intentional". Flag for human judgment.

3. **In the final report**, include a summary line: "Reviewed by N specialists × M providers = K total agents. X findings with cross-model agreement, Y model-specific findings, Z contradictions."

Output format:

```
## Spec Review: <spec name>

**Verdict: APPROVE / REQUEST_CHANGES**

### Critical Findings
- [Section] Description and suggested resolution (Provider: <model>)

### High Priority
- [Section] Description and suggested resolution

### Medium Priority
- [Section] Description

### Low Priority / Suggestions
- [Section] Description

### Cross-Review Tensions
- [Specialist A vs B] Description of the disagreement and both perspectives

### Summary
<1-2 paragraph synthesis of the spec's readiness>
Reviewed by N specialists × M providers = K total agents. X findings with cross-model agreement, Y model-specific findings, Z contradictions.
```

After the review, disband the team with `team_delete`.

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "The spec is good enough to start coding" | A spec with critical gaps leads to rework. 30 minutes of review prevents days of wrong implementation. |
| "We'll figure out the details during implementation" | That's what the spec is for. Missing details in the spec become bugs in the code. |
| "This is just a spec, not code -- it doesn't need rigorous review" | The spec is the foundation. A flawed foundation produces flawed code. |
| "The reviewers are being too theoretical" | Check if findings have concrete impact. If a finding can't describe a specific failure mode, it's too theoretical. |

## Red Flags

- Spec approved without any reviewers examining it
- Only one specialist perspective used (missing dimensions of review)
- Findings without suggested resolutions (identifying problems without solutions is incomplete)
- Critical findings hand-waved as "we'll handle it later"
- Spec has no success criteria or acceptance tests
- Spec describes a solution without stating the problem it solves
- Cross-review tensions ignored rather than surfaced

## Verification

Before approving a spec:

- [ ] At least 3 specialist perspectives examined the spec
- [ ] All critical findings have specific suggested resolutions
- [ ] Success criteria are concrete and testable
- [ ] Cross-review tensions are surfaced and documented
- [ ] The spec describes both what will be built AND what won't (scope boundaries)
- [ ] The verdict is justified with reference to specific findings
