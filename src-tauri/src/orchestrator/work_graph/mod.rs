//! Typed plan and runtime work graphs for orchestration.

pub mod archetypes;
pub mod archive;
pub mod codegraph;
pub mod context;
pub mod divergence;
pub mod plan_parse;
pub mod retro;
pub mod review;
pub mod runtime;
pub mod schema;
pub mod toposort;
pub mod validate;

pub use schema::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, TaskId, WorkEdge, WorkGraph, WorkGraphOmission,
    WorkGraphOmissionReason, WorkNode,
};
pub use toposort::{topological_sort, CycleError};
