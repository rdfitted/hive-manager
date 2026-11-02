# Render check

Run this from the repository root in PowerShell:

```powershell
npm run render:check
```

Use Windows with WebView2, Node.js/npm, installed project dependencies (including
`playwright-core`), Git, Rust, and the MSVC Windows build tools required by Tauri.
Playwright uses the installed WebView2 runtime. It does not download a browser.

The command builds a debug executable with bundled frontend assets and the Tauri
custom protocol. It does not use a Vite dev server or install the app. The build
wrapper uses one Cargo job and a shared target under
`%LOCALAPPDATA%\hive-manager-target\loop1`.

The build receipt is `tools/render/out/build-state.json`. Reuse requires matching
source/configuration input hashes, the expected executable path, and the executable
hash. Missing or changed inputs or executable trigger a build. Inputs changing
during a build stop the run before launch. Documentation edits do not invalidate
the build.

Recorded local timings were 352.7 seconds for the first debug build and about
166–194 seconds for later rebuilds. Complete cached render runs took about
103–109 seconds, including roughly 66 seconds for the stall loop and sweep.
Allow about eight minutes for a build plus captures on the measured machine,
plus any gate wait. A completely empty dependency cache has not been timed.
Hosted Windows CI timing has not yet been measured.

In a managed worktree the command joins the session FIFO gate and releases it in
cleanup. Otherwise it takes an identity-checked local render lock. Do not wrap it
in a second FIFO request that you hold while it waits for its own request.
Wait for the cold build and startup; an app process alone does not prove that the
page has loaded. The driver waits for CDP, custom-protocol navigation, and the
synthetic session text.

## Isolation rule

A hive may run this isolated instance through this command or the three settings
below. A hive may still never launch a normal debug build beside the live app.
In a managed delivery, wait for the Queen's `INSTANCE GO` before starting an
isolated instance.

| Setting | Meaning |
| --- | --- |
| `HIVE_MANAGER_INSTANCE_DIR` | Required existing absolute root with a `.hive-instance` marker. |
| `HIVE_MANAGER_INSTANCE_PORT` | Required API port, at least 1024 and never 18800. |
| `HIVE_MANAGER_INSTANCE_CDP_PORT` | Optional CDP port with the same restrictions; must differ from the API port. |

These settings apply only to debug builds. Release builds ignore them. A partial
or invalid debug configuration refuses startup. Roots cannot overlap real app
data or the webview profile, contain the real home, or use unsafe paths or linked
trees. The bootstrap redirects app data, home, transcript/configuration roots,
temporary files, Git configuration, and the webview profile into the instance.
It clears inherited agent identity and related environment overrides. Path-bearing
operations are contained; mutating actions, real agent spawns, preview launch,
and updater/process plugin operations are refused. Autostart writes are guarded
against in source tests. These guards are application boundaries, not an OS sandbox.

The command seeds fresh synthetic sessions and a small local Git fixture, picks
free loopback ports, and attaches Playwright over CDP. The app opens unfocused,
off-screen, with its own webview profile. Cleanup matches executable, PID and
creation time before stopping owned processes. It removes the marked instance
profile even after an assertion or navigation failure. Stale-run cleanup checks
ownership too; it refuses to reap an active run. Never kill processes by image name.

## Assertions and gates

The report keeps raw `passed` and gate-level `accepted` separately. Exit zero
means every assertion was accepted, including explicitly recorded known findings.

| Assertion group | What it checks |
| --- | --- |
| Origin, API and environment | Custom-protocol origin, isolated API marker, and nine observed process environment roots inside the instance. |
| Viewports | Renderer CSS dimensions of 1440×900 and 800×900; the report also records pixel ratio. |
| Screen and PNG content | Expected fixture text in a visible target, decoded nonuniform pixels, correct PNG dimensions, and all required captures. |
| Grid panes | Eight visible panes. |
| Grid composers | Composer boxes inside their named panes, with and without an attention card, at load and after the stall/sweep wait. |
| Attention | Finished seats have no card or per-agent “needs you” badge; stalled and exited-during-startup seats have both, before and after the wait. |
| Version badge | Hit testing at its centre and computed visibility, opacity and line-through styles on normal screens where it mounts. Dashboard absence and modal backdrop interception are recorded separately. |
| Chat | Fixture table and code render; fixture inline HTML remains literal text and creates no matching HTML element. |
| Kanban and sidebar | Named synthetic cards and sidebar entries are present. |
| Updater | The actual updater IPC invocation rejects in isolated mode. |
| Network and console | No observed request/resource uses live port 18800; no browser console error or page error occurs during observation. |
| Mutation coverage | All 23 required render predicates have before/mutant/restored proof; both network observers detect a locally fulfilled synthetic request. |
| Cleanup and live app | No owned process/profile remains; live health, PID, executable and start identity are unchanged, or port 18800 remains unused when no live app exists. |
| Desktop | Input desktop remains unchanged; foreground identity is compared only when observable. |

G41 is the render/assertion gate, including restored mutation proofs and issued
known findings. G42 is the before/after live-app check. G43 requires normal and
forced-failure cleanup evidence, plus confirmation that output stays ignored by
Git. The driver's recorded forced-failure test exited nonzero as intended and
still removed its processes and profile. G44 is a separate review with the agent's
own eyes: the Queen reads the PNGs and records what each shows in the PR body.

## Read the output

`tools/render/out/report.json` is the latest run, including a failed or diagnostic
run. Check `phase`, `passed`, `error`, `assertions`, `mutations`, and `screenshots`.
A forced-failure report can therefore replace the latest successful render report.
Each run also keeps its report and PNGs under `tools/render/out/runs/<run-id>/`.
Build diagnostics go to `tools/render/out/build.log`.

Read the PNGs listed in the report and say what you saw. A pass is not a substitute
for looking. The complete run captures Launch, Focus, Grid, Kanban, expanded
Sidebar, Review, and Keyboard Shortcuts at both widths. Kanban captures each
synthetic card after scrolling, for 16 PNGs in total. Report clipping, overlap,
missing content, or unexpected blank areas by screen and width. Separate expected
blank terminals from missing chat or controls.

PNGs stay local. Never commit them, attach them to public text, or upload them.
The entire output directory is ignored. Reports can contain machine paths and
process details; keep them local too. Publish only a path-free text summary.

## Known findings

`tools/render/known-findings.json` maps assertion names to positive issue numbers.
The current entries are both Grid composer containment checks for
[#414](https://github.com/rdfitted/hive-manager/issues/414).

The assertion still runs and records its actual boxes and card state. Matching
overflow is `passed: false`, `accepted: true`, `status: "known-finding"`. It must
have nonzero composer dimensions, actual overflow, `contained: false`, and the
expected attention-card state. A different failure shape fails the gate. A missing
issue number fails it too. If the assertion starts passing, `unexpected-pass`
fails the command until the issue and recorded finding are reconciled. The shape
check does not pin exact pixel values, so the report's numbers still need review.

## Limits

Terminals are blank because the fixtures have no PTY. Terminal content and palette
are not checked. There are no live agents and no real delivery or spawn. This
drives Windows WebView2 only; it does not establish macOS rendering.

Foreground preservation cannot be measured while the input desktop is not
`Default`. Recorded runs used `Screen-saver`, returned a zero foreground handle,
and stored `foregroundPreserved: null`. That is unavailable evidence, not a pass;
an unchanged input desktop does not prove preserved focus.

Geometry assertions cover the boxes and points they name, not the whole UI.
Nonuniform PNG pixels do not establish visual correctness. There is no pixel
diffing. Presence of Review content does not exercise a real delivery. The network
checks cover the observed webview requests, not a general network sandbox.

## Troubleshooting

- **No CDP listener with a renderer running:** WebView2 failed to open its listener
  when `USERPROFILE\AppData\Local` was missing below the re-rooted home. Bootstrap
  now creates that folder. Keep the isolated home; do not point it back at the real
  profile. The controlled bisect found the missing folder, not the non-Default
  desktop, caused those CDP failures.
- **Trying `tauri-driver`:** the exactly matched EdgeDriver inherited a user-data
  folder that triggered the deliberate real-profile overlap guard. That route did
  not work here. Keep the guard and use the CDP driver.
- **Review/Git errors:** a Windows verbatim path in `GIT_CONFIG_GLOBAL` broke Git.
  Instance environment paths now use validated ordinary absolute disk paths.
  Inspect the contained path; do not restore the real Git configuration.
- **Manifest changes after a build:** Tauri rewrote `src-tauri/Cargo.toml` line
  endings. The build wrapper restores the original bytes if only line endings
  changed, and rejects any other content change. Do not discard concurrent edits.
- **Slow first launch or build:** wait for build completion and the driver's
  readiness checks. Inspect `build.log` or the report's failure URL, ready state,
  and text. Do not start a second debug app to test readiness.
