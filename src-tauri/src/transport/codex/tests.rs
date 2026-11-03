use super::*;

#[test]
fn failed_completion_envelopes_emit_visible_error_and_keep_turn_end() {
    for (status, error, expected) in [
        (
            "failed",
            json!({"message":"synthetic Codex failure","codexErrorInfo":"other","additionalDetails":null}),
            Some("synthetic Codex failure"),
        ),
        (
            "failed",
            Value::Null,
            Some("Codex turn failed. Inspect the raw event log for details."),
        ),
        ("completed", Value::Null, None),
        ("interrupted", Value::Null, None),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
        // No process/home access: exercise the real notification -> persisted feed path.
        let transport = CodexTransport::new(
            CodexOptions {
                command: "unused-fixture".into(),
                server_args: vec![],
                cwd: temp.path().into(),
                env: vec![],
                model: None,
                effort: None,
                initial_prompt: None,
                session_id: "session".into(),
                agent_id: "worker".into(),
            },
            &registry,
        );
        registry
            .register("session", "worker", Some("held".into()), transport.clone())
            .unwrap();
        {
            let mut state = transport.core.state.lock();
            state.thread = Some("held".into());
            state.ready = true;
        }
        let mut envelope = json!({"method":"turn/completed","params":{"threadId":"foreign","turn":{"id":"turn","items":[],"status":status,"error":error}}});
        transport.core.notification(envelope.clone());
        assert!(registry
            .feeds
            .history("session", "worker", None, None, 100)
            .unwrap()
            .events
            .is_empty());
        envelope["params"]["threadId"] = json!("held");
        transport.core.notification(envelope);
        let events = registry
            .feeds
            .history("session", "worker", None, None, 100)
            .unwrap()
            .events;
        assert_eq!(events[0].kind, AgentEventKind::TurnEnd);
        assert_eq!(events[0].payload["status"], status);
        if let Some(message) = expected {
            assert_eq!(events.len(), 2);
            assert_eq!(events[1].kind, AgentEventKind::Error);
            assert_eq!(events[1].payload["message"], message);
            assert_eq!(events[1].payload["text"], message);
        } else {
            assert_eq!(
                events.len(),
                1,
                "successful/interrupted completion does not invent an error"
            );
        }
    }
}

#[test]
fn g12_workers_and_native_child_never_cross_confirm() {
    let mut a = correlate::TurnCorrelation::new("held-a".into());
    let mut b = correlate::TurnCorrelation::new("held-b".into());
    // Identical turn IDs deliberately force the thread check to do real work.
    a.observe("held-b", "same-turn", false);
    a.observe("held-child", "same-turn", false);
    a.response("same-turn");
    b.response("same-turn");
    assert_ne!(
        a.delivery().confirmed,
        Some(true),
        "G12 foreign worker/child must never confirm parent"
    );
    assert_ne!(
        b.delivery().confirmed,
        Some(true),
        "G12 response alone must not confirm"
    );
    a.observe("held-a", "wrong-turn", false);
    assert_ne!(
        a.delivery().confirmed,
        Some(true),
        "G12 wrong turn must not confirm"
    );
    a.observe("held-a", "same-turn", false);
    assert_eq!(a.delivery().confirmed, Some(true));
    assert_ne!(b.delivery().confirmed, Some(true));
    b.observe("held-b", "same-turn", false);
    assert_eq!(b.delivery().confirmed, Some(true));
}

#[test]
fn early_notifications_are_buffered_and_completion_is_not_confirmation() {
    let mut early = correlate::TurnCorrelation::new("held".into());
    early.observe("held", "turn", false);
    early.observe("held", "turn", true);
    assert_ne!(early.delivery().confirmed, Some(true));
    early.response("turn");
    assert_eq!(early.delivery().confirmed, Some(true));
    assert!(early.completed());
    let mut completed = correlate::TurnCorrelation::new("held".into());
    completed.observe("held", "turn", true);
    completed.response("turn");
    assert!(completed.completed());
    assert_ne!(completed.delivery().confirmed, Some(true));
}

#[test]
fn g13_auth_and_environment_guard_before_any_turn() {
    for kind in ["apiKey", "amazonBedrock", "gcp", ""] {
        assert!(require_chatgpt(&json!({"account":{"type":kind}})).is_err());
    }
    assert!(require_chatgpt(&json!({"account":null})).is_err());
    assert!(
        require_chatgpt(&json!({"account":{"type":"chatgpt"},"requiresOpenaiAuth":true})).is_ok()
    );
    assert!(
        require_chatgpt(&json!({"account":{"type":"chatgpt"},"requiresOpenaiAuth":false})).is_err()
    );
    let mut cmd = std::process::Command::new("fixture");
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "openai_api_key",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_GIT_BASH_PATH",
        "CODEX_HOME",
    ] {
        cmd.env(key, "synthetic");
    }
    client::remove_parent_and_billing_env(
        &mut cmd,
        [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "openai_api_key",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
        ]
        .into_iter()
        .map(|k| (k.into(), "synthetic".into())),
    );
    let values: std::collections::HashMap<_, _> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect();
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "openai_api_key",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
    ] {
        let matching: Vec<_> = values
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(key))
            .collect();
        assert!(
            !matching.is_empty() && matching.iter().all(|(_, value)| value.is_none()),
            "G13 must remove every case variant of {key}"
        );
    }
    assert_eq!(values["CODEX_HOME"].as_deref(), Some("synthetic"));
    assert_eq!(
        values["CLAUDE_CODE_GIT_BASH_PATH"].as_deref(),
        Some("synthetic")
    );
    assert!(client::STARTUP_TIMEOUT >= Duration::from_secs(120));
    assert_eq!(client::PINNED_SCHEMA_SHA256.len(), 64);
    let schema: Value = serde_json::from_str(include_str!("schema-contract.json")).unwrap();
    assert_eq!(schema["version"], client::PINNED_VERSION);
    assert_eq!(schema["schema_sha256"], client::PINNED_SCHEMA_SHA256);
    let required = schema["surface"]["v2/TurnStartedNotification.json"]["required"]
        .as_array()
        .unwrap();
    assert!(required.contains(&json!("threadId")) && required.contains(&json!("turn")));
}

#[test]
fn every_server_request_gets_an_answer_and_unknown_gets_error() {
    for method in [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "item/permissions/requestApproval",
        "item/tool/requestUserInput",
        "mcpServer/elicitation/request",
        "item/tool/call",
        "account/chatgptAuthTokens/refresh",
        "attestation/generate",
        "currentTime/read",
        "execCommandApproval",
        "applyPatchApproval",
        "future/unknown",
    ] {
        let reply = events::server_reply(
            &json!({"id":"synthetic-request","method":method,"params":{"threadId":"held"}}),
        );
        assert_eq!(reply["id"], "synthetic-request");
        assert!(
            reply.get("result").is_some() || reply.get("error").is_some(),
            "{method} must not hang"
        );
        if method == "future/unknown" {
            assert!(reply.get("error").is_some());
        }
        if matches!(method, "execCommandApproval" | "applyPatchApproval") {
            assert_eq!(reply["result"]["decision"], "abort");
        }
    }
}

#[test]
fn normalization_never_attributes_unheld_threads_and_child_is_activity() {
    assert!(events::normalize(
        "worker",
        "held",
        "turn/started",
        &json!({"threadId":"foreign","turn":{"id":"turn"}}),
        false
    )
    .is_none());
    let child = events::normalize(
        "worker",
        "held",
        "turn/started",
        &json!({"threadId":"child","turn":{"id":"turn"}}),
        true,
    )
    .unwrap();
    assert_eq!(child.agent_id, "worker");
    assert_eq!(child.kind, AgentEventKind::Status);
    assert_eq!(child.payload["parent_thread_id"], "held");
    let text=events::normalize("worker","held","item/completed",&json!({"threadId":"held","item":{"id":"message","type":"agentMessage","text":"synthetic"}}),false).unwrap();
    assert_eq!(text.kind, AgentEventKind::AssistantMessage);
    assert_eq!(text.payload["text"], "synthetic");
}

#[test]
fn config_and_native_feature_flags_match_pty_without_forcing_delegation() {
    let temp = tempfile::tempdir().unwrap();
    let options = CodexOptions::from_pty(
        "codex",
        &[
            "--dangerously-bypass-approvals-and-sandbox",
            "-m",
            "fixture-model",
            "-c",
            "model_reasoning_effort=\"xhigh\"",
            "--disable",
            "multi_agent",
            "Read fixture and execute.",
        ],
        temp.path().into(),
        vec![],
        "fixture".into(),
        "fixture-worker".into(),
    )
    .unwrap();
    assert_eq!(options.model.as_deref(), Some("fixture-model"));
    assert_eq!(options.effort.as_deref(), Some("xhigh"));
    assert!(options
        .server_args
        .windows(2)
        .any(|p| p == ["--disable", "multi_agent"]));
    assert!(!options.server_args.iter().any(|p| p == "--enable"));
    assert_eq!(
        options.initial_prompt.as_deref(),
        Some("Read fixture and execute.")
    );
    assert!(options
        .server_args
        .iter()
        .any(|p| p.starts_with("sqlite_home=")));
}

#[cfg(windows)]
fn fake_server(
    hang: bool,
) -> (
    tempfile::TempDir,
    Arc<crate::pty::PtyManager>,
    Arc<TransportRegistry>,
    String,
    String,
) {
    let temp = tempfile::Builder::new()
        .prefix("hive codex fixture ")
        .tempdir()
        .unwrap();
    let script = r#"
$ErrorActionPreference='Stop'
$child=Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList '-NoProfile -NonInteractive -Command Start-Sleep -Seconds 300' -WindowStyle Hidden -PassThru
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'child.pid.pending'),[string]$child.Id)
[IO.File]::Move((Join-Path $PSScriptRoot 'child.pid.pending'), (Join-Path $PSScriptRoot 'child.pid'))
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'environment.json'),(@{billing_present=[bool]($env:OPENAI_API_KEY -or $env:CODEX_API_KEY);parent_marker_present=[bool]$env:CLAUDECODE;agent=$env:HIVE_AGENT_ID;token_present=[bool]$env:HIVE_AGENT_TOKEN}|ConvertTo-Json -Compress))
function Reply($m,$r){[Console]::Out.WriteLine((@{id=$m.id;result=$r}|ConvertTo-Json -Compress -Depth 16))}
function Notify($method,$params){[Console]::Out.WriteLine((@{method=$method;params=$params}|ConvertTo-Json -Compress -Depth 16))}
$seq=0;$turn=$null
while($null -ne ($line=[Console]::ReadLine())) {
 $m=$line|ConvertFrom-Json
 [IO.File]::AppendAllText((Join-Path $PSScriptRoot 'requests.jsonl'),$line+"`n")
 switch($m.method) {
  'initialize' {if(!(Test-Path (Join-Path $PSScriptRoot 'hang'))){Reply $m @{userAgent='codex-cli/0.166.0'}}}
  'account/read' {$kind='chatgpt';if(Test-Path (Join-Path $PSScriptRoot 'api-account')){$kind='apiKey'};Reply $m @{account=@{type=$kind};requiresOpenaiAuth=$true}}
  'thread/start' {Reply $m @{thread=@{id='held-fixture';cwd=$PSScriptRoot};modelProvider='openai'};if(Test-Path (Join-Path $PSScriptRoot 'close-input')){Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class PipeClose{[DllImport("kernel32.dll")]public static extern IntPtr GetStdHandle(int n);[DllImport("kernel32.dll")]public static extern bool CloseHandle(IntPtr h);}';[PipeClose]::CloseHandle([PipeClose]::GetStdHandle(-10))|Out-Null;[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'input-closed'),'yes');Start-Sleep -Seconds 300}}
  'turn/start' {$seq++;$turn="turn-$seq";Notify 'turn/started' @{threadId='held-fixture';turn=@{id=$turn;status='inProgress'}};Reply $m @{turn=@{id=$turn;status='inProgress'}}}
  'turn/interrupt' {if($m.params.turnId -ne $turn){[Console]::Out.WriteLine((@{id=$m.id;error=@{code=-32602;message='wrong turn'}}|ConvertTo-Json -Compress -Depth 8));break};if(Test-Path (Join-Path $PSScriptRoot 'natural-exit')){exit 0};Notify 'turn/completed' @{threadId='held-fixture';turn=@{id=$turn;status='interrupted'}};Reply $m @{}}
  'fixture/exit' {exit 0}
 }
}
"#;
    std::fs::write(temp.path().join("server.ps1"), script).unwrap();
    std::fs::write(temp.path().join("codex.cmd"),"@echo off\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0server.ps1\"\r\n").unwrap();
    if hang {
        std::fs::write(temp.path().join("hang"), "").unwrap();
    }
    let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
    let manager = Arc::new(crate::pty::PtyManager::new());
    manager.attach_transports(registry.clone());
    let session = uuid::Uuid::new_v4().to_string();
    let agent = format!("{session}-worker-1");
    manager.configure_transports(
        &session,
        [crate::domain::agent_transport::AgentTransportKind::CodexAppServer]
            .into_iter()
            .collect(),
    );
    (temp, manager, registry, session, agent)
}

#[cfg(windows)]
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "bounded fake server condition");
        std::thread::sleep(Duration::from_millis(25));
    }
}

// Readiness is the pass criterion; this ceiling only fails a broken fixture.
// Descendants sleep for 300 s: fail well before natural exit could hide a leak.
#[cfg(windows)]
const FIXTURE_WAIT_FAILSAFE: Duration = Duration::from_secs(120);

#[cfg(windows)]
fn spawn_with_startup_result<T: Send + 'static, E: std::fmt::Display + Send + 'static>(
    start: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> (
    std::thread::JoinHandle<Result<T, E>>,
    std::sync::mpsc::Receiver<Result<(), String>>,
) {
    let (finished, result) = std::sync::mpsc::channel();
    let startup = std::thread::spawn(move || {
        let result = start();
        let _ = finished.send(result.as_ref().map(|_| ()).map_err(|error| error.to_string()));
        result
    });
    (startup, result)
}

#[cfg(windows)]
fn wait_for_child_pid(root: &std::path::Path) -> u32 {
    wait_for_child_pid_with_startup(root, None)
}

#[cfg(windows)]
fn wait_for_child_pid_with_startup(
    root: &std::path::Path,
    startup: Option<&std::sync::mpsc::Receiver<Result<(), String>>>,
) -> u32 {
    let mut pid = None;
    let mut startup_completed = false;
    wait_for_signal("child PID publication", || {
        if let Some(startup) = startup {
            match startup.try_recv() {
                Ok(Err(error)) => panic!("fixture startup failed before child PID publication: {error}"),
                Ok(Ok(())) => startup_completed = true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) if !startup_completed => {
                    panic!("fixture startup thread exited without reporting its result");
                }
                Err(_) => {}
            }
        }
        pid = std::fs::read_to_string(root.join("child.pid"))
            .ok()
            .and_then(|value| value.trim().parse().ok());
        pid.is_some()
    });
    pid.unwrap()
}

#[cfg(windows)]
fn wait_for_signal(name: &str, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(
            started.elapsed() < FIXTURE_WAIT_FAILSAFE,
            "fixture failsafe expired waiting for {name}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    eprintln!("fixture_wait_ms name={name} elapsed={}", started.elapsed().as_millis());
}

#[cfg(windows)]
#[test]
#[should_panic(expected = "fixture startup failed before child PID publication: synthetic startup failure")]
fn g15_startup_failure_is_reported_before_pid() {
    let temp = tempfile::tempdir().unwrap();
    let (startup, result) = spawn_with_startup_result(|| Err::<(), _>("synthetic startup failure"));
    assert!(startup.join().unwrap().is_err());
    wait_for_child_pid_with_startup(temp.path(), Some(&result));
}

#[cfg(windows)]
fn child_alive(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, timeout: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(handle, 0) } == 258;
    unsafe {
        CloseHandle(handle);
    }
    alive
}

#[cfg(windows)]
#[test]
fn manager_shutdown_cancels_stalled_initialize_without_blocking_unrelated_pty() {
    let (temp, manager, registry, session, agent) = fake_server(true);
    let owner = manager.clone();
    let id = agent.clone();
    let path = temp.path().to_path_buf();
    let start = std::thread::spawn(move || {
        owner.create_session(
            id,
            crate::pty::AgentRole::Worker {
                index: 1,
                parent: None,
            },
            &path.join("codex.cmd").to_string_lossy(),
            &[],
            Some(&path.to_string_lossy()),
            80,
            24,
        )
    });
    wait_until(|| {
        std::fs::read_to_string(temp.path().join("requests.jsonl"))
            .unwrap_or_default()
            .contains("initialize")
    });
    let pid = wait_for_child_pid(temp.path());
    let (sent, received) = std::sync::mpsc::channel();
    let pty_owner = manager.clone();
    let unrelated = std::thread::spawn(move || {
        let result = pty_owner
            .create_session(
                "unrelated-pty".into(),
                crate::pty::AgentRole::ScratchShell,
                "fixture",
                &[],
                None,
                80,
                24,
            )
            .and_then(|_| pty_owner.kill("unrelated-pty"));
        sent.send(result.is_ok()).unwrap();
    });
    let responsive = received.recv_timeout(Duration::from_secs(2));
    let errors = manager.shutdown_transports();
    assert!(start.join().unwrap().is_err());
    unrelated.join().unwrap();
    assert!(errors.is_empty(), "shutdown errors: {errors:?}");
    assert_eq!(
        responsive.unwrap_or(false),
        true,
        "unrelated PTY create/kill must finish while protocol initialize stalls"
    );
    wait_until(|| !child_alive(pid));
    assert!(registry.get(&session, &agent).is_none());
    assert!(
        !manager.has_heartbeat_credential_for_test(&agent),
        "cancelled start cannot bind a late heartbeat token"
    );
    assert!(manager.shutdown_transports().is_empty());
}

#[cfg(windows)]
#[test]
fn g15_startup_cancel_and_shim_tree_stop_and_natural_exit() {
    for mode in ["cancel", "stop", "exit", "app-exit", "closed-stdin"] {
        let (temp, manager, registry, session, agent) = fake_server(mode == "cancel");
        if mode == "closed-stdin" {
            std::fs::write(temp.path().join("close-input"), "").unwrap();
            // A cmd shim retains its own stdin read handle. Use a direct fake
            // server here so closing stdin genuinely breaks the writer pipe.
            let options = CodexOptions {
                command: "powershell.exe".into(),
                server_args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    temp.path()
                        .join("server.ps1")
                        .to_string_lossy()
                        .into_owned(),
                ],
                cwd: temp.path().into(),
                env: vec![
                    ("OPENAI_API_KEY".into(), "synthetic".into()),
                    ("CODEX_API_KEY".into(), "synthetic".into()),
                    ("CLAUDECODE".into(), "synthetic".into()),
                    ("HIVE_AGENT_ID".into(), agent.clone()),
                ],
                model: None,
                effort: None,
                initial_prompt: None,
                session_id: session.clone(),
                agent_id: agent.clone(),
            };
            let transport = CodexTransport::new(options, &registry);
            transport.start_blocking().unwrap();
            registry
                .register(&session, &agent, transport.thread_id(), transport.clone())
                .unwrap();
            transport.activate().unwrap();
            wait_for_signal("input-closed publication", || temp.path().join("input-closed").exists());
            let environment: Value = serde_json::from_slice(
                &std::fs::read(temp.path().join("environment.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(environment["billing_present"], false);
            assert_eq!(environment["parent_marker_present"], false);
            assert_eq!(environment["agent"], agent);
            assert_eq!(environment["token_present"], false);
            let pid = wait_for_child_pid(temp.path());
            assert!(
                transport.send_input("synthetic", true).is_err(),
                "closed server stdin must fail the writer"
            );
            wait_for_signal("descendant process exit", || !child_alive(pid)); // Writer owns cleanup before explicit stop.
            registry.stop_agent_sync(&session, &agent).unwrap();
            continue;
        }
        let cmd = temp.path().join("codex.cmd").to_string_lossy().to_string();
        let cwd = temp.path().to_string_lossy().to_string();
        let owner = manager.clone();
        let id = agent.clone();
        let (spawn, startup_result) = spawn_with_startup_result(move || {
            owner.create_session(
                id,
                crate::pty::AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                &cmd,
                &[],
                Some(&cwd),
                80,
                24,
            )
        });
        let pid = wait_for_child_pid_with_startup(temp.path(), Some(&startup_result));
        assert!(child_alive(pid), "fixture descendant must start");
        if mode == "cancel" {
            manager.kill(&agent).unwrap();
            assert!(spawn.join().unwrap().is_err());
        } else {
            spawn.join().unwrap().unwrap();
            assert!(manager.has_heartbeat_credential_for_test(&agent));
            assert!(
                manager.is_alive(&agent),
                "PTY-less worker survives startup liveness"
            );
            assert!(registry.get(&session, &agent).is_some());
            if mode == "exit" {
                let entry = registry.get(&session, &agent).unwrap();
                assert_eq!(
                    entry
                        .transport
                        .send_input("synthetic", true)
                        .unwrap()
                        .confirmed,
                    Some(true)
                );
                std::fs::write(temp.path().join("natural-exit"), "").unwrap();
                assert!(entry.transport.interrupt_sync().is_err());
                wait_for_signal("protocol runtime exit", || !manager.is_alive(&agent));
                wait_for_signal("descendant process exit", || !child_alive(pid)); // Natural exit must clean descendants before explicit stop.
            } else if mode == "app-exit" {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                assert!(runtime.block_on(registry.stop_all()).is_empty());
            }
            manager.kill(&agent).unwrap();
        }
        wait_for_signal("descendant process exit", || !child_alive(pid));
        assert!(
            !manager.is_alive(&agent),
            "G15 no live protocol runtime after cleanup"
        );
        assert!(
            registry.get(&session, &agent).is_none(),
            "G15 cleanup removes registration"
        );
        assert!(
            !manager.has_heartbeat_credential_for_test(&agent),
            "G15 cleanup revokes credential after stop_all removed registration"
        );
        manager.kill(&agent).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn fake_rpc_staging_flush_fifo_and_billing_refusal() {
    let (temp, manager, registry, session, agent) = fake_server(false);
    let cmd = temp.path().join("codex.cmd").to_string_lossy().to_string();
    let cwd = temp.path().to_string_lossy().to_string();
    manager
        .create_session(
            agent.clone(),
            crate::pty::AgentRole::Worker {
                index: 1,
                parent: None,
            },
            &cmd,
            &[],
            Some(&cwd),
            80,
            24,
        )
        .unwrap();
    let entry = registry.get(&session, &agent).unwrap();
    assert!(entry
        .transport
        .send_input("first", false)
        .unwrap()
        .is_pending());
    assert_eq!(
        entry.transport.send_input("", true).unwrap().confirmed,
        Some(true)
    );
    assert!(entry
        .transport
        .send_input("second", true)
        .unwrap()
        .is_pending());
    assert!(entry
        .transport
        .send_input("third", true)
        .unwrap()
        .is_pending());
    let turns = || -> Vec<Value> {
        std::fs::read_to_string(temp.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|m| m["method"] == "turn/start")
            .collect()
    };
    assert_eq!(turns().len(), 1, "active-turn sends must queue");
    entry.transport.interrupt_sync().unwrap();
    wait_until(|| turns().len() == 2);
    wait_until(|| entry.transport.interrupt_sync().is_ok());
    wait_until(|| turns().len() == 3);
    assert_eq!(
        turns()
            .iter()
            .map(|m| m["params"]["input"][0]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    wait_until(|| entry.transport.interrupt_sync().is_ok());
    assert!(entry.transport.send_input("", true).unwrap().is_pending());
    assert_eq!(turns().len(), 3, "empty flush must be a no-op");
    assert!(manager
        .write(&agent, b"\t")
        .unwrap_err()
        .to_string()
        .contains("no terminal"));
    manager.kill(&agent).unwrap();

    let (temp, manager, registry, session, agent) = fake_server(false);
    std::fs::write(temp.path().join("api-account"), "").unwrap();
    let error = manager
        .create_session(
            agent.clone(),
            crate::pty::AgentRole::Worker {
                index: 1,
                parent: None,
            },
            temp.path().join("codex.cmd").to_str().unwrap(),
            &[],
            temp.path().to_str(),
            80,
            24,
        )
        .unwrap_err();
    assert!(error.to_string().contains("ChatGPT login"));
    assert!(registry.get(&session, &agent).is_none());
    let requests = std::fs::read_to_string(temp.path().join("requests.jsonl")).unwrap();
    assert!(
        !requests.contains("turn/start"),
        "G13 must refuse billing auth before any turn"
    );
    assert!(!requests.contains("thread/start"));
}
