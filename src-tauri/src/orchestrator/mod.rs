//! Orchestrator module for vNext session coordination.
//!
//! This module will extract orchestration logic from controller.rs into
//! specialized submodules for better maintainability and testability.

pub mod planner;
pub mod session_orchestrator;
pub mod fusion;
pub mod resolver;
