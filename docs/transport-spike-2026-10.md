# Transport comparison decision record — 2026-10-21

Evidence-only outcome for #358, Lane X (T11–T13). No transport implementation
was added to the application. Measurements stayed within the one-working-day box.

**Recommendation:** prefer B1, Hive-owned `codex app-server`, for the next
operator-approved implementation slice. It passed the measured M2/M3 prerequisites
with the normal Codex home and a scratch `sqlite_home`. B2 is **not evaluated**:
operator setup was not confirmed. This is not evidence that B1 outperforms T3.
Gate implementation on resolving or bounding B1's observed cold initialization
delay, and on the operator's ruling. Do not start the winning implementation here.

## Method and evidence

Windows, `codex-cli 0.163.0`, model `gpt-6.1-sol`. Standalone stdlib Python probe
in `tools/transport-spike/b1/probe.py`; fresh OS-temp directory outside this repo,
throwaway `git init`, no application worktree/branch. One authenticated worker ran
one read-only cwd-printing shell command. No real prompts, transcript bodies,
account identifiers, tool output or raw stderr are saved.

The installed CLI generated 318 JSON schema files in scratch storage; the sorted
relative filenames plus file bytes have SHA-256
`b889535466ed1da1b0a456d38cda727fbf6e54f58f69dc217606a2d5cba56cba`.
Only selected schema shapes and the digest are retained. Primary measured evidence
is `tools/transport-spike/b1/results.json`; failed first startup is
`initial-timeout.json`; schema-only facts are in `schema-evidence.json`.

M2/M3 were the first eligibility checks: observe the push lifecycle, compare the
thread and command cwd with the caller's scratch directory, and check database
creation under the explicit `sqlite_home` override. No polling API was used.

## M1–M8

| Metric | B1: own app-server | B2: T3 MCP runtime |
|---|---|---|
| M1 Delivery | **MEASURED:** `turn/start` JSON-RPC response carried a turn ID in **150.03 ms**; matching `turn/started` arrived **163.52 ms** after submission. This measures runtime turn acceptance, not successful task completion. First attempt never initialized within 180 s; successful second initialization took 255.081 s. | **BLOCKED: operator setup** |
| M2 Events | **MEASURED: YES, push.** Unsolicited `turn/started`, `item/started`/`item/completed` (`commandExecution`), `item/commandExecution/outputDelta`, and `turn/completed` arrived on stdout without polling. Thread/turn correlation matched. Final status `completed`; start-to-completion about **9.987 s**. | **BLOCKED: operator setup** |
| M3 Worktree control | **MEASURED: YES for tested cwd and SQLite relocation.** Normal Codex home retained; caller-set scratch cwd matched both returned thread cwd and command execution cwd; command exit 0. Explicit `-c sqlite_home=<scratch store>` produced five SQLite databases there, consistent with `cli/agent_store.rs`. No shared-home scan was performed, so exclusive absence of every possible shared-home write is **UNMEASURED**. Real repository worktree creation was intentionally not tested. | **BLOCKED: operator setup** |
| M4 Approvals | **MEASURED:** `approvalPolicy:"never"` + `sandbox:"danger-full-access"` accepted and echoed as the expected policy; read-only shell command ran without a client approval grant. Non-bypass live approval emission **UNMEASURED**. Schema contains `item/commandExecution/requestApproval` and `item/fileChange/requestApproval`; schema presence is not a live test. | **BLOCKED: operator setup** |
| M5 Native delegation | **UNMEASURED:** no worker subagents spawned; Assignment Contract confinement not exercised. Schema has collaboration tool items, which does not establish enforcement. The read-only reviewer of this spike was not a child of the probed worker and supplies no M5 evidence. | **BLOCKED: operator setup** |
| M6 Multi-CLI | **UNMEASURED:** no Claude/Cursor transport implementation or effort measurement. Codex success does not establish their integration cost. | **BLOCKED: operator setup** |
| M7 Dependency risk | **MEASURED:** installed version 0.163.0; version-specific schema generation succeeded (318 files, digest above); CLI labels app-server experimental. Cross-version upgrade compatibility, future pin availability and maturity/contribution policy are **UNMEASURED**. | **BLOCKED: operator setup** |
| M8 Operations | **MEASURED:** existing installed Codex/account worked; one local stdio server, Python and Git; no additional install/login/tunnel/pairing performed. Normal-home startup first exceeded 180 s, then initialized in **255.081 s**; thread creation **6.048 s**; complete successful process **271.497 s**. Root cause of the startup delay **UNMEASURED**. Clean-home diagnostic initialized in 0.678 s; this suggests environment/configuration involvement but does not identify a cause. | **BLOCKED: operator setup** |

Neither tested B1 prerequisite returned a disqualifying "no". B2 has no measured
yes/no on M2/M3 and cannot be declared eligible. Whether T3 needs polling, preserves
SQLite isolation, or requires Full access across every project is **BLOCKED:
operator setup**, not inferred from documentation. No T3 software, account,
pairing, tunnel or access grant was created.

## Authentication diagnostic and limits

The normal-home run above is the M3 evidence. Empty homes are diagnostic only,
never a proposed Hive isolation strategy: Hive uses `sqlite_home` while sharing
Codex credentials/configuration.
The normal-home run's exact authentication/billing mode is **UNMEASURED**;
successful work does not distinguish OAuth credentials from an environment key.

An initial empty-home turn failed; API-key environment fallback was not controlled
in that attempt, so its authentication/billing mode is **UNMEASURED** (retained in
`isolated-flow-pre-guard.json`). After the Queen identified the #207/#208 hazard,
the probe was changed to remove both `OPENAI_API_KEY` and `CODEX_API_KEY` for all
empty-home launches. The controlled repeat (`isolated-flow.json`) reported
`account_present:false`, `requires_auth:true`, and failed with an auth error.
**That controlled diagnostic did not authenticate.** No credentials were copied.
Its 29.49 ms `turn/started` followed by auth failure also demonstrates why a turn
start proves runtime acceptance rather than completed or billable work.

The probe itself never opens, copies or writes the real Codex/Claude home. The
normal CLI consumes its existing home; all explicit probe filesystem writes are
to scratch and the owned evidence paths. We did not audit incidental CLI home
writes. Timings are individual samples, not percentiles or load measurements.

## I1 mapping

P1 published I1 on the shared channel at 2026-10-21 15:02:24Z:
`Delivery { confirmed: Option<bool>, basis: String, source: String, turn_id: Option<String> }`;
bases include `transcript-confirmed`, `transcript-pending`,
`transcript-unavailable`, `transcript-timeout`, `protocol-turn-started`;
sources are `transcript | protocol | none`.

For **B1**, correlate the outbound `turn/start` request ID with its response's
`turn.id`, then match **`turn/started` with the same `threadId` and `turn.id`**.
That event would yield `Delivery { confirmed: Some(true), basis:
"protocol-turn-started", source: "protocol", turn_id: Some(<matched turn>) }`.
Buffer early notifications until the response establishes the turn ID. Never
confirm an unrelated turn, raw response arrival, screen activity, or a failed
initialization. The turn-start event does not assert successful completion.

For **B2**, the concrete runtime event is **BLOCKED: operator setup**. No measured
T3 subscription/notification can be named. A future probe must demonstrate a
runtime turn-start event and message-to-thread/turn correlation before mapping it
to `protocol-turn-started`. MCP request success or a polled thread status alone
is insufficient evidence for that basis.

## Follow-up issue shape for the preferred candidate

Proposed title: **Codex structured transport: bounded startup and correlated
turn-start delivery (B1)**. File/start only after the operator rules on this record.

- Scope the first slice to one Codex worker behind an opt-in transport; preserve
  existing PTY behavior for the other CLIs.
- Diagnose the 255 s initialization delay without changing operator configuration;
  set explicit startup deadlines, cancellation and actionable failure reporting.
  Keep existing authentication and per-worker `sqlite_home`; never clone homes or
  accidentally select API-key billing.
- Pin/test against the approved CLI/schema version. Implement initialize/initialized,
  thread creation, turn submission, notification consumption and shutdown.
- Correlate request ID, thread ID and turn ID before setting I1's protocol basis;
  handle early events, rejected requests, process exit, cancellation and auth errors.
- Prove isolation across two controlled workers, native delegation confinement,
  and live approval routing. These remain follow-up gates, not passed spike rows.
- Acceptance: no polling required for turn/tool/completion supervision; cwd and
  SQLite override retained; no false cross-worker delivery; bounded process cleanup;
  no private transcript/account data in test fixtures. Measure Claude/Cursor effort
  separately. Revisit B2 only after operator setup is explicitly confirmed.

## Verification

Live B1 completed with correlated events and a zero-exit command; controlled
empty-home authentication failed as expected. The owned Python source and JSON
artifacts are checked for syntax/shape and private-data exclusions. G-spike-isolated
requires zero `transport-spike` references in `src`, `src-tauri`, `package.json`
and `src-tauri/Cargo.toml`. No owned UI files exist, so UI slop-check is not applicable.
No app/build files, dependency locks or migrations were changed by this lane.

Protocol background: [official Codex App Server documentation](https://learn.chatgpt.com/docs/app-server).
Live claims above come from the installed CLI, not assumptions based on that page.
