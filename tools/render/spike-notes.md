# Webview driver spike

Status: real CDP attach and viewport measurements proved with a temporary synthetic-profile directory. The bootstrap fix landed in 1ea4046; a rebuilt run without the temporary driver line is pending. No normal debug instance was started.

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

Chosen driver: Playwright CDP. Chosen viewport method: CDP emulation via `page.setViewportSize`, with actual renderer measurements. The permanent bootstrap folder fix is 1ea4046. The temporary directory-creation line has been removed from the spike. After the Queen confirms the fix, a rebuilt no-scaffold spike must repeat this proof before T11 opens. T11-T13 and the complete G41/G43 mutation gate remain pending.
