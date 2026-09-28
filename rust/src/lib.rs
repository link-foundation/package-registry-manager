//! Repository discovery and package-registry setup planning.

pub mod auth_urls;
pub mod browser;
pub mod containers;
pub mod discovery;
pub mod flows;
pub mod model;
pub mod plan;
pub mod registry_state;
pub mod setup;
pub mod workflows;

pub use discovery::inspect_repository;
pub use model::{
    Inspection, Package, PlanMode, Registry, RepositoryInfo, SetupPlan, SetupStep, StepKind,
};
pub use plan::{build_plans, build_plans_for, build_plans_with, PlanOptions};
