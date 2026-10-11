//! Default-branch release filtering and failed publishing-job recognition.

use regex::Regex;
use serde_json::Value;

/// Fields needed to select, report and safely retry a release run.
pub const RELEASE_RUN_FIELDS: &str =
    "databaseId,conclusion,status,url,headSha,headBranch,event,createdAt";

/// Filter out PRs and other branches, deduplicate events, and sort newest first.
#[must_use]
pub fn eligible_release_runs(runs: Vec<Value>, branch: &str) -> Vec<Value> {
    let mut unique = std::collections::BTreeMap::new();
    for run in runs {
        if run["headBranch"] == branch
            && matches!(run["event"].as_str(), Some("push" | "workflow_dispatch"))
        {
            unique.insert(run["databaseId"].to_string(), run);
        }
    }
    let mut runs: Vec<_> = unique.into_values().collect();
    runs.sort_by(|left, right| {
        right["createdAt"]
            .as_str()
            .cmp(&left["createdAt"].as_str())
            .then_with(|| {
                right["databaseId"]
                    .as_u64()
                    .cmp(&left["databaseId"].as_u64())
            })
    });
    runs
}

/// Only failed publish/preflight jobs or failed publishing steps qualify.
#[must_use]
pub fn failed_publishing_job(jobs: &Value) -> bool {
    let name = Regex::new(r"(?i)(?:^|[^a-z])(?:publish(?:ing)?|preflight)(?:[^a-z]|$)")
        .expect("valid publishing job pattern");
    jobs.as_array().is_some_and(|jobs| {
        jobs.iter().any(|job| {
            job["conclusion"] == "failure"
                && (name.is_match(job["name"].as_str().unwrap_or_default())
                    || job["steps"].as_array().is_some_and(|steps| {
                        steps.iter().any(|step| {
                            step["conclusion"] == "failure"
                                && name.is_match(step["name"].as_str().unwrap_or_default())
                        })
                    }))
        })
    })
}
