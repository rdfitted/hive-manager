# Plan: Evaluator Peer Architecture (GAN-Inspired Quality Loop)

**Date:** 2026-03-29
**Status:** Draft
**Source:** Anthropic engineering blog — "Harness Design for Long-Running Apps" + session discussion

---

## Summary

Introduce an **Evaluator** as a peer authority to the Queen in Hive sessions. The Evaluator owns quality assurance — managing its own team of QA workers, grading milestones against negotiated acceptance criteria, and gating progress through an adversarial feedback loop inspired by GAN (Generator/Discriminator) architecture.

This transforms Hive from a build tool into a build + verify tool where product-level quality is enforced automatically, not just code-level quality via PR review.

---

## Motivation

### Current State
- Queen is the sole authority: plans, delegates, commits, manages Workers and Code Reviewers
- Quality gate: Code Reviewers check PRs for code quality (static analysis, patterns, correctness)
- Gap: No one verifies the **running app** actually works from a user perspective
- A PR can pass code review but the feature is broken when you click through it

### What the Anthropic Article Demonstrates
- Separate evaluator agents are more effective than self-evaluation (agents praise their own mediocre work)
- Playwright-based live app testing catches what code review misses
- Sprint contracts with gradable criteria convert subjective quality into measurable pass/fail
- GAN dynamic (generator vs discriminator) produces dramatically better output than solo agents
- Results: Solo agent = 20 min/$9/broken. Full harness = 6 hrs/$200/fully working.

### Key Insight
The Evaluator must be a **peer** to the Queen, not a subordinate. Each preserves their own context and manages their own team. The Queen doesn't waste context on QA history; the Evaluator doesn't waste context on implementation details.

---

## Architecture

### Bicameral Hive Structure

```
              Master Planner
             ↙              ↘
         Queen                Evaluator
       (execution)           (quality)
        ↓  ↓  ↓               ↓  ↓  ↓
       W1  W2  W3           QA1 QA2 QA3
       Code Reviewers       (UI, API, a11y, perf)
```

### Role Definitions

**Queen (Execution Authority)**
- Preserves the vision and strategic context
- Manages: Planners, Workers, Code Reviewers
- Controls: commits and PRs
- Focus: "Are we building the right thing? Is the code correct?"

**Evaluator (Quality Authority)**
- Preserves quality standards and acceptance criteria
- Manages: QA Workers (UI, API, accessibility, performance testers)
- Controls: milestone approval gate
- Focus: "Does this actually work? Is it good?"

Neither can override the other. They negotiate.

### Evaluator's Team

| QA Worker Role | Tools | Responsibility |
|----------------|-------|----------------|
| UI Tester | Playwright MCP, screenshots | Click through flows, grade visual quality, test interactions |
| API Tester | HTTP client, curl | Hit endpoints, validate responses, check error handling |
| Accessibility Tester | axe-core, Lighthouse a11y | Contrast, keyboard nav, screen reader, ARIA |
| Performance Tester | Lighthouse perf, load testing | Render timing, bundle size, load times |

The Evaluator synthesizes their reports into a single verdict per milestone.

---

## Communication Flow

### Planning Phase
1. Master Planner drafts spec
2. Spec sent to **both** Queen and Evaluator simultaneously
3. Evaluator reviews spec and asks: "How will I verify this? What's testable?"
4. Evaluator ←→ Master Planner negotiate until spec has concrete, gradable acceptance criteria
5. Output: approved spec with embedded test plan and grading weights

### Build Phase
1. Queen assigns work to Workers (existing flow)
2. Workers build, open PRs (existing flow)
3. Code Reviewers approve PRs (existing flow)
4. Queen signals "milestone ready for QA" via peer communication channel
5. Evaluator deploys QA team against the running app
6. QA workers report back to Evaluator
7. Evaluator synthesizes: **PASS** or **FAIL** + structured feedback

### Feedback Loop
- If **PASS** → milestone accepted, Queen proceeds to next milestone
- If **FAIL** → Evaluator sends structured feedback to Queen (peer-to-peer)
- Queen assigns fixes to her Workers
- Workers fix → PR → Code Review → Queen signals "ready for re-QA"
- Loop until Evaluator approves or max iterations hit

### Completion Phase
- Evaluator runs full regression against ALL criteria (not just current milestone)
- Final grade report with per-criterion scores
- Sign-off or rejection with specific remediation list

---

## Context Preservation

Each leader preserves different context, preventing bloat:

**Queen's context:**
- The spec and strategic goals
- What's been built so far
- Which Workers own which files/domains
- Code review history
- Technical debt and tradeoffs made

**Evaluator's context:**
- Acceptance criteria and grading weights
- Test results history (what passed, what failed, regressions)
- Quality trends across iterations ("this keeps breaking")
- User experience insights from Playwright sessions
- Auth strategy and test environment state

---

## Sprint Contract Format

Before each milestone, the Evaluator produces a contract:

```markdown
# Sprint Contract: Milestone 3 — User Dashboard

## Grading Weights
- Functionality: 40%
- Design Quality: 25%
- Accessibility: 20%
- Performance: 15%

## Acceptance Criteria

### Functionality (pass/fail each)
- [ ] Dashboard loads within 3 seconds with 100 items
- [ ] Filter by date range returns correct results
- [ ] Export to CSV downloads valid file with all visible columns
- [ ] Empty state shows helpful message, not blank page
- [ ] Pagination controls work (next, prev, jump to page)

### Design Quality (scored 1-10)
- [ ] Consistent spacing and alignment across all panels
- [ ] Color palette matches design system tokens
- [ ] Responsive layout works at 768px, 1024px, 1440px

### Accessibility (pass/fail each)
- [ ] All interactive elements keyboard-navigable
- [ ] Color contrast ratio >= 4.5:1 for text
- [ ] Screen reader can navigate all dashboard sections

### Performance (measured)
- [ ] Lighthouse performance score >= 80
- [ ] Largest Contentful Paint < 2.5s
- [ ] No layout shifts after initial render

## Threshold
- All pass/fail criteria must PASS
- Scored criteria must average >= 7/10
- Any single scored criterion < 5/10 is an automatic FAIL
```

---

## Auth Gate Solution

Many apps have functionality behind authentication. The Evaluator needs a configurable auth strategy per session.

### Session Config Field

```json
{
  "auth_strategy": {
    "type": "dev_bypass",
    "endpoint": "/auth/dev-login",
    "token": "${TEST_AUTH_TOKEN}"
  }
}
```

### Approach: Dev Auth Bypass (Only Strategy Needed)

Since we only evaluate apps we build and control end-to-end, a single strategy is sufficient:

**Dev Auth Bypass Route** — every app gets a guarded endpoint:
```
GET /auth/dev-login?token=${TEST_AUTH_TOKEN}
```
- Sets the session cookie directly, no login UI needed
- Gated behind `NODE_ENV=development` or `ALLOW_TEST_AUTH=true` — never ships to production
- Playwright hits this endpoint first, gets authenticated, then tests normally
- Works regardless of auth provider (Supabase, Firebase, custom JWT, etc.)

This should be a standard pattern baked into our project scaffolding so every new app automatically includes the bypass route.

### Implementation
- Auth config stored in session config: `{ "auth_endpoint": "/auth/dev-login", "auth_token": "${TEST_AUTH_TOKEN}" }`
- Evaluator reads config before dispatching QA workers
- QA workers hit the bypass endpoint first, receive authenticated browser context
- If no auth config present, Evaluator assumes public app (no auth needed)

---

## Implementation Phases

### Phase 1: Peer Hierarchy Support
**Goal:** Session hierarchy supports two top-level agents instead of one

Changes:
- `AgentHierarchy` struct: add `peers` concept (vec of top-level agents, not just Queen)
- Each peer can spawn sub-teams independently
- Session creation UI: option to enable Evaluator peer
- `PersistedSession`: serialize/deserialize peer hierarchy

Files likely affected:
- `src-tauri/src/session/controller.rs` — session creation, hierarchy management
- `src-tauri/src/session/types.rs` — AgentHierarchy struct
- `src/lib/components/AgentTree.svelte` — render two peer trees side by side

### Phase 2: Peer-to-Peer Communication
**Goal:** Queen and Evaluator can exchange structured messages

Changes:
- New coordination message type: `PEER_FEEDBACK`, `MILESTONE_READY`, `QA_VERDICT`
- File-based channel: `session/peer-comms/queen-to-evaluator/`, `session/peer-comms/evaluator-to-queen/`
- File watcher routes messages to the correct peer
- Queen writes `milestone-ready.md` → Evaluator picks up
- Evaluator writes `qa-feedback-N.md` → Queen picks up

Files likely affected:
- `src-tauri/src/coordination/` — new peer communication module
- `src-tauri/src/http/handlers/` — new endpoints for peer messaging
- File watcher config — watch peer-comms directories

### Phase 3: Evaluator CLI Profile & QA Worker Templates
**Goal:** Evaluator agent type with appropriate behavioral profile and tooling

Changes:
- CLI Registry: add Evaluator profile (InstructionFollowing — skeptical, methodical)
- Default tools: Playwright MCP, Lighthouse, axe-core
- Evaluator prompt template: "You are a ruthless QA engineer. Grade against the contract. Do not rationalize failures."
- QA Worker prompt templates: UI tester, API tester, accessibility tester, performance tester

Files likely affected:
- `src-tauri/src/cli/registry.rs` — add Evaluator behavioral profile
- New prompt templates in session template directory

### Phase 4: Sprint Contract System
**Goal:** Structured contract negotiation and grading

Changes:
- Contract file format (markdown with checkboxes + scoring)
- Evaluator parses contract, grades each line
- Grading output format: structured JSON + human-readable summary
- Contract negotiation flow: Planner → Evaluator → revised contract → approved

Files likely affected:
- Session template directory — contract templates
- Evaluator prompt — contract parsing and grading instructions

### Phase 5: Auth Strategy Configuration
**Goal:** Evaluator can authenticate against the running app

Changes:
- Session config: `auth_strategy` field with type, endpoint, credentials
- Auth handler module: implements each strategy (dev_bypass, storage_state, credentials, cookie_inject)
- QA workers receive pre-authenticated browser context
- UI: auth strategy selector in session launch dialog

Files likely affected:
- Session config types
- New auth handler module
- Launch dialog UI components

### Phase 6: Feedback Loop & Gating
**Goal:** Evaluator gates milestones, feedback routes back to Queen

Changes:
- Milestone state machine: `building` → `qa_pending` → `qa_pass` / `qa_fail`
- Max QA iterations per milestone (prevent infinite loops)
- Deadlock prevention: after N failed rounds, escalate to human
- Queen reads QA feedback, assigns fixes to Workers
- Context reset option: fresh Worker session with handoff file (Ralph-style)

Files likely affected:
- Session state management
- Coordination module — feedback routing
- Queen prompt template — handling QA feedback

### Phase 7: UI Enhancements
**Goal:** Visualize the bicameral structure and QA status

Changes:
- Dual agent tree: Queen's team on left, Evaluator's team on right
- Milestone status badges: building, QA pending, QA pass, QA fail
- QA feedback panel: shows current contract, grades, iteration count
- Quality trends: sparkline of scores across milestones

---

## Deadlock Prevention

Risk: Queen and Evaluator disagree indefinitely (Queen thinks it's done, Evaluator keeps failing it).

Mitigations:
1. Max QA iterations per milestone (default: 3) — after 3 fails, escalate to human
2. Evaluator must provide ACTIONABLE feedback (not just "fail") — specific criteria + what to fix
3. Human override: dashboard button to force-approve a milestone
4. Evaluator can't raise the bar between iterations — grades against the SAME contract

---

## Success Metrics

- Reduction in post-merge bugs caught by human review
- Percentage of milestones that pass QA on first attempt (target: improve over time)
- Time from "milestone ready" to "QA verdict" (target: < 15 min for standard milestones)
- False positive rate: QA failures that turn out to be evaluator errors (target: < 10%)

---

## Open Questions

1. Should the Evaluator persist across sessions (Ralph-style context handoff) or reset each milestone?
2. Can the Evaluator's QA team run in parallel with the Queen's next milestone (pipeline), or must it be strictly sequential?
3. How does this interact with Fusion mode (competing variants)? Does each variant get its own QA pass?
4. Should the Evaluator have commit access for test fixtures, or is it strictly read-only on the codebase?
5. Token budget: QA passes with Playwright + screenshots are expensive. What's the budget ceiling per milestone?
