//! Additional OIDC registries and token-only publishing plans.
use crate::credential_cycle::credential_steps;
use crate::flows::FlowContext;
use crate::model::{Package, Registry, SetupStep, StepKind};
use crate::tokens::token_secret_steps;
use serde_json::{json, Value};

fn requirements(package: &Package, registry: Registry) -> Value {
    package.credential_requirements.clone().unwrap_or_else(|| {
        crate::credential_requirements::credential_requirements(registry, &[])
            .expect("registry has a credential set")
    })
}

fn pick(requirements: &Value, roles: &[&str]) -> Vec<Value> {
    requirements["secrets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| roles.iter().any(|role| entry["role"] == *role))
        .cloned()
        .collect()
}

fn describe(entries: &[Value]) -> String {
    entries
        .iter()
        .map(|entry| entry["secret"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Central publishing needs a provider-specific pair plus signing material; no
/// OIDC is claimed. Missing secrets are requested, present ones are kept, and
/// nothing is rotated or revoked.
#[must_use]
pub fn maven_credential_steps(package: &Package) -> Vec<SetupStep> {
    let requirements = requirements(package, Registry::MavenCentral);
    let pair = pick(&requirements, &["central-username", "central-token"]);
    let signing = pick(&requirements, &["gpg-private-key", "gpg-passphrase"]);
    vec![
        SetupStep::new(
            "ensure-central-credential-pair",
            &format!("Ensure the Central user-token pair {}", describe(&pair)),
            StepKind::Api,
            format!(
                "{} Generate a Central Portal user token (username + password pair) only when {} is missing, hold the values in memory, and store them as encrypted GitHub secrets through gh-manager. Existing secrets are kept; nothing is rotated or revoked.",
                requirements["oidc_note"].as_str().unwrap_or_default(),
                describe(&pair)
            ),
        )
        .confirmed()
        .detail("secrets", json!(pair))
        .detail("oidc", json!("unsupported"))
        .url("https://central.sonatype.com/usertoken"),
        SetupStep::new(
            "ensure-signing-key",
            &format!("Ensure the signing prerequisites {}", describe(&signing)),
            StepKind::Manual,
            "Maven Central rejects unsigned artifacts. Configure central.signing to reuse local private material or explicitly authorize repository-specific key generation. Verify detached signing and independent public-key readback, store the signing pair encrypted with its actual expiry, and remove temporary private material.",
        )
        .confirmed()
        .detail("secrets", json!(signing))
        .detail("state", json!("manual-prerequisite")),
    ]
}

/// Public Packagist: tags plus hook or update API; no OIDC, explicit states otherwise.
#[must_use]
pub fn packagist_steps(package: &Package) -> Vec<SetupStep> {
    let requirements = requirements(package, Registry::Packagist);
    let api = requirements["secrets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    vec![
        SetupStep::new("ensure-packagist-package", "Validate and register the root Composer package", StepKind::Api, "Validate composer.json, discover the exact registered repository, recover sign-in in the same browser profile and submit once only if absent. Never rename a published package."),
        SetupStep::new("packagist-update-api", "Encrypt Safe update credentials and verify acceptance", StepKind::Api, "Select the Safe API token in memory, encrypt the selected secrets through gh-manager, read presence back and independently require HTTP 202 update acceptance.").detail("secrets", json!(api)),
        SetupStep::new("packagist-github-hook", "Verify the Packagist push hook and test delivery", StepKind::Api, "Inspect/create/repair the Packagist hook through gh-manager; preserve unrelated hooks, read back active JSON push settings and inspect a fresh test delivery."),
        SetupStep::new("verify-packagist-package", "Verify exact Packagist publication and consumer", StepKind::Api, "Resume only affected failed jobs after configuration verification. Separately verify registration, queued update, exact indexed version/source SHA, Composer consumer and GitHub release."),
    ]
}

#[must_use]
pub fn trusted_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    if package.registry == Registry::NuGet {
        return nuget_flow(package, context);
    }
    let (program, args, url, description) = match package.registry {
        Registry::RubyGems => ("gem",vec!["build",package.manifest.rsplit('/').next().unwrap_or_default()],"https://rubygems.org/profile/oidc/pending_trusted_publishers".to_owned(),"Register the GitHub Actions identity as a pending publisher, or add it in the existing gem settings."),
        Registry::NuGet => ("dotnet",vec!["pack","--configuration","Release"],"https://www.nuget.org/account/TrustedPublishing".to_owned(),"Register a trusted publishing policy for this GitHub Actions identity. Use NuGet/login to exchange OIDC for a short-lived API key; never store that key."),
        _ => ("deno",vec!["publish","--dry-run"],format!("https://jsr.io/{}/settings",package.name),"Link this package to the GitHub repository; publish from the configured workflow through OIDC."),
    };
    let mut steps = vec![
        SetupStep::new("validate-package",&format!("Validate {} package",package.registry),StepKind::Check,"Build or validate locally without uploading.").command(program,&args),
        SetupStep::new("configure-trusted-publisher",&format!("Configure {} trusted publishing",package.registry),StepKind::Browser,description).url(url),
        SetupStep::new("verify-oidc-release","Verify an OIDC release before removing tokens",StepKind::Manual,format!("Run {} and verify registry acceptance through OIDC; remove secret references from every workflow before deleting unused credentials.",context.workflow.as_deref().unwrap_or_default())),
    ];
    if let Some(slug) = &context.slug {
        steps.extend(token_secret_steps(package, slug));
    }
    steps
}

#[must_use]
pub fn token_flow(package: &Package) -> Vec<SetupStep> {
    let command = match package.registry {
        Registry::MavenCentral => Some(("mvn", vec!["--batch-mode", "verify"])),
        Registry::VsCodeMarketplace | Registry::OpenVsx => {
            Some(("npx", vec!["--no-install", "vsce", "package"]))
        }
        _ => None,
    };
    let gradle = package.registry == Registry::MavenCentral && package.audit.contains_key("gradle");
    let mut steps = if gradle {
        crate::gradle_distribution::gradle_validation_steps(package)
    } else {
        Vec::new()
    };
    if let Some((program, args)) = command.filter(|_| !gradle) {
        steps.push(
            SetupStep::new(
                "validate-package",
                "Validate package before credential changes",
                StepKind::Check,
                "Build without publishing.",
            )
            .command(program, &args),
        );
    }
    if package.registry == Registry::MavenCentral {
        let group = package
            .name
            .split_once(':')
            .map(|(group, _)| group)
            .filter(|group| {
                !group.is_empty()
                    && group
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            });
        let mut step = SetupStep::new(
            "verify-namespace",
            &format!(
                "Verify the Central namespace{}",
                group.map_or(String::new(), |group| format!(" {group}"))
            ),
            StepKind::Browser,
            "Sign in to the Central Portal and verify the namespace used by the package coordinates. Publishing stays blocked until the namespace reads back as verified.",
        )
        .url("https://central.sonatype.com/publishing/namespaces");
        if let Some(group) = group {
            step = step.detail("namespace", json!(group));
        }
        steps.push(step.detail("state", json!("manual-prerequisite")));
        steps.extend(maven_credential_steps(package));
        steps.push(crate::gradle_distribution::central_publication_step(
            package,
        ));
        return steps;
    }
    steps.extend(credential_steps(
        package.registry,
        package
            .token_secrets
            .first()
            .map_or("registry token", String::as_str),
    ));
    steps
}

/// Static distribution metadata does not prove a publication destination.
#[must_use]
pub fn cpp_flow(package: &Package) -> Vec<SetupStep> {
    let (program, args) = match package.registry {
        Registry::Conan => ("conan", vec!["inspect", ".", "--format=json"]),
        Registry::Vcpkg => ("vcpkg", vec!["install", "--dry-run", "--x-manifest-root=."]),
        _ => ("cmake", vec!["-S", ".", "-B", "build/prm-validation"]),
    };
    let mut steps = vec![
        SetupStep::new(
            "validate-package",
            "Validate distribution metadata",
            StepKind::Check,
            "Validate locally without uploading.",
        )
        .command(program, &args),
        SetupStep::new(
            "review-distribution-channel",
            "Resolve distribution provider and CI configuration",
            StepKind::Manual,
            if package.registry == Registry::Conan {
                "Read CONAN_REMOTE_URL and credential health for CONAN_LOGIN_USERNAME/CONAN_PASSWORD through gh-manager at effective repository/environment scope. Choose an explicit remote, validate conan create and consumer tests in reviewed CI, and retain existing credentials. A local recipe establishes neither public presence nor permissions."
            } else {
                "Review the CMake install/export, package config and CPack release workflow or vcpkg registry repository, baseline, port and version database. Validate installation and consumer tests in CI. Provider lookup/setup is unsupported until a destination is explicitly configured; publication remains unverified."
            },
        ),
    ];
    if package.registry == Registry::Conan {
        steps.insert(1, SetupStep::new("audit-distribution-channel", "Read Conan remote and credential health", StepKind::Api, "Read effective remote configuration and credential health through gh-manager. Unknown credentials and missing remotes remain explicit; do not upload or rotate credentials."));
    }
    steps
}

fn nuget_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let project = package.manifest.rsplit('/').next().unwrap_or_default();
    let mut steps = vec![
        SetupStep::new("validate-package", "Validate NuGet package", StepKind::Check, "Build and test the selected project before configuration; do not upload.").command("dotnet", &["test", project, "--configuration", "Release"]),
        SetupStep::new("configure-nuget-policy", "Verify exact NuGet trusted publishing policy", StepKind::Api, "Read explicit nuget.username, package_owner, package_ids and operations from .package-registry-manager.json; resolve immutable GitHub IDs, verify the authenticated profile, discover before creating a narrow policy, read activation back and ensure effective NUGET_USER through gh-manager. Retain existing fallback secrets."),
        SetupStep::new("verify-nuget-release", "Verify NuGet CI and exact public consumer", StepKind::Api, format!("Verify {}: successful OIDC exchange and actual upload are separate gates. Retain existing token fallbacks. Require the repaired current-main revision, every required build/test/documentation/mirror/release stage, exact public package download and consumer restore/build. A skipped stage or placeholder remains incomplete.", context.workflow.as_deref().unwrap_or_default())),
    ];
    if let Some(slug) = &context.slug {
        steps.extend(token_secret_steps(package, slug));
    }
    steps
}
