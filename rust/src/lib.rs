//! Repository discovery and package-registry setup planning.

pub mod approvals;
pub mod auth_urls;
pub mod automation;
pub mod browser;
pub mod browser_catalogue;
pub mod browser_options;
pub mod containers;
pub mod crates_api;
pub mod default_browser;
pub mod discovery;
pub mod flows;
pub mod model;
pub mod npm_package;
pub mod pages;
pub mod plan;
pub mod prerequisites;
pub mod profile;
pub mod publishers;
pub mod registry_state;
pub mod setup;
pub mod sign_in_import;
pub mod skips;
pub mod tokens;
mod webdriver_automation;
pub mod workflows;

pub use discovery::{inspect_repository, inspect_repository_with, InspectOptions};
pub use model::{
    Inspection, Package, PlanMode, Registry, RepositoryInfo, SetupPlan, SetupStep, Skipped,
    StepKind,
};
pub use plan::{build_plans, build_plans_for, build_plans_with, PlanOptions};
