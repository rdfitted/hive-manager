# Webview driver spike

Status: real CDP attach and viewport measurements proved on the rebuilt guarded app without a driver scaffold. The bootstrap fix is 1ea4046. No normal debug instance was started.

## Initial observations before the profile-directory bisect

- Built with `npm run tauri -- build --debug --no-bundle`, one Cargo job, under the session FIFO lock. First build: 352.7 seconds, exit 0.
- Guarded instances received only the three `HIVE_MANAGER_INSTANCE_*` settings from the driver. Rust supplied the WebView2 environment settings.
- WebView2 156.0.4314.58 process command lines showed `--user-data-dir` below the fresh instance's `webview/EBWebView`, and the requested remote debugging port and loopback address.
- Two Node launches did not produce a listening CDP endpoint within 60 seconds. A native `ProcessStartInfo` comparison also showed no listening CDP endpoint. No real CDP attachment or viewport/origin measurement has succeeded.
- The native comparison exposed inherited output handles keeping a synchronous launcher open. Its exact owned application PID was stopped; the comparison is inconclusive about window visibility. It does not establish a visibility fix.
- `tauri-driver` 2.1.0 with exactly matched EdgeDriver 156.0.4314.58 failed session creation. Application stderr explained the failure: `Isolated instance refused: Instance overlaps real data or webview profile`. EdgeDriver's inherited profile override conflicts with the guard; the guard remains intact.
- Each attempted run left no matching process and removed its profile. The live application's health response and PID stayed unchanged before and after every run.
- `GetForegroundWindow()` returned zero in the execution environment before and after. A follow-up Win32 probe found window station `WinSta0`, thread desktop `Default`, and active input desktop `Screen-saver`. Foreground preservation remains unproven while the input desktop differs; zero is not presented as a successful focus measurement.
- `npm run check`: 0 errors, 0 warnings. Fixture tests: 4/4 pass. Full `npm test`: 85 files, 912 tests passed in 155.16 seconds, no rerun. `cargo check` and `cargo check --tests` passed. `render:check`, G41 and mutation proofs remain pending. G42 and failure-cleanup G43 passed for the attempted runs.

Local diagnostic JSON stays in ignored `out/`; screenshots and machine paths are not posted publicly.

The active Screen-saver input desktop is also a possible cause of the unavailable CDP listener. These attempts do not establish that CDP cannot work on an unlocked desktop. Later attempts record input desktop names before/after and renderer presence in the local JSON. Page load is `unobserved` until the driver actually inspects it; a browser or renderer process alone does not prove a page loaded. No wake, unlock, or input-desktop change is attempted.

## Documented, not observed

- [Playwright's WebView2 guide](https://playwright.dev/docs/webview2) documents `connectOverCDP` against an app-provided debugging port.
- [Microsoft's WebView2 report](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5645) describes environment debugging overrides being dropped in elevated hosts starting with runtime 150. This host reports Medium integrity and its browser arguments are visible, so that report does not establish our cause.
- [Tauri's manual WebDriver setup](https://tauri.app/develop/tests/webdriver/manual-setup/) requires a matched EdgeDriver on Windows. The fallback used an exact version match.

## Intermediate API-only experiment, before the bisect

The app-source owner provided the same debug-gated CDP arguments through `WindowConfig.additional_browser_args` in 6660345, retaining all profile/environment isolation and browser defaults. The rebuild passed in 193.78 seconds. The subsequent real attempt still had no reachable CDP endpoint after 60 seconds.

That run recorded `inputDesktopBefore=Screen-saver`, `inputDesktopAfter=Screen-saver`, `rendererStarted=true`, `pageLoaded=unobserved`, and `foregroundPreserved=null`. It observed a real renderer process, not a loaded page. No `DevToolsActivePort` file was found. Exact PID/tree cleanup and profile removal passed; live health and PID were unchanged. No screenshot was possible, so pixel content remains unproven. A separate read-only `GetGUIThreadInfo` probe returned access denied (5). No registry, wake, unlock, or desktop change was attempted.

At that point CDP remained the preferred documented route. The proposed viewport method was `page.setViewportSize` with measured `innerWidth`, `innerHeight`, and `devicePixelRatio` at 1440x900 and 800x900; none had yet been observed. Real attach, origin, viewports, screenshots, foreground preservation, G41, and T11-T13 were held pending a successful attach or the Queen's ruling.

T10 investigation note [posted on #412](https://github.com/rdfitted/hive-manager/issues/412#issuecomment-6588505022) from PowerShell using a body file, then read back. It contains no screenshots or machine paths.

## Controlled bare-host bisect

A disposable, pinned wry 0.54.1 / tao 0.34.5 host loaded only a local data URL into its own scratch profile. It used no Hive code or data. Windows stayed unfocused throughout as a safety invariant. Every row ran with input desktop `Screen-saver` before and after. Pass means a real Playwright CDP attach, page-load event, DOM content and screenshot; failing rows still had a renderer and a finished page-load event.

| Cumulative setting or confirmation | CDP |
| --- | --- |
| Bare host, environment debugging port only | Pass |
| Add loopback debugging address | Pass |
| API arguments instead of environment arguments | Pass |
| Add scratch profile environment override | Pass |
| Add re-rooted TEMP/TMP | Pass |
| Add re-rooted APPDATA | Pass |
| Add re-rooted LOCALAPPDATA | Pass |
| Add USERPROFILE/HOME pointing at an empty synthetic home | Fail; repeated |
| USERPROFILE only | Fail |
| HOME only | Pass |
| Both home variables plus all normal profile folders | Pass |
| Both home variables plus only AppData/Local | Pass; repeated |
| Both home variables plus AppData parent only | Fail |

The trigger is a missing `AppData/Local` below the synthetic USERPROFILE. Creating that directory restores CDP while keeping both home variables isolated. This corrects the provisional Screen-saver hypothesis: that desktop state did not itself prevent attach. The precise internal WebView2 failure mechanism is not established. Diagnostic logs had the same DPI, unsupported-LLM and AMD-video messages in positive and negative runs; no explanatory crash dump was found.

All probe processes were stopped by identity-checked exact PID. A taskkill child-exited race interrupted one diagnostic report; process absence and live health/PID were checked independently, and the report writer now handles that race. Automatic approval rejected recursive scratch-profile removal. Those profiles were preserved for the Queen's requested cleanup; the host source/binary were never committed.

## Real guarded app proof with the directory scaffold

Two fresh real-app runs created only `home/AppData/Local` after seeding, without rebuilding or changing a guard. Both attached with Playwright `connectOverCDP`. `page.setViewportSize` produced measured 1440x900 and 800x900 CSS viewports, each with `devicePixelRatio=1`, at `http://tauri.localhost`. Both had `window.__HIVE_INSTANCE__.isolated=true` and their own API base. The off-screen, skip-taskbar, unfocused app settings remained in place.

Both PNGs were inspected: actual launch form, synthetic sidebar and version badge. The corrected confirmation captures contain 3,977 and 3,984 distinct colors respectively. The final report records `pageLoaded=true` after waiting for the manifest session name; a prior immediate DOMContentLoaded sample was too early for hydration.

`environment.ps1` reads only the owned x64 process after matching PID, executable and creation time. It returns only nine isolation-root variables, never the full environment. Its self-process check and real-app read proved USERPROFILE, HOME, APPDATA, LOCALAPPDATA, TEMP, TMP, CODEX_HOME, CLAUDE_CONFIG_DIR and WEBVIEW2_USER_DATA_FOLDER all stayed inside the instance. Request listeners and resource timing found no request to port 18800. Every real-app run removed its process tree and profile; live health and PID were unchanged. Foreground inspection remains unavailable on the different input desktop and is recorded as null, not a pass.

Chosen driver: Playwright CDP. Chosen viewport method: CDP emulation via `page.setViewportSize`, with actual renderer measurements. The permanent bootstrap folder fix is 1ea4046. The temporary directory-creation line has been removed from the spike.

## Permanent bootstrap proof

The confirmed bootstrap fix was rebuilt with `tauri build --debug --no-bundle` under the FIFO lock in 165.75 seconds. The unmodified seed had no `home/AppData/Local` before launch; it existed after bootstrap. The driver did not create it. Real attach, hydrated page, both measured viewports at DPR1, instance marker, contained process environment, no port-18800 requests, process/profile cleanup and unchanged live health/PID all passed again. Input desktop remained `Screen-saver`; foreground inspection remains explicitly unavailable. Scratch folder and compiled control host absence were verified after the Queen's cleanup.

T9 is proved with that recorded foreground limitation. T11-T13 results follow.

## Complete driver and assertion proofs

`npm run render:check` acquires the FIFO gate in a managed session, or an identity-checked local lock otherwise. It fingerprints the build inputs and executable, builds only when required, seeds a fresh short marked root, and launches only the guarded custom-protocol executable. The three instance settings are the only driver-supplied overrides. All browser flags, profile and home roots still come from Rust. The report preserves owned executable/PID/start identities and WebView descendants for cleanup review. Every run stops its exact owned tree and removes its instance profile in `finally`, including navigation failures and the deliberate mid-run failure.

The confirmed environment-normalization fix was rebuilt in 181.313 seconds. CDP and Review both passed afterwards; all nine actual child environment roots were contained and non-verbatim. The profile scaffold was absent before launch and present after bootstrap. The latest complete assertion/mutation pass reused that build and took 104.910 seconds, including 66.631 seconds observing the stall loop and sweep. A previously observed blank startup target is now guarded by waiting for the custom-protocol navigation before reload; failures preserve their URL, ready state and text for diagnosis.

Seven manifest screens are captured at both 1440x900 and 800x900: Launch, Focus, Grid, Kanban, expanded Sidebar, Review and Keyboard Shortcuts. Kanban captures each synthetic card after scrolling it into view, giving 16 PNGs in total. Private visual review confirmed the launch form, Markdown table/code, eight Grid panes, both Running/Completed cards, expanded agents and Recent entry, actual Review diff, and the real keyboard dialog. Narrow Grid clips composers, matching the recorded finding below. A blank terminal itself is expected without a PTY. PNG decoding checks actual pixels; no image or JSON output is committed or uploaded.

The installed-version badge is measured with `elementsFromPoint` and computed styles on the route that mounts it. No line-through or overlapping element was observed on normal screens at either width. Dashboard has no UpdateChecker component, so its absence is recorded rather than asserted. The modal backdrop intentionally interdicts the background badge; its observation is recorded separately under the approved scope ruling.

The Grid composer containment failures remain raw failures against [#414](https://github.com/rdfitted/hive-manager/issues/414), at load and after the timed observation. At 1440px, the normal composer's bottom is 460.797 versus pane bottom 407.188; the attention composer's bottom is 412.797. At 800px, the corresponding bottoms are 538.797 and 490.797 versus pane bottom 435.188. Known-finding acceptance requires a positive issue number, the expected card state and actual nonzero-box overflow. A changed failure, missing issue or unexpected pass blocks the command. No app layout was changed.

## Mutation and failure coverage

All 23 required render predicates have recorded before/mutant/restored results and fail by their assertion name. Mutate-test-restore stays inside the gate. The mutations are:

| Assertions | Mutation |
| --- | --- |
| `screen-not-empty` | Hide the expected screen container |
| `screenshot-has-pixels` | Cover the viewport with a solid white DOM layer |
| `viewport-1440`, `viewport-800` | Change the measured CSS width |
| `version-badge-unobscured` | Apply a computed line-through |
| `grid-eight-panes` | Hide one actual pane |
| Finished, stalled and exited-during-startup card/count checks (six) | Insert an unwanted card/badge or hide a required one |
| Both Grid composer containment checks | Establish a temporary contained DOM control, move it outside the pane, restore the real #414 layout |
| `chat-table`, `chat-code`, `chat-literal` | Remove the table, replace code text, inject an interpreted literal-HTML element |
| `kanban-fixture-present`, `sidebar-fixture-present` | Hide a real card or temporarily remove its actual sidebar node |
| `updater-invoke-rejected` | Replace the owned adapter's IPC call with a resolved response, reload it, restore exact source bytes |
| `all-screens-captured` | Remove a required capture record and restore it |
| `no-console-errors` | Emit a deliberate uniquely named browser console error |
| `no-live-api-request` | Fulfill a synthetic port-18800 URL locally inside Playwright; both observers detect it, no live service is contacted |

The ResourceTiming mutation initially exposed its default 250-entry buffer filling during the 66-second observation. The driver now allows 20,000 entries and emits a gating console error on buffer overflow. The corrected pass retained 458 normal resource entries, and both the request listener and ResourceTiming detected the locally fulfilled mutation. Normal console errors are never filtered to obtain a pass. Tauri freezes its invoke bridge, so the updater mutation targets the owned adapter instead of weakening the real bridge.

`--force-failure` exited 1 at the named deliberate failure after attach and environment verification. Cleanup, no-live-request and console checks passed; independent checks found no instance profile or owned WebView process, no process using the guarded executable, and a clean full `git status` against the committed driver. Every normal and forced run preserved the live app's health, executable identity, start time and PID. Foreground preservation remains unavailable on the Screen-saver input desktop and is never claimed as proved.

Validation: `npm run check` reports zero errors/warnings; full `npm test` passes 912 tests across 85 files in 124.33 seconds, without a flake rerun; the ten Node fixture/lifecycle/finding tests pass; `cargo check` and `cargo check --tests` pass. The stricter fixture change also passes the real Rust storage/controller loader test. G41 has named assertions and 23 restored mutation proofs; G42 preserves the live app; G43 has normal and forced-failure cleanup plus ignored output and clean git evidence.

## Windows CI decision

The measured cached rebuild plus the complete capture/proof run is about five minutes, within the approximately 15-minute render budget. A `windows-latest` workflow uses the existing pinned setup/cache actions, the same isolated target path, a 15-minute render step, and a missing-runtime bootstrap only on its disposable runner. Its YAML and every PowerShell step were parsed locally, and the relative Rust-cache target mapping was verified. The hosted job itself has not run yet; cold-cache duration is unmeasured. There is no artifact-upload step. Microsoft documents the [runtime detection and silent bootstrap procedure](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution); the [official deployment sample](https://github.com/MicrosoftEdge/WebView2Samples/blob/main/SampleApps/WebView2APISample/AppWindow.cpp) supplies the bootstrap link.
