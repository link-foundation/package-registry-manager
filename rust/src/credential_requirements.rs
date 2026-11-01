//! Registries without public OIDC publishing.
//!
//! The exact credential set a
//! workflow needs, which secrets it already references, and where values may
//! live. Values are only ever held in memory or written to encrypted GitHub
//! secrets; nothing here rotates, revokes or deletes an existing credential.
use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::model::Registry;
use crate::workflows::Workflow;

#[allow(clippy::struct_field_names)]
struct Role {
    role: &'static str,
    candidates: &'static [&'static str],
    default: &'static str,
    optional: bool,
}

struct CredentialSet {
    oidc_note: &'static str,
    namespace: Option<&'static str>,
    roles: &'static [Role],
}

const MAVEN: CredentialSet = CredentialSet {
    oidc_note: "Sonatype Central Portal has no OIDC trusted publishing; publish with a Central user-token pair and a GPG signing key.",
    namespace: Some("verified Central Portal namespace matching the groupId"),
    roles: &[
        Role {
            role: "central-username",
            candidates: &[
                "CENTRAL_USERNAME",
                "MAVEN_CENTRAL_USERNAME",
                "OSSRH_USERNAME",
                "SONATYPE_USERNAME",
                "MAVEN_USERNAME",
            ],
            default: "CENTRAL_USERNAME",
            optional: false,
        },
        Role {
            role: "central-token",
            candidates: &[
                "CENTRAL_TOKEN",
                "MAVEN_CENTRAL_TOKEN",
                "MAVEN_CENTRAL_PASSWORD",
                "OSSRH_TOKEN",
                "OSSRH_PASSWORD",
                "SONATYPE_TOKEN",
                "MAVEN_PASSWORD",
            ],
            default: "CENTRAL_TOKEN",
            optional: false,
        },
        Role {
            role: "gpg-private-key",
            candidates: &["GPG_PRIVATE_KEY", "MAVEN_GPG_PRIVATE_KEY", "SIGNING_KEY"],
            default: "GPG_PRIVATE_KEY",
            optional: false,
        },
        Role {
            role: "gpg-passphrase",
            candidates: &["GPG_PASSPHRASE", "MAVEN_GPG_PASSPHRASE", "SIGNING_PASSWORD"],
            default: "GPG_PASSPHRASE",
            optional: false,
        },
    ],
};

const PACKAGIST: CredentialSet = CredentialSet {
    oidc_note: "Public packagist.org has no OIDC publishing (Private Packagist OIDC is a separate product); releases come from tags via the GitHub hook or the update-package API.",
    namespace: None,
    roles: &[
        Role {
            role: "packagist-username",
            candidates: &["PACKAGIST_USERNAME"],
            default: "PACKAGIST_USERNAME",
            optional: true,
        },
        Role {
            role: "packagist-api-token",
            candidates: &["PACKAGIST_TOKEN", "PACKAGIST_API_TOKEN"],
            default: "PACKAGIST_TOKEN",
            optional: true,
        },
    ],
};

static SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsecrets\.([A-Za-z_][A-Za-z0-9_]*)\b").expect("static pattern"));
static COMMENT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*#.*$").expect("static pattern"));

/// Structured requirements resolved against the secrets the workflows read.
#[must_use]
pub fn credential_requirements(registry: Registry, workflows: &[Workflow]) -> Option<Value> {
    let set = match registry {
        Registry::MavenCentral => &MAVEN,
        Registry::Packagist => &PACKAGIST,
        _ => return None,
    };
    let referenced = workflows
        .iter()
        .flat_map(|workflow| {
            let text = COMMENT_LINE.replace_all(&workflow.contents, "");
            SECRET
                .captures_iter(&text)
                .map(|found| found[1].to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    let secrets = set
        .roles
        .iter()
        .map(|role| {
            let found = role
                .candidates
                .iter()
                .find(|name| referenced.contains(**name));
            let mut entry = json!({
                "role": role.role,
                "secret": found.copied().unwrap_or(role.default),
                "referenced": found.is_some(),
            });
            if role.optional {
                entry["optional"] = json!(true);
            }
            entry
        })
        .collect::<Vec<_>>();
    let mut result = json!({
        "oidc": "unsupported",
        "oidc_note": set.oidc_note,
    });
    if let Some(namespace) = set.namespace {
        result["namespace"] = json!(namespace);
    }
    result["storage"] = json!("memory-or-encrypted-github-secret");
    result["secrets"] = json!(secrets);
    Some(result)
}
