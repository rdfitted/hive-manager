//! Planning-facing work-graph API.
//!
//! Plan parsing and validation live in `work_graph`; this module preserves the
//! orchestrator's planner seam while exposing the typed graph contract.

pub use super::work_graph::{
    topological_sort, BindingRef, CompositeExpansion, CycleError, EdgeKind, EdgeProvenance,
    NodeContract, NodeKind, NodeStatus, TaskGraph, TaskId, WorkEdge, WorkGraph,
    WorkGraphOmission, WorkGraphOmissionReason, WorkNode,
};
