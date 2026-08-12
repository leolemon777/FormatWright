//! Shared application-layer use cases for every first-party surface.
//!
//! Surfaces adapt protocols and presentation. Durable job scheduling, state
//! transitions, and resource admission live here so CLI, Desktop, and future
//! API/MCP workers do not fork the state machine.

pub mod job_execution;

pub use job_execution::{JobExecutionService, QueueRunReport, QueueWindowControl};
