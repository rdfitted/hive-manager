# PTY submit-gap sweep

This procedure measures whether a discrete bare carriage return submits input to each
supported agent composer after a configurable delay. It deliberately does not infer success
from a marker file, an agent reply, or any other downstream action: those signals are
confounded when an operator is present and can press Enter. That constraint is asymmetric:
operator interference invalidates a positive "submitted" observation, but text visibly
remaining in the composer is admissible negative evidence because a keypress cannot
manufacture the staged payload.

Transcript confirmation (#356) now supplies an additional instrument: submitted messages
carry a trailing `[hive-msg:<8 hex>]` marker. `submit_confirmed: true` requires that marker
in the agent's user record followed by a turn start; `submit_confirmation_source` is
`transcript` and `turn_id` identifies the turn. The screen classifier is exposed only as
`submit_screen_activity`. Unsupported CLIs return `null` / `transcript-unavailable`.
The request waits up to 8 seconds, then returns `null` / `transcript-pending` and watches
for at most 60 more seconds. Timeout is `false` only while the message is visibly staged;
otherwise it remains unknown. Deleting the agent cancels its watch.

Issue #241 owns the receiver-side sweep matrix below. A transcript proves that a turn
started, not whether a human pressed Enter. It therefore does not establish an uncontaminated
positive sweep result. The matrix cells remain operator-owned and unchanged.

The read-only screen instrument is:

```http
GET /api/sessions/{session_id}/agents/{agent_id}/pty-buffer
```

A successful read returns HTTP 200 with the existing bounded PTY tail:

```json
{
  "session_id": "{session_id}",
  "agent_id": "{agent_id}",
  "output": "recent PTY bytes decoded as text",
  "byte_count": 32
}
```

Malformed session or agent IDs return HTTP 400. A missing session, an agent not registered in
that session, or a registered agent without a PTY returns HTTP 404. The endpoint only observes
the existing 8 KB ring buffer; it never writes bytes or submits input. The ASCII fixtures below
keep `byte_count` directly comparable to the fixture byte length.

## Shipped behavior under test

- Since #256, a submitted payload is delivered inside one bracketed-paste envelope
  (`ESC[200~` payload `ESC[201~`), and the envelope is closed before Enter is sent. The
  envelope is what stops TUI paste-coalescing (observed in codex, which suppresses Enter
  for a window after a fast raw byte burst) from swallowing the follow-up `\r` as a
  literal newline inside the paste.
- Enter is still a bare `\r` written as its own discrete PTY write; no `\n` is appended
  and Enter is outside the bracketed-paste envelope. After the synchronous wait, exactly
  one retry is allowed only when the transcript has not recorded the message and the
  composer visibly holds its exact marker. Screen activity alone never triggers a retry.
- Every nonempty `"submit": true` message receives a fresh nonce. An empty flush request
  writes only bare Enter and carries no nonce. If the agent has an earlier pending nonce
  within its watch window, the flush confirms against that nonce; otherwise it returns
  `null` / `transcript-unavailable`. A `"submit": false` staging write carries no nonce
  and issues no Enter.
- Codex's submit-time gap is 150 ms plus 1 ms per 200 sanitized payload bytes, capped at
  600 ms. `HIVE_PTY_SUBMIT_GAP_MS` overrides it; other adapters retain their 50 ms default.
- A multi-line payload retains its internal newlines and receives one Enter after the whole
  payload, not one Enter per line.
- Trailing `\r`/`\n` on an injected message are always stripped before writing
  (`clean_message`), so a payload cannot self-submit by embedding its own newline; the
  discrete Enter write is the only submit mechanism.
- `pty.paste` remains bracketed input with no automatic submit.
- The compiled fallback gap is 50 ms for non-codex adapters. Codex uses a payload-scaled
  150–600 ms gap at submit time; the shared adapter policy default remains 50 ms.
- `HIVE_PTY_SUBMIT_GAP_MS` values from 0 through 300,000 ms override the adapter policy for the
  sweep. Invalid, overflowed, and larger values are rejected with one warning and fall back to the
  adapter default; they are never silently clamped. The 300,000 ms safety ceiling is not a default.
  The parsed override is cached in a `OnceLock` on first use, so it is restart-scoped: changing
  the variable requires restarting the backend, not just re-running a request.

The historical 50 ms discussion predates the current Codex policy. Uncontrolled observations
reported two failures around 1.2–2.7 seconds, two successes around 4–5 seconds, and one
success around 65 seconds. Busy state was uncontrolled, so these are not matrix evidence.
The #256 bracketed envelope improves framing, but does not establish that Codex no longer
needs a submit gap. Non-codex adapters retain the 50 ms default pending controlled evidence.

## Sender receipt and delivery signal (#256, #259, #356)

The inject response reports measured facts, not request echoes:

- `write_started_at` and `submitted_at` bracket the successful write. `payload_bytes_written`
  counts sanitized payload bytes inside the envelope; embedded `ESC[201~` is stripped.
- `submit_attempted` and `submit_keystroke_issued` reflect the initial Enter write.
  `submit_bytes_written` counts all Enter bytes actually written. `submit_attempts` is 0
  for no Enter, 1 for the initial Enter, or 2 if the single retry also lands.
  `submit_retry_failed` and `submit_retry_failure` distinguish a failed retry from no retry.
- `submit_confirmed` is transcript evidence: `true` requires the exact nonce in the agent's
  user record followed by a turn start. `submit_confirmation_source` is `transcript`,
  `submit_confirmation_basis` is `transcript-confirmed`, and `turn_id` identifies that turn.
  Screen activity never makes this field true. Unsupported or unreadable transcript sources
  return `null` / `transcript-unavailable`; pending evidence returns `null` / `transcript-pending`.
- `submit_confirmation_window_ms` is the 8,000 ms synchronous transcript window;
  `submit_confirmation_elapsed_ms` measures elapsed delivery observation, including retry work.
  A pending result starts up to 60 seconds of background watching, cancelled on agent deletion.
  At expiry, `transcript-timeout` is false only if the nonce is still visibly staged;
  otherwise confirmation remains unknown. No-submit requests have no transcript confirmation.
- `submit_screen_activity`, `submit_screen_activity_basis`, `submit_screen_activity_window_ms`
  and `submit_screen_activity_elapsed_ms` expose the advisory screen classifier separately.
  Its window is 1,500 ms: true means sustained activity, false means no observed reaction,
  and null means indeterminate or unavailable. These observations cannot confirm delivery.
- The single extra-CR retry requires transcript-pending, no recorded nonce-bearing user
  message, and a visibly staged nonce. The handler rechecks transcript and screen state
  immediately before retrying. A recorded user message without a turn suppresses retry.
  A screen-false result alone does not authorize a retry.
- `pre_write_pty_observation_available`, `pre_write_activity_change_count`,
  `receiver_busy_before_write`, `pre_write_observation_window_ms` and
  `pre_write_observation_elapsed_ms` describe the 250 ms pre-write screen baseline.
  The historical #260 busy-receiver downgrade now applies only to advisory screen activity:
  changes spanning at least 125 ms downgrade its would-be positive to null with basis
  `busy-receiver-indeterminate`. It does not change transcript confirmation.
- `pty_observation_available`, `pty_output_bytes_before`, `pty_output_bytes_after_write`,
  `pty_output_bytes_after`, `pty_activity_observed`, `observation_window_ms` and
  `observation_elapsed_ms` describe bounded PTY snapshots and activity. `evidence_scope`
  identifies the transcript delivery scope. None of these screen counters establishes a
  sweep PASS without the controlled receiver-buffer evidence required by #241.

## Two-call workaround

Before #256, a single-call submit could leave the payload staged in a codex composer with
the Enter swallowed. The two-call pattern remains supported and is the recovery path for
any content found staged in a composer:

```bash
# 1) deliver the payload, leave it in the composer
curl -X POST .../inject -d '{"target_agent_id":"...","message":"...","submit":false}'
# 2) separate call: bare Enter flushes it
curl -X POST .../inject -d '{"target_agent_id":"...","message":"","submit":true}'
```

## Evidence status before this sweep

Issue #241 owns this sweep matrix and the single-call receiver-side assertion.

No cell has been measured against the current per-adapter policy and T6 instrument. Each cell
below must be measured in two controlled states: **idle at the prompt** and **mid-generation**.

| Adapter | About 100 bytes | About 2.5 KB | Multi-line |
|---|---|---|---|
| `codex` | **UNMEASURED** for the current build. Historical v0.43.0 behavior: FAIL, operator-confirmed twice. | **UNMEASURED** | **UNMEASURED** |
| `claude` | **UNMEASURED** | **UNMEASURED** | **UNMEASURED** |

The historical Codex failures are useful baseline evidence, but they do not count as a
measured current-build cell. The uncontrolled gap observations above also do not count as
matrix cells because their adapter, payload class, and busy state were not all controlled.

## Prerequisites

1. Use a build containing the per-adapter policy, the deterministic PTY tests, and T6's
   read-only buffer endpoint.
2. Start a fresh backend for each gap candidate with
   `HIVE_PTY_SUBMIT_GAP_MS=<milliseconds>` set before launch.
3. Start a fresh real session for the selected adapter and record its session and agent IDs.
4. For a positive PASS, the operator must not focus the target terminal or touch the keyboard
   from immediately before injection until after the buffer observation is recorded. A
   visibly staged payload remains admissible FAIL evidence even if the operator was present,
   because operator input can confound submission but cannot manufacture that negative state.
5. Record the build SHA, OS/ConPTY version, adapter CLI version, model, session/agent IDs,
   candidate gap, payload class, attempt number, and whether the agent was visibly busy.

Do not run Codex and Claude trials concurrently. Background output can evict relevant bytes
from the bounded 8 KB PTY tail and makes busy-state control harder.

## Payload fixtures

Use ASCII fixtures so byte counts do not depend on encoding:

```powershell
$shortPayload = ('s' * 96) + ' END'       # 100 bytes
$longPayload = ('l' * 2556) + ' END'      # 2560 bytes (2.5 KiB)
$multiPayload = "line-01`nline-02`nline-03 END"
```

Give each attempt a short unique prefix such as `C-S-050-A1` (adapter, payload class, gap,
attempt). Keep the total payload in the same size class by shortening the repeated body by
the prefix length.

## Trial procedure

For each adapter (`codex`, then `claude`), payload class, controlled agent state, and candidate
gap:

1. Launch the backend with that candidate gap and create a fresh target session. Put the agent
   into the declared state before the injection:

   - **idle at prompt**: wait for the composer to be idle and record the buffer evidence;
   - **mid-generation**: submit a bounded setup prompt, confirm from the buffer that output is
     actively streaming, then inject the sweep payload without touching the keyboard.
2. Send `GET /api/sessions/{session_id}/agents/{agent_id}/pty-buffer` and save the complete
   HTTP status and JSON response as the baseline.
3. Submit one payload through the existing injection endpoint:

   ```http
   POST /api/sessions/{session_id}/inject
   Content-Type: application/json

   {
     "target_agent_id": "{agent_id}",
     "message": "{payload}",
     "submit": true
   }
   ```

4. Without touching the target terminal, repeat the PTY-buffer `GET` at 250 ms, 1 second,
   5 seconds, and 10 seconds after the configured gap. Save every raw status and response;
   do not rely on a transient UI repaint. Note that the inject POST itself can stay pending
   up to 8,000 ms for synchronous transcript confirmation, plus any single retry write.
   Pending confirmation continues in the bounded background watch. Issue the POST from a
   separate shell (or background it) rather than waiting for its response — otherwise the 250 ms and 1 second
   samples are missed before the POST returns.
5. Score the current terminal state reconstructed from the PTY output:

   - **PASS**: the unique payload is no longer sitting in the composer and the buffer shows
     the composer accepted the discrete Enter.
   - **FAIL**: the unique payload remains in the composer, including the literal-newline or
     shift-Enter shape documented by issue #226.
   - **INVALID**: for a claimed PASS, the operator touched the keyboard; or, for either result,
     the 8 KB tail evicted the observation, the process exited, or output is insufficient to
     distinguish submitted from staged.

   A marker file, response text, or other downstream agent action is supporting context only;
   it never upgrades an ambiguous buffer observation to PASS. Operator interference likewise
   cannot establish a PASS, but a payload visibly resident in the composer is always admissible
   FAIL evidence.
6. Repeat until there are three valid attempts for that adapter/payload/state/gap cell. Record
   every invalid attempt
   and its cause, but exclude it from the pass denominator.
7. After the submit trial, create a fresh session and send the same payload with
   `"submit": false`. Confirm the buffer shows staged, unsubmitted content. This validates
   the instrument for that adapter/build before accepting a PASS.
8. Run the `pty.paste` regression once per adapter/build and confirm the content remains
   staged with no automatic Enter.

## Gap search

Run a coarse sweep at `0, 50, 100, 250, 500, 1000, 2500, 5000` ms in **both controlled agent
states**. If a candidate produces three valid passes, bisect between it and the greatest lower
failing candidate until the resolution is 50 ms. Do not promote a measured threshold directly
into the compiled policy: retain trial records, compare both adapters, both states, and all
payload classes, and choose a safety margin explicitly in a follow-up review.

## Results ledger

Append one row per attempt. A cell becomes **MEASURED PASS** only after three valid passes at
the stated gap; any valid failure makes it **MEASURED FAIL** at that gap.

| Build | Adapter/version | Payload class | Bytes/lines | Gap ms | Attempt | Busy state | Buffer artifact | Result | Notes |
|---|---|---:|---:|---:|---:|---|---|---|---|
| _unmeasured_ | | | | | | | | | |

After the operator sweep, replace the evidence-status table's `UNMEASURED` labels with the
measured result and link each cell to its retained buffer artifacts. Cells not actually run
must remain explicitly `UNMEASURED`.
