#!/usr/bin/env rust-script
//! Keep the colocated npm package on the same release version as the crate.
//!
//! ```cargo
//! [dependencies]
//! regex = "1"
//! ```

use regex::Regex;
use std::{env, fs, path::Path, process::exit};

fn update_versions(path: &Path, version: &str, expected: usize) -> Result<(), String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let version_line = Regex::new(r#"(?m)^(\s*\"version\"\s*:\s*\")[^\"]+(\",?)$"#)
        .map_err(|error| format!("Failed to build version matcher: {error}"))?;
    let found = version_line.find_iter(&content).count();
    if found < expected {
        return Err(format!(
            "{} contains {found} version fields; expected at least {expected}",
            path.display()
        ));
    }
    let replacement = format!("${{1}}{version}${{2}}");
    let updated = version_line.replacen(&content, expected, replacement.as_str());
    fs::write(path, updated.as_bytes())
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))?;
    println!("Updated {} to version {version}", path.display());
    Ok(())
}

fn sync(root: &Path, version: &str) -> Result<(), String> {
    update_versions(&root.join("package.json"), version, 1)?;
    update_versions(&root.join("package-lock.json"), version, 2)
}

fn main() {
    let version = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: rust-script sync-js-version.rs <version>");
        exit(2);
    });
    if let Err(error) = sync(Path::new("js"), &version) {
        eprintln!("Error: {error}");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_only_the_package_versions_in_a_lockfile() {
        let root = env::temp_dir().join(format!("sync-js-version-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"version\": \"1.0.0\",\n  \"name\": \"demo\"\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("package-lock.json"),
            "{\n  \"version\": \"1.0.0\",\n  \"packages\": {\n    \"\": {\n      \"version\": \"1.0.0\"\n    },\n    \"node_modules/x\": {\n      \"version\": \"9.0.0\"\n    }\n  }\n}\n",
        )
        .unwrap();

        sync(&root, "1.1.0").unwrap();
        let lock = fs::read_to_string(root.join("package-lock.json")).unwrap();
        assert_eq!(lock.matches("\"version\": \"1.1.0\"").count(), 2);
        assert!(lock.contains("\"version\": \"9.0.0\""));
        let _ = fs::remove_dir_all(root);
    }
}
