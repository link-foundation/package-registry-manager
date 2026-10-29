//! Release authentication evidence is scoped to each executable publishing job.
use crate::{
    publishers::{
        child_block, child_indent, grants_id_token, job_publishes, only_called, parse_workflow,
        ParsedWorkflow,
    },
    workflows::Workflow,
    Registry,
};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}
/// Report missing OIDC or fallback, independently of last release provenance.
#[must_use]
pub fn publishing_policy(root: &Path, workflows: &[Workflow], registry: Registry) -> String {
    let parsed: BTreeMap<_, _> = workflows
        .iter()
        .map(|workflow| (workflow.name.clone(), parse_workflow(&workflow.contents)))
        .collect();
    let mut policies = BTreeSet::new();
    for workflow in workflows {
        if !only_called(&parsed[&workflow.name].header) {
            visit(
                root,
                &parsed,
                &workflow.name,
                registry,
                None,
                &mut BTreeSet::new(),
                &mut policies,
            );
        }
    }
    if policies.len() == 1 {
        policies.first().expect("one policy").to_string()
    } else if policies.is_empty() {
        "authentication-unverified".into()
    } else {
        "mixed-authentication".into()
    }
}
fn visit(
    root: &Path,
    parsed: &BTreeMap<String, ParsedWorkflow>,
    name: &str,
    registry: Registry,
    caller: Option<bool>,
    seen: &mut BTreeSet<String>,
    policies: &mut BTreeSet<&'static str>,
) {
    if !seen.insert(name.into()) {
        return;
    }
    let Some(workflow) = parsed.get(name) else {
        return;
    };
    let workflow_grant = grants_id_token(child_block(&workflow.header, 0, "permissions").as_ref());
    for job in &workflow.jobs {
        let own = grants_id_token(
            child_block(&job.lines, child_indent(&job.lines), "permissions").as_ref(),
        )
        .or(workflow_grant);
        let grant = caller.map_or_else(
            || own.unwrap_or(false),
            |caller| caller && own.unwrap_or(true),
        );
        let call = rx(r#"^\s*uses:\s*["']?\./\.github/workflows/([\w.-]+\.ya?ml)"#);
        if let Some(called) = job.lines.iter().find_map(|line| call.captures(line)) {
            visit(
                root,
                parsed,
                &called[1],
                registry,
                Some(grant),
                &mut seen.clone(),
                policies,
            );
        } else if job_publishes(root, workflow, job, registry) {
            let text = workflow
                .header
                .iter()
                .chain(&job.lines)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            policies.insert(policy(&text, registry, grant));
        }
    }
}
pub(crate) fn policy(text: &str, registry: Registry, grant: bool) -> &'static str {
    let token = rx(r"secrets\.(?:CARGO_(?:REGISTRY_)?TOKEN|NPM_(?:AUTH_)?TOKEN|NODE_AUTH_TOKEN|PYPI_(?:API_)?TOKEN)").is_match(text);
    let oidc = grant
        && match registry {
            Registry::CratesIo => rx(r"(?m)^[ \t]*- ").split(text).any(|step| {
                step.contains("crates-io-auth-action@")
                    && !rx(r"(?s)\bif:.*\bvars\.").is_match(step)
            }),
            Registry::PyPi => text.contains("pypa/gh-action-pypi-publish@"),
            _ => rx(r"npm publish|npm run|node .*(?:publish|release)").is_match(text),
        };
    let fallback = rx(r"continue-on-error:\s*true").is_match(text)
        && if registry == Registry::CratesIo {
            rx(r"outputs\.token\s*\|\|\s*secrets\.").is_match(text)
        } else {
            text.contains("outcome == 'failure'")
        };
    if oidc && token && fallback {
        "oidc-with-token-fallback"
    } else if oidc {
        "oidc-only"
    } else if token {
        "token-only"
    } else {
        "authentication-unverified"
    }
}
