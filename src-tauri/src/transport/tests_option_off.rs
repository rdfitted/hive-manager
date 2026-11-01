use crate::domain::agent_transport::{selected_transport, AgentTransportKind, AgentTransports};
use crate::pty::AgentRole;

#[test]
fn g11_command_arguments_match_b044969_recording() {
    // Captured from b044969 controller::build_command. Normalize only the fresh
    // Claude UUID; it is deliberately generated once for every PTY launch.
    for (cli, expected_command, expected) in [
        (
            "claude",
            "claude",
            vec![
                "--session-id",
                "<fresh-session-id>",
                "--dangerously-skip-permissions",
                "--model",
                "fixture-model",
                "--fixture-flag",
            ],
        ),
        (
            "codex",
            "codex",
            vec![
                "--dangerously-bypass-approvals-and-sandbox",
                "-m",
                "fixture-model",
                "--fixture-flag",
            ],
        ),
        (
            "cursor",
            "wsl",
            vec![
                "-d",
                "Ubuntu",
                "/root/.local/bin/agent",
                "--force",
                "--trust",
                "--model",
                "fixture-model",
                "--fixture-flag",
            ],
        ),
    ] {
        let config = crate::pty::AgentConfig {
            cli: cli.into(),
            model: Some("fixture-model".into()),
            flags: vec!["--fixture-flag".into()],
            ..Default::default()
        };
        let (command, mut args) = crate::session::SessionController::build_command(&config);
        if cli == "claude" {
            assert!(uuid::Uuid::parse_str(&args[1]).is_ok());
            args[1] = "<fresh-session-id>".into();
        }
        assert_eq!(command, expected_command);
        assert_eq!(args, expected, "G11 recorded PTY argv for {cli}");
    }
    let temp = tempfile::tempdir().unwrap();
    let registry = std::sync::Arc::new(super::TransportRegistry::new(temp.path().into(), None));
    let manager = crate::pty::PtyManager::new();
    manager.attach_transports(registry.clone());
    assert!(
        registry.get_agent("fixture-worker").is_none(),
        "G11 no protocol process registered with option off"
    );
}

#[test]
fn launch_setting_omits_empty_and_roundtrips_per_session() {
    let legacy = serde_json::json!({"project_path":"fixture", "queen_config":{"cli":"codex","flags":[]},"workers":[]});
    let config: crate::session::HiveLaunchConfig = serde_json::from_value(legacy).unwrap();
    assert!(config.agent_transports.is_empty());
    assert!(serde_json::to_value(&config)
        .unwrap()
        .get("agent_transports")
        .is_none());
    let temp = tempfile::tempdir().unwrap();
    let storage = crate::storage::SessionStorage::new_with_base(temp.path().into()).unwrap();
    storage
        .save_agent_transports("fixture", &Default::default())
        .unwrap();
    assert!(!storage
        .session_dir("fixture")
        .join("agent-transports.json")
        .exists());
    assert!(storage.load_agent_transports("fixture").unwrap().is_empty());
    let enabled = [AgentTransportKind::CodexAppServer].into_iter().collect();
    storage.save_agent_transports("fixture", &enabled).unwrap();
    assert_eq!(storage.load_agent_transports("fixture").unwrap(), enabled);
    assert!(storage
        .load_agent_transports("other-fixture")
        .unwrap()
        .is_empty());
}

#[test]
fn option_off_and_ineligible_seats_stay_on_pty() {
    let worker = AgentRole::Worker {
        index: 1,
        parent: None,
    };
    for cli in ["claude", "codex", "cursor"] {
        assert_eq!(
            selected_transport(&AgentTransports::new(), cli, &worker),
            None
        );
    }
    let enabled = [
        AgentTransportKind::CodexAppServer,
        AgentTransportKind::ClaudeStreamJson,
    ]
    .into_iter()
    .collect();
    for role in [AgentRole::Queen, AgentRole::MasterPlanner] {
        for cli in ["codex", "claude"] {
            assert_eq!(selected_transport(&enabled, cli, &role), None);
        }
    }
    assert_eq!(
        selected_transport(&enabled, "codex", &worker),
        Some(AgentTransportKind::CodexAppServer)
    );
    assert_eq!(selected_transport(&enabled, "cursor", &worker), None);
}
