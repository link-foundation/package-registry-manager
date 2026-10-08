//! Ordered repair steps for transferred repositories.

use crate::flows::FlowContext;
use crate::model::{Package, SetupStep, StepKind};

/// Put verified replacement, old-publisher removal, and manifest review before retries.
#[must_use]
pub fn repair_steps(
    package: &Package,
    context: &FlowContext<'_>,
    steps: Vec<SetupStep>,
) -> Vec<SetupStep> {
    let mut steps: Vec<_> = steps
        .into_iter()
        .filter(|step| !["verify-trusted-publisher", "rerun-release"].contains(&step.id.as_str()))
        .collect();
    if package.registry != crate::model::Registry::Npm
        && !context.manual
        && package.exists_on_registry == Some(true)
    {
        let index = steps
            .iter()
            .position(|step| {
                ["configure-trusted-publisher", "attach-trusted-publisher"]
                    .contains(&step.id.as_str())
            })
            .unwrap_or_default();
        steps.insert(index, SetupStep::new("check-repository-publisher", "Read the current repository's configured publisher", StepKind::Check, "Check the repository, workflow and environment in authenticated settings before attaching a replacement."));
    }
    let index = steps
        .iter()
        .rposition(|step| {
            ["configure-trusted-publisher", "attach-trusted-publisher"].contains(&step.id.as_str())
        })
        .map_or(0, |index| index + 1);
    let mut repairs = vec![
        SetupStep::new("verify-repository-publisher", "Verify the new repository's trusted publisher", StepKind::Check, "Read registry settings and require the new repository, workflow and environment before removing old trust."),
        SetupStep::new("remove-old-publisher", "Remove the old repository's trusted publisher", StepKind::Api, "Remove only publishers naming the old repository, after verifying the replacement; confirm their removal in registry settings.").confirmed(),
    ];
    if package
        .repository_mismatches
        .iter()
        .any(|finding| finding.source == "manifest")
    {
        repairs.push(SetupStep::new("fix-manifest-repository", "Offer the manifest repository URL fix in a reviewed pull request", StepKind::Api, "Create a branch and draft pull request for the repository URL. Pause release retries until it is merged.").confirmed());
    }
    repairs.push(SetupStep::new("rerun-release", "Re-run failed release jobs after repository repairs", StepKind::Check, "Verify the remote default-branch manifests name the current repository before retrying failed release jobs."));
    let mut index = index;
    if package.exists_on_registry != Some(true) {
        if let Some(position) = repairs
            .iter()
            .position(|step| step.id == "fix-manifest-repository")
        {
            steps.insert(0, repairs.remove(position));
            index += 1;
        }
    }
    steps.splice(index..index, repairs);
    steps
}
