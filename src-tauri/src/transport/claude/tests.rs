use super::*;

#[test]
fn failed_completion_envelopes_emit_visible_error_and_keep_turn_end() {
    for (failed, details, expected) in [
        (
            true,
            json!({"subtype":"error_during_execution","errors":["synthetic Claude failure","second detail"]}),
            Some("synthetic Claude failure\nsecond detail"),
        ),
        (
            true,
            json!({"subtype":"success","result":"failure result text"}),
            Some("failure result text"),
        ),
        (
            true,
            json!({"subtype":"error_max_turns"}),
            Some("Claude turn failed. Inspect the raw event log for details."),
        ),
        (false, json!({"subtype":"success","result":"done"}), None),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
        let transport = ClaudeTransport::new(
            ClaudeOptions {
                command: "unused-fixture".into(),
                args: vec![],
                cwd: temp.path().into(),
                env: vec![],
                transcript_id: "held".into(),
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
            state.ready = true;
            state.init_checked = true;
            state.current = Some(correlate::TurnCorrelation::new(
                "held".into(),
                "input".into(),
            ));
        }
        let mut envelope = json!({"type":"result","session_id":"foreign","uuid":"result-uuid","parent_tool_use_id":null,"is_error":failed,"user_message_uuids":["input"],"duration_ms":1,"duration_api_ms":1,"num_turns":1,"total_cost_usd":0,"usage":{}});
        envelope
            .as_object_mut()
            .unwrap()
            .extend(details.as_object().unwrap().clone());
        transport.core.notification(envelope.clone());
        envelope["session_id"] = json!("held");
        envelope["parent_tool_use_id"] = json!("child-tool");
        transport.core.notification(envelope.clone());
        envelope["parent_tool_use_id"] = Value::Null;
        envelope["user_message_uuids"] = json!(["wrong-input"]);
        transport.core.notification(envelope.clone());
        assert!(registry
            .feeds
            .history("session", "worker", None, None, 100)
            .unwrap()
            .events
            .is_empty());
        envelope["user_message_uuids"] = json!(["input"]);
        transport.core.notification(envelope);
        let events = registry
            .feeds
            .history("session", "worker", None, None, 100)
            .unwrap()
            .events;
        assert_eq!(events[0].kind, AgentEventKind::TurnEnd);
        assert_eq!(events[0].payload["is_error"], failed);
        if let Some(message) = expected {
            assert_eq!(events.len(), 2);
            assert_eq!(events[1].kind, AgentEventKind::Error);
            assert_eq!(events[1].payload["message"], message);
            assert_eq!(events[1].payload["text"], message);
        } else {
            assert_eq!(
                events.len(),
                1,
                "successful completion does not invent an error"
            );
        }
    }
}

fn replay(session: &str, uuid: &str) -> Value {
    json!({"type":"user","session_id":session,"uuid":uuid,"isReplay":true,"parent_tool_use_id":null,
        "message":{"role":"user","content":"synthetic"}})
}
fn started(session: &str) -> Value {
    json!({"type":"stream_event","session_id":session,"parent_tool_use_id":null,
        "event":{"type":"message_start","message":{"id":"synthetic-message"}}})
}

#[test]
fn g12_claude_foreign_worker_child_and_receipt_alone_never_confirm() {
    let mut a = correlate::TurnCorrelation::new("held-a".into(), "same-input".into());
    let mut b = correlate::TurnCorrelation::new("held-b".into(), "same-input".into());
    for foreign in ["held-b", "child"] {
        a.observe(&replay(foreign, "same-input"));
        a.observe(&started(foreign));
    }
    // Exercise the session check before ProtocolConfirmation sees Hive's held IDs.
    assert_ne!(
        a.delivery().confirmed,
        Some(true),
        "G12 Claude foreign worker/child must never confirm parent"
    );
    let mut child = replay("held-a", "same-input");
    child["parent_tool_use_id"] = json!("child-tool");
    a.observe(&child);
    a.observe(&started("held-a"));
    assert_ne!(
        a.delivery().confirmed,
        Some(true),
        "G12 Claude nested user cannot accept parent input"
    );
    a.observe(&replay("held-a", "wrong-input"));
    assert_ne!(a.delivery().confirmed, Some(true));
    b.observe(&replay("held-b", "same-input"));
    assert_ne!(
        b.delivery().confirmed,
        Some(true),
        "replay is receipt, not a started turn"
    );
    a.observe(&replay("held-a", "same-input"));
    a.observe(&started("held-a"));
    assert_eq!(a.delivery().confirmed, Some(true));
    assert_ne!(b.delivery().confirmed, Some(true));
    b.observe(&started("held-b"));
    assert_eq!(b.delivery().confirmed, Some(true));
}

#[test]
fn result_matches_active_input_and_never_manufactures_start() {
    let mut state = correlate::TurnCorrelation::new("held".into(), "input".into());
    state.observe(&json!({"type":"result","session_id":"held","user_message_uuids":["wrong"]}));
    assert!(!state.completed);
    state.observe(&json!({"type":"result","session_id":"foreign","user_message_uuids":["input"]}));
    assert!(!state.completed);
    state.observe(&json!({"type":"result","session_id":"held","user_message_uuids":["input"]}));
    assert!(state.completed);
    assert_ne!(state.delivery().confirmed, Some(true));
}

#[test]
fn message_start_before_matching_replay_cannot_accept_input() {
    let mut state = correlate::TurnCorrelation::new("held".into(), "input".into());
    state.observe(&started("held"));
    state.observe(&replay("held", "input"));
    assert_ne!(
        state.delivery().confirmed,
        Some(true),
        "only a message start after matching replay proves the turn started"
    );
    state.observe(&started("held"));
    assert_eq!(state.delivery().confirmed, Some(true));
}

#[test]
fn injected_billing_selector_is_rejected_before_process_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let client = client::JsonClient::new();
    let error = client
        .spawn(
            "nonexistent-fixture-command",
            &[],
            temp.path(),
            &[("Anthropic_Api_Key".into(), "synthetic-secret".into())],
            Arc::new(|_| {}),
        )
        .unwrap_err();
    assert!(error.contains("ANTHROPIC_API_KEY"));
    assert!(!error.contains("synthetic-secret"));
    assert!(!client.alive.load(Ordering::Acquire));
}

#[test]
fn g13_claude_environment_and_account_fail_closed() {
    for name in [
        "ANTHROPIC_API_KEY",
        "anthropic_api_key",
        "ANTHROPIC_AUTH_TOKEN",
        "Anthropic_Profile",
        "CLAUDE_CODE_USE_BEDROCK",
        "claude_code_use_vertex",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_FUTURE",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_ORGANIZATION_ID",
    ] {
        let error =
            client::require_subscription_env([(name.into(), "synthetic-secret-never-log".into())])
                .unwrap_err();
        assert!(error.contains(&name.to_ascii_uppercase()));
        assert!(!error.contains("synthetic-secret-never-log"));
        // Presence, including an empty value, is deliberately refused.
        assert!(client::require_subscription_env([(name.into(), "".into())]).is_err());
    }
    assert!(client::require_subscription_env([(
        "CLAUDE_CODE_GIT_BASH_PATH".into(),
        "synthetic".into()
    )])
    .is_ok());
    assert!(
        client::require_subscription(&json!({"account":{"subscriptionType":"Claude Ultra"}}))
            .is_ok()
    );
    for account in [
        json!(null),
        json!({}),
        json!({"subscriptionType":"unknown"}),
        json!({"subscriptionType":"Claude Ultra","apiKeySource":"apiKeyHelper"}),
        json!({"subscriptionType":"Claude Ultra","apiKeySource":"environment"}),
        json!({"subscriptionType":"Claude Ultra","tokenSource":"provider"}),
        json!({"apiKeySource":"none"}),
    ] {
        assert!(client::require_subscription(&json!({"account":account})).is_err());
    }
    let mut command = std::process::Command::new("fixture");
    for key in crate::adapters::wsl::PARENT_SESSION_ENV_NAMES {
        command.env(key, "synthetic");
    }
    command.env("CLAUDE_CODE_GIT_BASH_PATH", "keep");
    client::remove_parent_env(
        &mut command,
        crate::adapters::wsl::PARENT_SESSION_ENV_NAMES
            .iter()
            .map(|k| ((*k).into(), "synthetic".into())),
    );
    assert!(command
        .get_envs()
        .filter(|(k, _)| crate::adapters::wsl::is_parent_session_env(&k.to_string_lossy()))
        .all(|(_, v)| v.is_none()));
    assert!(command
        .get_envs()
        .any(|(k, v)| k == "CLAUDE_CODE_GIT_BASH_PATH" && v.is_some()));
    assert!(client::STARTUP_TIMEOUT >= Duration::from_secs(120));
}

#[test]
fn pty_conversion_preserves_policy_model_effort_and_pinned_id() {
    let id = uuid::Uuid::new_v4().to_string();
    let options = ClaudeOptions::from_pty(
        "claude",
        &[
            "--session-id",
            &id,
            "--dangerously-skip-permissions",
            "--model",
            "synthetic-model",
            "--effort",
            "high",
            "initial task",
        ],
        ".".into(),
        vec![],
        "session".into(),
        "worker".into(),
    )
    .unwrap();
    let error = ClaudeOptions::from_pty(
        "claude",
        &[
            "--session-id",
            &id,
            "--dangerously-skip-permissions",
            "--settings=synthetic-secret",
        ],
        ".".into(),
        vec![],
        "session".into(),
        "worker".into(),
    )
    .err()
    .unwrap();
    assert!(error.contains("--settings") && !error.contains("synthetic-secret"));
    assert_eq!(options.transcript_id, id);
    assert_eq!(options.initial_prompt.as_deref(), Some("initial task"));
    assert_eq!(
        options
            .args
            .iter()
            .filter(|a| a.as_str() == "--session-id")
            .count(),
        1
    );
    assert!(options
        .args
        .windows(2)
        .any(|p| p == ["--session-id", id.as_str()]));
    assert!(options
        .args
        .windows(2)
        .any(|p| p == ["--permission-prompts", "none"]));
    assert!(options
        .args
        .windows(2)
        .any(|p| p == ["--model", "synthetic-model"]));
    assert!(options.args.windows(2).any(|p| p == ["--effort", "high"]));
    for flag in [
        "--bare",
        "--resume",
        "--settings",
        "--no-session-persistence",
        "--permission-mode",
    ] {
        assert!(ClaudeOptions::from_pty(
            "claude",
            &["--session-id", &id, "--dangerously-skip-permissions", flag],
            ".".into(),
            vec![],
            "session".into(),
            "worker".into()
        )
        .is_err());
    }
}

#[test]
fn synthetic_wire_normalizes_text_tools_and_skips_unknown() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("wire-fixture.json")).unwrap();
    let events: Vec<_> = fixtures
        .iter()
        .flat_map(|v| events::normalize("worker", v, Some("message")))
        .collect();
    for kind in [
        AgentEventKind::UserMessage,
        AgentEventKind::AssistantDelta,
        AgentEventKind::AssistantMessage,
        AgentEventKind::ToolStart,
        AgentEventKind::ToolResult,
    ] {
        assert!(events.iter().any(|e| e.kind == kind));
    }
    assert_eq!(events.len(), 5);
    assert!(events.iter().all(|e| e.agent_id == "worker"));
    for subtype in ["can_use_tool", "future_request", "elicitation"] {
        let response = events::server_reply(
            &json!({"type":"control_request","request_id":"request","request":{"subtype":subtype}}),
        );
        assert_eq!(response["response"]["request_id"], "request");
        assert!(
            response["response"]["subtype"] == "error"
                || response["response"]["response"]["behavior"] == "deny"
        );
    }
}

#[cfg(windows)]
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "bounded Claude fixture condition"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
// PID publication is a fixture signal, not a startup performance budget.
#[cfg(windows)]
fn wait_for_child_pid(root: &std::path::Path) -> u32 {
    loop {
        if let Ok(value) = std::fs::read_to_string(root.join("child.pid")) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn wait_for_signal(mut predicate: impl FnMut() -> bool) {
    while !predicate() {
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn child_alive(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn WaitForSingleObject(h: *mut c_void, t: u32) -> u32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    let h = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if h.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(h, 0) } == 258;
    unsafe {
        CloseHandle(h);
    }
    alive
}
#[cfg(windows)]
fn fixture(
    mode: &str,
) -> (
    tempfile::TempDir,
    Arc<crate::pty::PtyManager>,
    Arc<TransportRegistry>,
    String,
    String,
    String,
) {
    let temp = tempfile::Builder::new()
        .prefix("hive claude fixture ")
        .tempdir()
        .unwrap();
    std::fs::write(temp.path().join("mode"), mode).unwrap();
    let transcript = uuid::Uuid::new_v4().to_string();
    std::fs::write(temp.path().join("session-id"), &transcript).unwrap();
    std::fs::write(
        temp.path().join("server.ps1"),
        include_str!("fake-server.ps1"),
    )
    .unwrap();
    std::fs::write(temp.path().join("claude.cmd"),"@echo off\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0server.ps1\"\r\n").unwrap();
    let manager = Arc::new(crate::pty::PtyManager::new());
    let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
    manager.attach_transports(registry.clone());
    let session = uuid::Uuid::new_v4().to_string();
    let agent = format!("{session}-worker-1");
    manager.configure_transports(
        &session,
        [crate::domain::agent_transport::AgentTransportKind::ClaudeStreamJson]
            .into_iter()
            .collect(),
    );
    (temp, manager, registry, session, agent, transcript)
}
#[cfg(windows)]
fn launch(
    temp: &tempfile::TempDir,
    manager: &crate::pty::PtyManager,
    agent: &str,
    transcript: &str,
) -> Result<String, String> {
    manager
        .create_session(
            agent.into(),
            crate::pty::AgentRole::Worker {
                index: 1,
                parent: None,
            },
            &temp.path().join("claude.cmd").to_string_lossy(),
            &["--session-id", transcript, "--dangerously-skip-permissions"],
            Some(&temp.path().to_string_lossy()),
            80,
            24,
        )
        .map_err(|error| error.to_string())
}
#[cfg(windows)]
fn requests(temp: &tempfile::TempDir) -> Vec<Value> {
    std::fs::read_to_string(temp.path().join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(windows)]
#[test]
fn fake_claude_staging_flush_fifo_version_and_billing_guards() {
    let (temp, manager, registry, session, agent, transcript) = fixture("version");
    launch(&temp, &manager, &agent, &transcript).unwrap();
    let transport = registry.get(&session, &agent).unwrap().transport;
    assert!(!transport.capabilities().has_terminal);
    assert!(transport.send_input("staged", false).unwrap().is_pending());
    assert!(!requests(&temp).iter().any(|v| v["type"] == "user"));
    assert_eq!(
        transport.send_input("", true).unwrap().confirmed,
        Some(true)
    );
    assert!(transport.send_input("queued", true).unwrap().is_pending());
    transport.interrupt_sync().unwrap();
    wait_until(|| {
        requests(&temp)
            .iter()
            .filter(|v| v["type"] == "user")
            .count()
            == 2
    });
    let users: Vec<_> = requests(&temp)
        .into_iter()
        .filter(|v| v["type"] == "user")
        .collect();
    assert_eq!(users[0]["message"]["content"], "staged");
    assert_eq!(users[1]["message"]["content"], "queued");
    assert_eq!(users[0]["session_id"], transcript);
    assert_eq!(users[0]["priority"], "later");
    let page = registry
        .feeds
        .history(&session, &agent, None, None, 200)
        .unwrap();
    assert!(page
        .events
        .iter()
        .any(|e| e.payload["status"] == "version_mismatch"));
    assert!(page
        .events
        .iter()
        .any(|e| e.kind == AgentEventKind::TurnStart));
    assert!(page
        .events
        .iter()
        .any(|e| e.kind == AgentEventKind::ToolStart));
    manager.kill(&agent).unwrap();
    for mode in ["api-account", "api-source"] {
        let (temp, manager, registry, session, agent, transcript) = fixture(mode);
        let result = launch(&temp, &manager, &agent, &transcript);
        if mode == "api-account" {
            assert!(result.is_err());
            assert!(
                !requests(&temp).iter().any(|v| v["type"] == "user"),
                "G13 API account cannot receive first prompt"
            );
        } else {
            result.unwrap();
            let transport = registry.get(&session, &agent).unwrap().transport;
            assert!(
                transport.send_input("synthetic", true).is_err(),
                "G13 invalid runtime source stops the turn"
            );
        }
        manager.kill(&agent).unwrap();
        assert!(!manager.has_heartbeat_credential_for_test(&agent));
    }
}

#[cfg(windows)]
#[test]
fn g15_claude_startup_cancel_stop_exit_and_app_shutdown_reap_descendants() {
    for mode in ["cancel", "stop", "exit", "app-exit"] {
        let (temp, manager, registry, session, agent, transcript) = fixture(mode);
        let owner = manager.clone();
        let id = agent.clone();
        let path = temp.path().to_path_buf();
        let spawn = std::thread::spawn(move || {
            owner.create_session(
                id,
                crate::pty::AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                &path.join("claude.cmd").to_string_lossy(),
                &[
                    "--session-id",
                    &transcript,
                    "--dangerously-skip-permissions",
                ],
                Some(&path.to_string_lossy()),
                80,
                24,
            )
        });
        let pid = wait_for_child_pid(temp.path());
        assert!(child_alive(pid));
        if mode == "cancel" {
            manager.kill(&agent).unwrap();
            assert!(spawn.join().unwrap().is_err());
        } else {
            spawn.join().unwrap().unwrap();
            assert!(manager.has_heartbeat_credential_for_test(&agent));
            let transport = registry.get(&session, &agent).unwrap().transport;
            if mode == "exit" {
                assert_eq!(
                    transport.send_input("synthetic", true).unwrap().confirmed,
                    Some(true)
                );
                assert!(transport.interrupt_sync().is_err());
                wait_for_signal(|| !transport.is_alive());
                wait_for_signal(|| !child_alive(pid)); // Observe natural-exit cleanup before explicit stop.
            } else if mode == "app-exit" {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                assert!(runtime.block_on(registry.stop_all()).is_empty());
            }
            manager.kill(&agent).unwrap();
        }
        wait_for_signal(|| !child_alive(pid));
        assert!(
            !manager.is_alive(&agent),
            "G15 Claude no live runtime after cleanup"
        );
        assert!(registry.get(&session, &agent).is_none());
        assert!(!manager.has_heartbeat_credential_for_test(&agent));
        manager.kill(&agent).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn g15_claude_direct_writer_failure_cleans_owned_tree() {
    let (temp, _, registry, session, agent, transcript) = fixture("closed-stdin");
    let options = ClaudeOptions {
        command: "powershell.exe".into(),
        args: vec![
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
            ("CLAUDECODE".into(), "synthetic".into()),
            ("HIVE_AGENT_ID".into(), agent.clone()),
        ],
        transcript_id: transcript,
        initial_prompt: None,
        session_id: session.clone(),
        agent_id: agent.clone(),
    };
    let transport = ClaudeTransport::new(options, &registry);
    transport.start_blocking().unwrap();
    registry
        .register(&session, &agent, transport.thread_id(), transport.clone())
        .unwrap();
    transport.activate().unwrap();
    wait_for_signal(|| temp.path().join("input-closed").exists());
    let pid = wait_for_child_pid(temp.path());
    assert!(transport.send_input("synthetic", true).is_err());
    wait_for_signal(|| !child_alive(pid));
    let environment: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("environment.json")).unwrap())
            .unwrap();
    assert_eq!(environment["parent_marker_present"], false);
    assert_eq!(environment["agent"], agent);
    registry.stop_agent_sync(&session, &agent).unwrap();
}

#[cfg(windows)]
#[test]
fn g15_claude_drop_unregistered_runtime_and_bounded_control_failure() {
    for mode in ["stop", "cancel"] {
        let (temp, _, registry, session, agent, transcript) = fixture(mode);
        let mut options = ClaudeOptions::from_pty(
            &temp.path().join("claude.cmd").to_string_lossy(),
            &[
                "--session-id",
                &transcript,
                "--dangerously-skip-permissions",
            ],
            temp.path().into(),
            vec![],
            session,
            agent,
        )
        .unwrap();
        // Arguments are ignored by the synthetic shim, which reads only TempDir files.
        options.initial_prompt = None;
        let transport = ClaudeTransport::new(options, &registry);
        if mode == "cancel" {
            let weak = Arc::downgrade(&transport.core);
            transport
                .core
                .client
                .spawn(
                    &transport.core.options.command,
                    &transport.core.options.args,
                    temp.path(),
                    &[],
                    Arc::new(move |v| {
                        if let Some(core) = weak.upgrade() {
                            core.notification(v);
                        }
                    }),
                )
                .unwrap();
            let error = transport
                .core
                .client
                .request(
                    json!({"subtype":"initialize"}),
                    Instant::now() + Duration::from_millis(100),
                )
                .unwrap_err();
            assert!(error.contains("deadline") && error.contains("PTY"));
        } else {
            transport.start_blocking().unwrap();
        }
        let pid = wait_for_child_pid(temp.path());
        assert!(child_alive(pid));
        drop(transport);
        wait_for_signal(|| !child_alive(pid));
    }
}
