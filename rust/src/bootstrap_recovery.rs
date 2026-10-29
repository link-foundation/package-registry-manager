//! Bootstrap owner-setup failures are prerequisites, separate from OIDC rejection.
use regex::Regex;
use std::collections::BTreeMap;

/// Successful npm CLI lines bind generated names to their actual release versions.
#[must_use]
pub fn published_versions(source: &str, names: &[String]) -> BTreeMap<String, String> {
    let pattern = Regex::new(
        r"(?:^|\s)\+\s+((?:@[a-z0-9._-]+/)?[a-z0-9][a-z0-9._-]*)@([0-9][0-9A-Za-z.+-]*)(?:\s|$)",
    )
    .expect("regex");
    source
        .lines()
        .flat_map(|line| pattern.captures_iter(line))
        .filter(|capture| names.iter().any(|name| name == &capture[1]))
        .map(|capture| (capture[1].into(), capture[2].into()))
        .collect()
}

/// Extract all exact missing names from the npm owner-setup diagnostic.
#[must_use]
pub fn bootstrap_names(log: &str) -> Vec<String> {
    let diagnostic = Regex::new(r"(?i)npm owner setup required for ([^:\n]+):").expect("regex");
    let valid = Regex::new(r"^(?:@[a-z0-9._-]+/)?[a-z0-9][a-z0-9._-]*$").expect("regex");
    let mut names = Vec::new();
    for capture in diagnostic.captures_iter(log) {
        for name in capture[1]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
        {
            if !valid.is_match(name) {
                return Vec::new();
            }
            if !names.iter().any(|s| s == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_bootstrap_from_unrelated_build_failures() {
        assert_eq!(
            bootstrap_names("npm owner setup required for root, @org/alias: first publish"),
            ["root", "@org/alias"]
        );
        assert_eq!(
            bootstrap_names("build failed"),
            [] as [std::string::String; 0]
        );
        assert_eq!(
            bootstrap_names("npm owner setup required for ../../bad: first publish"),
            [] as [std::string::String; 0]
        );
    }
    #[test]
    fn release_versions_are_bound_to_each_actual_name() {
        let versions = published_versions(
            "+ root@2.0.0\n+ @org/alias@2.0.1\n",
            &["root".into(), "@org/alias".into()],
        );
        assert_eq!(versions["root"], "2.0.0");
        assert_eq!(versions["@org/alias"], "2.0.1");
    }
}
