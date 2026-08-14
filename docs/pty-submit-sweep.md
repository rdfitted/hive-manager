# PTY submit-gap sweep

This procedure measures whether a discrete bare carriage return submits input to each
supported agent composer after a configurable delay. It deliberately does not infer success
from a marker file, an agent reply, or any other downstream action: those signals are
confounded when an operator is present and can press Enter.

The read-only instrument is:

```http
GET /api/sessions/{session_id}/agents/{agent_id}/pty-buffer
```

A successful read returns HTTP 200 with the existing bounded PTY tail:

```json
{
  "session_id": "{session_id}",
  "agent_id": "{agent_id}",
  "output": "recent PTY bytes decoded as text",
  "byte_count": 31
}
```

Malformed session or agent IDs return HTTP 400. A missing session, an agent not registered in
that session, or a registered agent without a PTY returns HTTP 404. The endpoint only observes
the existing 8 KB ring buffer; it never writes bytes or submits input. The ASCII fixtures below
keep `byte_count` directly comparable to the fixture byte length.

## Shipped behavior under test

- The payload and Enter are two discrete PTY writes.
- Enter is a bare `\r`; no `\n` is appended and Enter is outside any bracketed-paste envelope.
- A multi-line payload retains its internal newlines and receives one Enter after the whole
  payload, not one Enter per line.
- `pty.paste` remains bracketed input with no automatic submit.
- The compiled fallback is 50 ms for every adapter until measurements justify a change.
- `HIVE_PTY_SUBMIT_GAP_MS` overrides the adapter policy for the sweep.

The 50 ms value is intentionally unchanged. Existing uncontrolled observations report two
failures around 1.2-2.7 seconds, two successes around 4-5 seconds, and one success around
65 seconds. Agent busy state was uncontrolled, so those observations suggest 50 ms is likely
too short but do not support replacing it with another guessed constant.

## Evidence status before this sweep

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
4. The operator must not focus the target terminal or touch the keyboard from immediately
   before injection until after the buffer observation is recorded.
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
   do not rely on a transient UI repaint.
5. Score the current terminal state reconstructed from the PTY output:

   - **PASS**: the unique payload is no longer sitting in the composer and the buffer shows
     the composer accepted the discrete Enter.
   - **FAIL**: the unique payload remains in the composer, including the literal-newline or
     shift-Enter shape documented by issue #226.
   - **INVALID**: the operator touched the keyboard, the 8 KB tail evicted the observation,
     the process exited, or output is insufficient to distinguish submitted from staged.

   A marker file, response text, or other downstream agent action is supporting context only;
   it never upgrades an ambiguous buffer observation to PASS.
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
