//! Repository discovery and package-registry setup planning.

pub mod approvals;
pub mod auth_urls;
pub mod automation;
pub mod bootstrap_reference;
pub mod browser;
pub mod browser_catalogue;
pub mod browser_options;
pub mod ci_credential_cycle;
pub mod containers;
pub mod crates_api;
pub mod credential_browser;
pub mod credential_cycle;
pub mod default_browser;
pub mod discovery;
pub mod extra_flows;
pub mod flows;
pub mod github;
pub mod manifest_proposal;
pub mod model;
pub mod npm_package;
pub mod npm_policy;
pub mod organization_scan;
pub mod package_coverage;
pub mod pages;
pub mod plan;
pub mod prerequisites;
pub mod profile;
pub mod publishers;
pub mod python;
pub mod registry_state;
pub mod repository_identity;
pub mod repository_repair;
pub mod setup;
pub mod sign_in_import;
pub mod skips;
pub mod source_code;
pub mod tokens;
pub mod version_guard;
mod webdriver_automation;
pub mod workflow_proposal;
pub mod workflows;

pub use discovery::{inspect_repository, inspect_repository_with, InspectOptions};
pub use model::{
    Inspection, Package, PlanMode, Registry, RepositoryInfo, SetupPlan, SetupStep, Skipped,
    StepKind,
};
pub use plan::{build_plans, build_plans_for, build_plans_with, PlanOptions};
