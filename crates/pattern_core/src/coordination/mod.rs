//! Agent coordination and group management
//!
//! This module provides the infrastructure for coordinating multiple agents
//! through various patterns like supervisor, round-robin, voting, etc.

pub mod groups;
pub mod patterns;
pub mod selectors;
pub mod types;
pub mod utils;

#[cfg(test)]
pub mod test_utils;

// Re-export main types
pub use groups::{AgentGroup, Constellation, GroupManager, GroupResponse};
pub use patterns::{
    DynamicManager, PipelineManager, RoundRobinManager, SleeptimeManager, SupervisorManager,
    VotingManager,
};
pub use selectors::{AgentSelector, CapabilitySelector, LoadBalancingSelector, RandomSelector};
pub use types::*;
