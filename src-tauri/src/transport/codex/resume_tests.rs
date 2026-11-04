use super::*;

#[test]
fn g49_codex_resume_preserves_identity_model_and_effort_without_replaying_task() {
    let id = uuid::Uuid::new_v4().to_string();
    let launch = CodexOptions::from_pty(
        "codex",
        &[
            "resume",
            &id,
            "--dangerously-bypass-approvals-and-sandbox",
            "-m",
            "synthetic-model",
            "-c",
            "model_reasoning_effort=high",
        ],
        ".".into(),
        vec![],
        "session".into(),
        "worker".into(),
    )
    .unwrap();
    assert_eq!(launch.resume_thread_id.as_deref(), Some(id.as_str()));
    assert_eq!(launch.model.as_deref(), Some("synthetic-model"));
    assert_eq!(launch.effort.as_deref(), Some("high"));
    assert!(launch.initial_prompt.is_none());
    assert!(!launch
        .server_args
        .iter()
        .any(|arg| arg == "resume" || arg == &id));
    for args in [
        vec!["resume"],
        vec!["resume", "--last"],
        vec!["resume", "invalid"],
    ] {
        assert!(CodexOptions::from_pty(
            "codex",
            &args,
            ".".into(),
            vec![],
            "session".into(),
            "worker".into()
        )
        .is_err());
    }
}

#[test]
fn g49_codex_resume_contract_keeps_pinned_schema_and_required_thread() {
    let contract: Value = serde_json::from_str(include_str!("schema-contract.json")).unwrap();
    assert_eq!(contract["schema_sha256"], client::PINNED_SCHEMA_SHA256);
    let resume = &contract["surface"]["v2/ThreadResumeParams.json"];
    assert_eq!(resume["required"], json!(["threadId"]));
    for option in ["threadId", "cwd", "model", "approvalPolicy", "sandbox"] {
        assert!(
            resume["properties"][option].is_object(),
            "G49 resume schema preserves {option}"
        );
    }
}

#[cfg(windows)]
#[test]
fn g49_codex_fake_resume_recalls_saved_content_and_preserves_billing_refusal() {
    use std::fs;
    for provider in ["openai", "synthetic-custom", "wrong-thread"] {
        let temp = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(
            temp.path().join("memory.json"),
            json!({(id.clone()): "synthetic-amber"}).to_string(),
        )
        .unwrap();
        fs::write(temp.path().join("resume.ps1"), r#"
$ErrorActionPreference='Stop'
$memoryByThread=Get-Content -LiteralPath 'memory.json' -Raw | ConvertFrom-Json
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'actual-cwd'),[Environment]::CurrentDirectory)
function Reply($m,$r){[Console]::Out.WriteLine((@{id=$m.id;result=$r}|ConvertTo-Json -Compress -Depth 20))}
function Notify($method,$params){[Console]::Out.WriteLine((@{method=$method;params=$params}|ConvertTo-Json -Compress -Depth 20))}
while($null -ne ($line=[Console]::ReadLine())){
 $m=$line|ConvertFrom-Json
 [IO.File]::AppendAllText((Join-Path $PSScriptRoot 'requests.jsonl'),$line+"`n")
 switch($m.method){
  'initialize' {Reply $m @{userAgent='codex-cli/0.166.0'}}
  'account/read' {Reply $m @{account=@{type='chatgpt'};requiresOpenaiAuth=$true}}
  'thread/start' {Reply $m @{thread=@{id='foreign-thread'};modelProvider='openai'}}
  'thread/resume' {
   $heldThread=$m.params.threadId
   $savedWord=$memoryByThread.$heldThread
   if(!$savedWord){exit 10}
   $responseThread=$heldThread
   $responseProvider=$env:SYNTHETIC_PROVIDER
   if($responseProvider -eq 'wrong-thread'){$responseThread='foreign-thread';$responseProvider='openai'}
   Reply $m @{thread=@{id=$responseThread};modelProvider=$responseProvider}
  }
  'turn/start' {
   Notify 'turn/started' @{threadId=$heldThread;turn=@{id='saved-turn';status='inProgress'}}
   Reply $m @{turn=@{id='saved-turn';status='inProgress'}}
   Notify 'item/completed' @{threadId=$heldThread;turnId='saved-turn';item=@{type='agentMessage';id='saved-answer';text=$savedWord}}
   Notify 'turn/completed' @{threadId=$heldThread;turn=@{id='saved-turn';status='completed'}}
  }
 }
}
"#).unwrap();
        let mut launch = CodexOptions::from_pty(
            "codex",
            &["resume", &id, "-m", "synthetic-model"],
            temp.path().into(),
            vec![("SYNTHETIC_PROVIDER".into(), provider.into())],
            "session".into(),
            "worker".into(),
        )
        .unwrap();
        // Use a fake server directly; never resolve or start the installed CLI.
        launch.options.command = "powershell.exe".into();
        launch.options.server_args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            temp.path()
                .join("resume.ps1")
                .to_string_lossy()
                .into_owned(),
        ];
        let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
        let transport = CodexTransport::new(launch, &registry);
        let result = transport.start_blocking();
        if provider != "openai" {
            let expected = if provider == "wrong-thread" {
                "different thread ID"
            } else {
                "billing"
            };
            assert!(
                result.unwrap_err().contains(expected),
                "G49 resume refuses foreign identity or provider billing"
            );
            assert!(!transport.is_alive());
            continue;
        }
        result.unwrap();
        assert_eq!(transport.thread_id().as_deref(), Some(id.as_str()));
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
                "G49 saved conversation must answer"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        transport.stop_sync().unwrap();
        assert!(remembered.to_string().contains("synthetic-amber"));
        let requests: Vec<Value> = fs::read_to_string(temp.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!requests
            .iter()
            .any(|request| request["method"] == "thread/start"));
        let resume = requests
            .iter()
            .find(|request| request["method"] == "thread/resume")
            .unwrap();
        assert_eq!(resume["params"]["threadId"], id);
        assert_eq!(resume["params"]["cwd"], json!(temp.path()));
        assert_eq!(resume["params"]["model"], "synthetic-model");
        assert_eq!(resume["params"]["approvalPolicy"], "never");
        assert_eq!(resume["params"]["sandbox"], "danger-full-access");
        assert_eq!(
            std::path::Path::new(
                fs::read_to_string(temp.path().join("actual-cwd"))
                    .unwrap()
                    .trim()
            )
            .canonicalize()
            .unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }
}
