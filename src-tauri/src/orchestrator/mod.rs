//! Orchestrator module for vNext session coordination.
//!
//! This module will extract orchestration logic from controller.rs into
//! specialized submodules for better maintainability and testability.

pub mod fusion;
pub mod planner;
pub mod resolver;
pub mod session_orchestrator;
