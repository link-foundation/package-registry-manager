//! Repository discovery and package-registry setup planning.

pub mod browser;
pub mod discovery;
pub mod model;
pub mod plan;
pub mod setup;

pub use discovery::inspect_repository;
pub use model::{Inspection, Package, Registry, RepositoryInfo, SetupPlan, SetupStep};
pub use plan::{build_plans, build_plans_for};
