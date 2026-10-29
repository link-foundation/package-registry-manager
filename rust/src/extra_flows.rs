//! Additional OIDC registries and token-only publishing plans.
use crate::credential_cycle::credential_steps;
use crate::flows::FlowContext;
use crate::model::{Package, Registry, SetupStep, StepKind};
use crate::tokens::token_secret_steps;

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
    let mut steps = Vec::new();
    if let Some((program, args)) = command {
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
        steps.push(SetupStep::new("verify-namespace","Verify a Central namespace",StepKind::Browser,"Sign in to the Central Portal and verify the namespace used by the package coordinates.").url("https://central.sonatype.com/publishing/namespaces"));
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
