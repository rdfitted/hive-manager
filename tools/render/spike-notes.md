# Webview driver spike

Status: investigation in progress; no driver accepted yet. No normal debug instance was started.

## Observed

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

## Next bounded experiment

Requested from the app-source owner: provide the same debug-gated CDP arguments through `WindowConfig.additional_browser_args`, retaining all profile/environment isolation and browser defaults. No registry change or guard weakening is proposed. Driver and viewport selection waits for a successful real attach.
