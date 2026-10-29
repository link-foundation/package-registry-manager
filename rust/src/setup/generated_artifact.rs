//! Generate and validate real npm artifacts before packing a selected alias.
use super::{
    bail, package_directory, Context, PathBuf, Result, Session, SetupStep, StepKind, Value,
};

impl Session<'_> {
    pub(super) async fn prepare_generated_artifact(&mut self) -> Result<()> {
        let metadata = self
            .plan
            .package
            .audit
            .get("generated_alias")
            .or_else(|| self.plan.package.audit.get("generated_artifact"))
            .context("generated npm artifact has no metadata")?;
        let generator = metadata["generator"].as_str().context(
            "generated npm artifact has no declared generator; configure it before bootstrap",
        )?;
        let source = PathBuf::from(
            self.values
                .get("worktree")
                .context("clean worktree is missing")?,
        )
        .join(package_directory(&self.plan.package.manifest))
        .canonicalize()?;
        let generator = source.join(generator).canonicalize()?;
        if !generator.starts_with(&source) {
            bail!("npm artifact generator must be inside the source package");
        }
        let destination = PathBuf::from(
            self.values
                .get("pack_destination")
                .context("pack destination is missing")?,
        )
        .join("generated");
        for (program, args) in [
            (
                "npm",
                vec![
                    "ci".into(),
                    "--ignore-scripts".into(),
                    "--no-audit".into(),
                    "--no-fund".into(),
                ],
            ),
            (
                "node",
                vec![
                    generator.to_string_lossy().into_owned(),
                    "--output-dir".into(),
                    destination.to_string_lossy().into_owned(),
                ],
            ),
        ] {
            let result = self
                .capture(
                    &SetupStep::new(
                        "generate-artifact",
                        "Generate npm artifacts",
                        StepKind::Check,
                        "Run the declared generator in the clean checkout.",
                    )
                    .command(
                        program,
                        &args.iter().map(String::as_str).collect::<Vec<_>>(),
                    )
                    .cwd(source.to_string_lossy()),
                )
                .await?;
            if result.code != 0 {
                bail!("{program} failed while generating npm artifacts");
            }
        }
        let destination = destination.canonicalize()?;
        let selected = destination.join(&self.plan.package.name).canonicalize()?;
        if !selected.starts_with(&destination) {
            bail!("invalid generated package path");
        }
        let actual: Value =
            serde_json::from_str(&std::fs::read_to_string(selected.join("package.json"))?)?;
        if actual["name"].as_str() != Some(&self.plan.package.name)
            || actual["version"].as_str() != self.plan.package.version.as_deref()
        {
            bail!("generated artifact identity/version differs from inspection; nothing was published");
        }
        self.values.insert(
            "generated_package".into(),
            selected.to_string_lossy().into_owned(),
        );
        Ok(())
    }
}
