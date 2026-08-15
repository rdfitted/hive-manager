//! Durable role definitions and per-session organization semantics.

pub mod adjudication;
pub mod boundary;
pub mod composition;
pub mod definitions;
pub mod ownership;
pub mod schema;

pub use schema::{
    evaluator_role_definition, AuthorityScope, ContextBoundary, KnowledgeRef, KnowledgeSource,
    RoleDefinition, RoleLens, SignalClass,
};
