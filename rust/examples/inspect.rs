use std::path::Path;

use package_registry_manager::inspect_repository;

fn main() -> anyhow::Result<()> {
    let repository = std::env::args().nth(1).unwrap_or_else(|| ".".to_owned());
    let inspection = inspect_repository(Path::new(&repository))?;
    println!("{}", serde_json::to_string_pretty(&inspection)?);
    Ok(())
}
