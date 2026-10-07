//! Additional OIDC registries and token-only publishing plans.
use crate::credential_cycle::credential_steps;
use crate::flows::FlowContext;
use crate::model::{Package, Registry, SetupStep, StepKind};
use crate::tokens::token_secret_steps;

#[must_use]
pub fn trusted_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
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
