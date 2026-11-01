use super::*;

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
    assert!(require_chatgpt(&json!({"account":{"type":"chatgpt"}})).is_ok());
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
    let schema:Value=serde_json::from_str(include_str!("schema-contract.json")).unwrap();
    assert_eq!(schema["version"],client::PINNED_VERSION);
    assert_eq!(schema["schema_sha256"],client::PINNED_SCHEMA_SHA256);
    let required=schema["surface"]["v2/TurnStartedNotification.json"]["required"].as_array().unwrap();
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
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'child.pid'),[string]$child.Id)
function Reply($m,$r){[Console]::Out.WriteLine((@{id=$m.id;result=$r}|ConvertTo-Json -Compress -Depth 16))}
function Notify($method,$params){[Console]::Out.WriteLine((@{method=$method;params=$params}|ConvertTo-Json -Compress -Depth 16))}
$seq=0;$turn=$null
while($null -ne ($line=[Console]::ReadLine())) {
 $m=$line|ConvertFrom-Json
 [IO.File]::AppendAllText((Join-Path $PSScriptRoot 'requests.jsonl'),$line+"`n")
 switch($m.method) {
  'initialize' {if(!(Test-Path (Join-Path $PSScriptRoot 'hang'))){Reply $m @{userAgent='codex-cli/0.166.0'}}}
  'account/read' {$kind='chatgpt';if(Test-Path (Join-Path $PSScriptRoot 'api-account')){$kind='apiKey'};Reply $m @{account=@{type=$kind};requiresOpenaiAuth=$true}}
  'thread/start' {Reply $m @{thread=@{id='held-fixture';cwd=$PSScriptRoot}}}
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
fn g15_startup_cancel_and_shim_tree_stop_and_natural_exit() {
    for mode in ["cancel", "stop", "exit", "app-exit"] {
        let (temp, manager, registry, session, agent) = fake_server(mode == "cancel");
        let cmd = temp.path().join("codex.cmd").to_string_lossy().to_string();
        let cwd = temp.path().to_string_lossy().to_string();
        let owner = manager.clone();
        let id = agent.clone();
        let spawn = std::thread::spawn(move || {
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
        wait_until(|| temp.path().join("child.pid").exists());
        let pid = std::fs::read_to_string(temp.path().join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
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
                wait_until(|| !manager.is_alive(&agent));
                wait_until(|| !child_alive(pid)); // Natural exit must clean descendants before explicit stop.
            } else if mode == "app-exit" {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                assert!(runtime.block_on(registry.stop_all()).is_empty());
            }
            manager.kill(&agent).unwrap();
        }
        wait_until(|| !child_alive(pid));
        assert!(
            !manager.is_alive(&agent),
            "G15 no live protocol runtime after cleanup"
        );
        assert!(
            registry.get(&session, &agent).is_none(),
            "G15 cleanup removes registration"
        );
        assert!(!manager.has_heartbeat_credential_for_test(&agent),"G15 cleanup revokes credential after stop_all removed registration");
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
