//! Unit tests for the action registry: listing + schemas (AC1), validate-before-run
//! (AC3), and caller visibility inside `run` (AC4).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use schemars::schema::RootSchema;
use serde_json::{json, Value};

use super::context::{ActionContext, Caller};
use super::error::{ActionError, ActionStatus};
use super::pty::resolve_create_role_for_test;
use super::registry::{build_registry, Action, ActionRegistry};
use crate::coordination::InjectionManager;
use crate::events::EventBus;
use crate::http::state::AppState;
use crate::pty::PtyManager;
use crate::session::SessionController;
use crate::storage::SessionStorage;

/// Build a hermetic `Arc<AppState>` backed by a temp storage dir.
fn test_state() -> Arc<AppState> {
    let dir = tempfile::TempDir::new().unwrap();
    let storage = Arc::new(SessionStorage::new_with_base(dir.path().to_path_buf()).unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    session_controller.write().set_storage(storage.clone());
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new_with_base(dir.path().to_path_buf()).unwrap(),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let app_state_db = Arc::new(crate::storage::ApplicationStateDb::open_in_memory().unwrap());
    let queue_repo = Arc::new(crate::storage::QueueRepo::new(app_state_db.clone()));
    queue_repo.ensure_schema().unwrap();
    let queue_manager = Arc::new(crate::coordination::QueueManager::new(
        queue_repo,
        event_bus.clone(),
    ));
    // Keep the TempDir alive for the lifetime of the process under test by leaking it;
    // tests are short-lived and this avoids a premature directory cleanup race.
    std::mem::forget(dir);
    Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller,
        injection_manager,
        storage,
        event_bus,
        app_state_db,
        queue_manager,
        None,
    ))
}

/// Tiny probe action that echoes the caller back so a test can assert the
/// `Caller` threaded through `run`.
struct CallerProbe;

#[async_trait]
impl Action for CallerProbe {
    fn name(&self) -> &'static str {
        "test.caller_probe"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(Value)
    }

    async fn run(&self, ctx: &ActionContext, _input: Value) -> Result<Value, ActionError> {
        Ok(serde_json::to_value(ctx.caller).unwrap())
    }
}

#[test]
fn test_registry_lists_all_actions() {
    let registry = build_registry();
    let names: Vec<&'static str> = registry.list().into_iter().map(|(name, _)| name).collect();

    // Session actions (AC2-required set).
    for expected in [
        "session.list",
        "session.get",
        "session.stop",
        "session.close",
        "session.launch_hive_v2",
        "session.launch_debate",
        "session.update_metadata",
    ] {
        assert!(
            names.contains(&expected),
            "missing session action {expected}"
        );
    }

    // Git actions (AC2-required set).
    for expected in [
        "git.list_branches",
        "git.current_branch",
        "git.switch_branch",
        "git.pull",
        "git.push",
        "git.fetch",
        "git.worktree_add",
        "git.worktree_list",
        "git.worktree_remove",
        "git.worktree_prune",
    ] {
        assert!(names.contains(&expected), "missing git action {expected}");
    }

    // Scratch terminals deliberately reuse the existing PTY action surface so they do
    // not require another Tauri command or ACL permission.
    for expected in ["pty.create", "pty.kill", "pty.list"] {
        assert!(names.contains(&expected), "missing PTY action {expected}");
    }
}

#[test]
fn test_pty_create_schema_exposes_scratch_role_and_ownership_metadata() {
    let registry = build_registry();
    let (_, schema) = registry
        .list()
        .into_iter()
        .find(|(name, _)| *name == "pty.create")
        .expect("pty.create action should be registered");
    let serialized = serde_json::to_string(&schema).expect("pty.create schema should serialize");

    for expected in ["role", "scratch_shell", "shell", "session_id"] {
        assert!(
            serialized.contains(expected),
            "pty.create schema should contain {expected}: {serialized}"
        );
    }
}

#[test]
fn test_pty_create_role_defaults_to_worker_and_resolves_scratch_shell() {
    let (default_role, default_owner) = resolve_create_role_for_test(json!({
        "id": "existing-agent",
        "command": "codex",
        "args": [],
        "cwd": null,
        "cols": 120,
        "rows": 30
    }))
    .expect("legacy PTY input should retain the worker default");
    assert!(matches!(
        default_role,
        crate::pty::AgentRole::Worker {
            index: 0,
            parent: None
        }
    ));
    assert_eq!(default_owner, None);

    let (scratch_role, scratch_owner) = resolve_create_role_for_test(json!({
        "id": "scratch:session-a:test",
        "command": "cmd.exe",
        "args": [],
        "cwd": ".",
        "cols": 120,
        "rows": 30,
        "role": "scratch_shell",
        "shell": "cmd",
        "session_id": "session-a"
    }))
    .expect("scratch metadata should resolve to the neutral role");
    assert!(matches!(scratch_role, crate::pty::AgentRole::ScratchShell));
    assert_eq!(scratch_owner.as_deref(), Some("session-a"));

    let ambiguous_id = resolve_create_role_for_test(json!({
        "id": "scratch:session-a:part:two",
        "command": "cmd.exe",
        "args": [],
        "cwd": ".",
        "cols": 120,
        "rows": 30,
        "role": "scratch_shell",
        "shell": "cmd",
        "session_id": "session-a"
    }))
    .expect_err("scratch unique ids containing colons must be rejected");
    assert_eq!(ambiguous_id.status, ActionStatus::BadRequest);
}

#[tokio::test]
async fn test_pty_kill_unregisters_scratch_ownership_without_spawning() {
    let registry = build_registry();
    let state = test_state();
    let session_id = "session-a";
    let pty_id = "scratch:session-a:test";
    state
        .session_controller
        .read()
        .insert_scratch_pty_ownership_for_test(session_id, pty_id);

    registry
        .dispatch(
            "pty.kill",
            &ActionContext::new(Caller::Frontend, state.clone()),
            json!({ "id": pty_id }),
        )
        .await
        .expect("killing an already-ended scratch PTY should still clear ownership");

    assert!(
        !state
            .session_controller
            .read()
            .owns_scratch_pty_for_test(session_id, pty_id),
        "pty.kill must unregister scratch ownership"
    );
}

#[tokio::test]
async fn test_scratch_pty_rejects_unknown_session_before_process_spawn() {
    let registry = build_registry();
    let ctx = ActionContext::new(Caller::Frontend, test_state());
    let result = registry
        .dispatch(
            "pty.create",
            &ctx,
            json!({
                "id": "scratch:missing-session:test",
                "command": "powershell.exe",
                "args": ["-NoLogo"],
                "cwd": ".",
                "cols": 120,
                "rows": 30,
                "role": "scratch_shell",
                "shell": "powershell",
                "session_id": "missing-session"
            }),
        )
        .await;

    let err = result.expect_err("unknown scratch owner should be rejected before spawning");
    assert_eq!(err.status, ActionStatus::BadRequest);
    assert!(
        err.message.contains("Session missing-session not found"),
        "unexpected message: {}",
        err.message
    );
}

#[test]
fn test_schema_per_action_serializes() {
    let registry = build_registry();
    for (name, schema) in registry.list() {
        let value = serde_json::to_value(&schema)
            .unwrap_or_else(|e| panic!("schema for {name} failed to serialize: {e}"));
        assert!(
            value.is_object(),
            "schema for {name} should serialize to a JSON object"
        );
    }
}

#[tokio::test]
async fn test_validation_runs_before_run_bad_color() {
    // `session.update_metadata` with an invalid color is a pure-validation path:
    // it must be rejected with BadRequest WITHOUT touching the controller.
    let registry = build_registry();
    let ctx = ActionContext::new(Caller::Http, test_state());

    let result = registry
        .dispatch(
            "session.update_metadata",
            &ctx,
            json!({ "id": "sess-1", "color": "not-a-color" }),
        )
        .await;

    let err = result.expect_err("invalid color should be rejected");
    assert_eq!(err.status, ActionStatus::BadRequest);
    assert!(
        err.message.contains("Invalid session color"),
        "unexpected message: {}",
        err.message
    );
}

#[tokio::test]
async fn test_validation_runs_before_run_bad_cli() {
    // `session.launch_hive_v2` with an invalid queen CLI must fail validation
    // (BadRequest) before the controller is ever invoked.
    let registry = build_registry();
    let ctx = ActionContext::new(Caller::Http, test_state());

    let input = json!({
        "project_path": ".",
        "queen_config": { "cli": "definitely-not-a-cli", "model": null, "flags": [] },
        "workers": [],
        "prompt": null
    });

    let result = registry
        .dispatch("session.launch_hive_v2", &ctx, input)
        .await;

    let err = result.expect_err("invalid CLI should be rejected");
    assert_eq!(err.status, ActionStatus::BadRequest);
    assert!(
        err.message.contains("Invalid CLI"),
        "unexpected message: {}",
        err.message
    );
}

#[tokio::test]
async fn test_unknown_action_is_not_found() {
    let registry = build_registry();
    let ctx = ActionContext::new(Caller::Http, test_state());
    let err = registry
        .dispatch("nope.does_not_exist", &ctx, json!({}))
        .await
        .expect_err("unknown action should error");
    assert_eq!(err.status, ActionStatus::NotFound);
}

#[tokio::test]
async fn test_caller_visible_in_run() {
    let mut registry = ActionRegistry::new();
    registry.register(Box::new(CallerProbe));
    let state = test_state();

    for caller in [Caller::Frontend, Caller::Http, Caller::Agent, Caller::Cli] {
        let ctx = ActionContext::new(caller, state.clone());
        let echoed = registry
            .dispatch("test.caller_probe", &ctx, json!({}))
            .await
            .expect("probe should run");
        let expected = serde_json::to_value(caller).unwrap();
        assert_eq!(echoed, expected, "caller mismatch for {caller:?}");
    }
}

#[tokio::test]
async fn test_session_list_dispatch_returns_array() {
    let registry = build_registry();
    let ctx = ActionContext::new(Caller::Frontend, test_state());
    let result = registry
        .dispatch("session.list", &ctx, json!({}))
        .await
        .expect("session.list should run");
    assert!(result.is_array(), "session.list should return a JSON array");
}
