//! Print the offline inspection of a repository as JSON, including skipped
//! manifests: `cargo run --example inspect -- <repository>`.
use std::path::PathBuf;

use package_registry_manager::{inspect_repository_with, InspectOptions};

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".to_owned()));
    let inspection = inspect_repository_with(
        &root,
        InspectOptions {
            include_skipped: true,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&inspection)?);
    Ok(())
}
