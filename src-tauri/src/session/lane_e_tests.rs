use super::*;
use uuid::Uuid;

#[test]
fn g49_resume_commands_keep_saved_model_flags_and_conversation_identity() {
    let id = Uuid::new_v4().to_string();
    let claude = AgentConfig {
        cli: "claude".into(),
        model: Some("synthetic-model".into()),
        flags: vec![
            "--session-id".into(),
            Uuid::new_v4().to_string(),
            "--resume=old".into(),
            "--continue".into(),
            "--fork-session".into(),
            "--effort".into(),
            "high".into(),
        ],
        initial_prompt: Some("This original task must not run again".into()),
        ..AgentConfig::default()
    };
    let (command, args) = SessionController::build_resume_command(&claude, &id).unwrap();
    assert_eq!(command, "claude");
    assert_eq!(&args[..2], &["--resume", id.as_str()]);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--model", "synthetic-model"]));
    assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
    assert!(!args.iter().any(|arg| arg.contains("old")
        || ["--session-id", "--continue", "--fork-session"].contains(&arg.as_str())));
    assert!(!args.iter().any(|arg| arg.contains("original task")));
    assert_eq!(
        SessionController::transcript_key(&command, &args),
        Some(id.clone())
    );

    let codex = AgentConfig {
        cli: "codex".into(),
        model: Some("synthetic-codex".into()),
        flags: vec!["-c".into(), "model_reasoning_effort=high".into()],
        ..AgentConfig::default()
    };
    let (command, args) = SessionController::build_resume_command(&codex, &id).unwrap();
    assert_eq!(command, "codex");
    assert_eq!(&args[..2], &["resume", id.as_str()]);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-m", "synthetic-codex"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-c", "model_reasoning_effort=high"]));
    assert_eq!(
        SessionController::transcript_key(&command, &args),
        Some(format!("codex-resume-{id}"))
    );
    assert!(SessionController::build_resume_command(&codex, "invalid").is_err());
    assert!(SessionController::build_resume_command(
        &AgentConfig {
            cli: "cursor".into(),
            ..AgentConfig::default()
        },
        &id
    )
    .is_err());
}
