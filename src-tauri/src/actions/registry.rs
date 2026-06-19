//! The [`Action`] trait, the [`ActionRegistry`], and [`build_registry`] — the
//! single place every session + git action is registered.
//!
//! ## Concurrency invariant
//!
//! Action `run` bodies acquire the `parking_lot` `SessionController` guard
//! synchronously and MUST NOT hold it across an `.await`. The controller methods
//! wrapped here are all synchronous, so each `run` locks, calls, maps, and drops
//! the guard before returning — never awaiting under the lock.

use std::collections::HashMap;

use async_trait::async_trait;
use schemars::schema::RootSchema;
use serde_json::Value;

use super::context::ActionContext;
use super::error::ActionError;

/// A single unit of work, addressable by a stable dotted `name` (e.g.
/// `session.list`, `git.pull`), dispatched uniformly over `serde_json::Value`.
///
/// The trait is object-safe (`Box<dyn Action>`): `run` takes/returns
/// `serde_json::Value`, and each concrete action deserializes the input into its
/// typed DTO at the top of `run` and serializes its typed output. The output is
/// intentionally a plain `Value` so a future `{ renderer?, data }` result
/// envelope (#127) can wrap it without changing this contract.
#[async_trait]
pub trait Action: Send + Sync {
    /// Stable, unique identifier for this action.
    fn name(&self) -> &'static str;

    /// JSON Schema describing the action's accepted input, exported via
    /// `schemars` so the set is surfaceable as agent/MCP tools.
    fn input_schema(&self) -> RootSchema;

    /// Validate `input` WITHOUT running. The default implementation simply
    /// confirms the input is an object; actions override to deserialize into
    /// their DTO and run domain validation. The registry always calls this
    /// before [`Action::run`].
    fn validate_input(&self, input: &Value) -> Result<(), ActionError> {
        if input.is_object() || input.is_null() {
            Ok(())
        } else {
            Err(ActionError::bad_request(format!(
                "Action '{}' expects a JSON object as input",
                self.name()
            )))
        }
    }

    /// Execute the action. Validation has already run.
    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError>;
}

/// Holds every registered action and dispatches by name. Validation always runs
/// before execution (see [`ActionRegistry::dispatch`]).
#[derive(Default)]
pub struct ActionRegistry {
    actions: HashMap<&'static str, Box<dyn Action>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            actions: HashMap::new(),
        }
    }

    /// Register an action. Panics on a duplicate name so a registration bug
    /// surfaces at startup rather than silently shadowing.
    pub fn register(&mut self, action: Box<dyn Action>) {
        let name = action.name();
        if self.actions.insert(name, action).is_some() {
            panic!("Duplicate action registration for '{}'", name);
        }
    }

    /// List every registered action with its input schema (AC1). Sorted by name
    /// for deterministic output.
    pub fn list(&self) -> Vec<(&'static str, RootSchema)> {
        let mut entries: Vec<(&'static str, RootSchema)> = self
            .actions
            .values()
            .map(|action| (action.name(), action.input_schema()))
            .collect();
        entries.sort_by_key(|(name, _)| *name);
        entries
    }

    /// Whether an action with this name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.actions.contains_key(name)
    }

    /// Validate then run the named action. Returns `NotFound` if the name is
    /// unknown. Validation (AC3) always precedes `run`.
    pub async fn dispatch(
        &self,
        name: &str,
        ctx: &ActionContext,
        input: Value,
    ) -> Result<Value, ActionError> {
        let action = self
            .actions
            .get(name)
            .ok_or_else(|| ActionError::not_found(format!("Unknown action '{}'", name)))?;

        action.validate_input(&input)?;
        action.run(ctx, input).await
    }
}

/// The single registration point for all actions. Both the runtime (`lib.rs`)
/// and the tests build the registry through this function so the action set is
/// defined in exactly one place.
pub fn build_registry() -> ActionRegistry {
    let mut registry = ActionRegistry::new();
    super::session::register(&mut registry);
    super::git::register(&mut registry);
    super::pty::register(&mut registry);
    super::coordination::register(&mut registry);
    registry
}
