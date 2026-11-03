use super::*;

#[test]
fn g49_claude_resume_is_an_exclusive_pinned_identity() {
    let id = uuid::Uuid::new_v4().to_string();
    let parse = |args: &[&str]| {
        ClaudeOptions::from_pty(
            "claude",
            args,
            ".".into(),
            vec![],
            "session".into(),
            "worker".into(),
        )
    };
    let options = parse(&[
        "--resume",
        &id,
        "--dangerously-skip-permissions",
        "--model",
        "synthetic-model",
        "--effort",
        "high",
    ])
    .unwrap();
    assert_eq!(options.transcript_id, id);
    assert!(options.args.windows(2).any(|p| p == ["--resume", &id]));
    assert!(
        options.initial_prompt.is_none(),
        "G49 do not replay the initial task"
    );
    for args in [
        vec![
            "--resume",
            &id,
            "--session-id",
            &id,
            "--dangerously-skip-permissions",
        ],
        vec!["--resume", "invalid", "--dangerously-skip-permissions"],
        vec![
            "--resume",
            &id,
            "--continue",
            "--dangerously-skip-permissions",
        ],
    ] {
        assert!(parse(&args).is_err());
    }
}

#[cfg(windows)]
#[test]
fn g49_claude_fake_resume_recalls_pre_restart_content() {
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    fs::write(
        temp.path().join("memory.json"),
        json!({(id.clone()): "synthetic-sapphire"}).to_string(),
    )
    .unwrap();
    fs::write(temp.path().join("resume.ps1"), r#"
$ErrorActionPreference='Stop'
$index=[Array]::IndexOf($args,'--resume')
if($index -lt 0){exit 9}
$sid=$args[$index+1]
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'actual-cwd'), [Environment]::CurrentDirectory)
$memory=Get-Content -LiteralPath 'memory.json' -Raw | ConvertFrom-Json
$remembered=$memory.$sid
if(!$remembered){exit 10}
function Emit($v){[Console]::Out.WriteLine(($v|ConvertTo-Json -Compress -Depth 20))}
while($null -ne ($line=[Console]::ReadLine())){
 $m=$line|ConvertFrom-Json
 if($m.type -eq 'control_request'){
  Emit @{type='control_response';response=@{subtype='success';request_id=$m.request_id;response=@{account=@{subscriptionType='Claude Ultra'};claude_code_version='2.1.316'}}}
 }elseif($m.type -eq 'user'){
  Emit @{type='system';subtype='init';session_id=$sid;apiKeySource='none';permissionMode='bypassPermissions'}
  Emit @{type='user';session_id=$sid;uuid=$m.uuid;parent_tool_use_id=$null;isReplay=$true;message=$m.message}
  Emit @{type='stream_event';session_id=$sid;parent_tool_use_id=$null;event=@{type='message_start';message=@{id='remembered'}}}
  Emit @{type='assistant';session_id=$sid;parent_tool_use_id=$null;message=@{id='remembered';content=@(@{type='text';text=$remembered})}}
  Emit @{type='result';session_id=$sid;parent_tool_use_id=$null;user_message_uuids=@($m.uuid);subtype='success'}
 }
}
"#).unwrap();
    fs::write(temp.path().join("claude.cmd"), "@echo off\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0resume.ps1\" %*\r\n").unwrap();
    let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
    let options = ClaudeOptions::from_pty(
        temp.path().join("claude.cmd").to_str().unwrap(),
        &["--resume", &id, "--dangerously-skip-permissions"],
        temp.path().into(),
        vec![],
        "session".into(),
        "worker".into(),
    )
    .unwrap();
    let transport = ClaudeTransport::new(options, &registry);
    transport.start_blocking().unwrap();
    registry
        .register(
            "session",
            "worker",
            transport.thread_id(),
            transport.clone(),
        )
        .unwrap();
    transport.activate().unwrap();
    let mut events = transport.subscribe();
    assert_eq!(
        transport
            .send_input("Recall the saved word", true)
            .unwrap()
            .confirmed,
        Some(true)
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let remembered = loop {
        if let Ok(event) = events.try_recv() {
            if event.kind == AgentEventKind::AssistantMessage {
                break event.payload;
            }
        }
        assert!(
            Instant::now() < deadline,
            "G49 fake resumed conversation must answer"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    transport.stop_sync().unwrap();
    assert_eq!(
        std::path::Path::new(
            fs::read_to_string(temp.path().join("actual-cwd"))
                .unwrap()
                .trim()
        )
        .canonicalize()
        .unwrap(),
        temp.path().canonicalize().unwrap(),
        "G49 saved cwd reaches the process"
    );
    assert!(
        remembered.to_string().contains("synthetic-sapphire"),
        "G49 saved conversation memory returned"
    );
}
