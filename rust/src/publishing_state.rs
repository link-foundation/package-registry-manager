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
    crate::publishers::regex(pattern)
}
/// Report missing OIDC or fallback, independently of last release provenance.
#[must_use]
pub fn publishing_policy(root: &Path, workflows: &[Workflow], registry: Registry) -> String {
    publishing_policies(root, workflows, registry).0
}
/// The policy of every publishing job, with the authentication facts of the
/// jobs whose OIDC exchange a repository variable gates.
#[must_use]
pub fn publishing_policies(
    root: &Path,
    workflows: &[Workflow],
    registry: Registry,
) -> (String, Vec<AuthenticationFacts>) {
    let parsed: BTreeMap<_, _> = workflows
        .iter()
        .map(|workflow| (workflow.name.clone(), parse_workflow(&workflow.contents)))
        .collect();
    let mut policies = BTreeSet::new();
    let mut gates = Vec::new();
    for workflow in workflows {
        if !only_called(&parsed[&workflow.name].header) {
            visit(
                root,
                &parsed,
                &workflow.name,
                registry,
                None,
                &mut BTreeSet::new(),
                &mut (&mut policies, &mut gates),
            );
        }
    }
    let policy = if policies.len() == 1 {
        policies.first().expect("one policy").to_string()
    } else if policies.is_empty() {
        "authentication-unverified".into()
    } else {
        "mixed-authentication".into()
    };
    (policy, gates)
}
fn visit(
    root: &Path,
    parsed: &BTreeMap<String, ParsedWorkflow>,
    name: &str,
    registry: Registry,
    caller: Option<bool>,
    seen: &mut BTreeSet<String>,
    found: &mut (&mut BTreeSet<&'static str>, &mut Vec<AuthenticationFacts>),
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
                found,
            );
        } else if job_publishes(root, workflow, job, registry) {
            let text = workflow
                .header
                .iter()
                .chain(&job.lines)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let facts = authentication_facts(&text, registry);
            if facts.gate.is_some() {
                found.1.push(facts);
            }
            found.0.insert(policy(&text, registry, grant));
        }
    }
}
pub(crate) fn policy(text: &str, registry: Registry, grant: bool) -> &'static str {
    if registry == Registry::NuGet {
        let oidc = grant && text.contains("NuGet/login@");
        let fallback = rx(r"PRM_NUGET_FALLBACK:\s*\$\{\{\s*secrets\.[\w]+").is_match(text);
        return if oidc && fallback && rx(r"continue-on-error:\s*true").is_match(text) {
            "oidc-with-token-fallback"
        } else if oidc {
            "oidc-only"
        } else if rx(r"secrets\.[\w]+").is_match(text) {
            "token-only"
        } else {
            "authentication-unverified"
        };
    }
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
    let facts = authentication_facts(text, registry);
    if facts.password_always_token {
        // `vars.FLAG == 'true' && '' || secrets.TOKEN` always yields the token:
        // '' is falsy in GitHub expressions, so the flag never selects OIDC.
        return if token {
            "token-only"
        } else {
            "authentication-unverified"
        };
    }
    if grant && token && facts.gate.is_some() && facts.shell_fallback {
        return "variable-gated-oidc-with-token-fallback";
    }
    if oidc && token && (fallback || facts.npm_native) {
        "oidc-with-token-fallback"
    } else if oidc {
        "oidc-only"
    } else if token {
        "token-only"
    } else {
        "authentication-unverified"
    }
}

pub use crate::model::AuthenticationGate;
/// Actual credential-selection semantics of a publishing job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthenticationFacts {
    pub gate: Option<AuthenticationGate>,
    pub shell_fallback: bool,
    pub password_always_token: bool,
    pub npm_native: bool,
}
/// Actual credential-selection semantics of a publishing job, kept apart from
/// publisher registration and historical trusted uploads.
#[must_use]
pub fn authentication_facts(text: &str, registry: Registry) -> AuthenticationFacts {
    let mut facts = AuthenticationFacts::default();
    let steps: Vec<&str> = split_steps(text);
    let auth = steps.iter().find(|step| match registry {
        Registry::CratesIo => step.contains("crates-io-auth-action@"),
        Registry::NuGet => step.contains("NuGet/login@"),
        _ => false,
    });
    if let Some(gate) =
        auth.and_then(|step| rx(r#"\bif:.*\bvars\.(\w+)\s*==\s*['"]true['"]"#).captures(step))
    {
        facts.gate = Some(AuthenticationGate {
            variable: gate[1].to_string(),
            value: "true".into(),
            ineffective: false,
        });
    }
    facts.shell_fallback = rx(r"\$\{\w*OIDC\w*:-\$\w+\}").is_match(text);
    if registry == Registry::PyPi {
        let password = rx(r"(?m)^\s*password\s*:\s*(.+)$")
            .captures(text)
            .map_or(String::new(), |found| found[1].to_string());
        if let Some(flag) = rx(r#"vars\.(\w+)\s*==\s*['"]true['"]\s*&&\s*''\s*\|\|\s*secrets\.\w+"#)
            .captures(&password)
        {
            facts.password_always_token = true;
            facts.gate = Some(AuthenticationGate {
                variable: flag[1].to_string(),
                value: "true".into(),
                ineffective: true,
            });
        }
    }
    if registry == Registry::Npm {
        // npm 11.5.1+ tries the OIDC exchange itself and falls back to the
        // configured token, so a job with both needs no separate fallback step.
        facts.npm_native = steps.iter().any(|step| {
            rx(r"\bnpm\s+publish\b").is_match(step)
                && rx(r"NODE_AUTH_TOKEN\s*:\s*\$\{\{\s*secrets\.").is_match(step)
        });
    }
    facts
}
/// Split text before each line that starts a YAML sequence item, as the JS
/// lookahead split `/(?=^[ \t]*- )/m` does.
fn split_steps(text: &str) -> Vec<&str> {
    let starts: Vec<usize> = rx(r"(?m)^[ \t]*- ")
        .find_iter(text)
        .map(|found| found.start())
        .filter(|start| *start > 0)
        .collect();
    let mut parts = Vec::new();
    let mut from = 0;
    for start in starts {
        parts.push(&text[from..start]);
        from = start;
    }
    parts.push(&text[from..]);
    parts
}
