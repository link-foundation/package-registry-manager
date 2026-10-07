use package_registry_manager::workflow_proposal::workflow_proposal;
use package_registry_manager::workflows::Workflow;
use package_registry_manager::{inspect_repository, Package, Registry};

fn fixture() -> (tempfile::TempDir, package_registry_manager::Inspection) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
    std::fs::create_dir_all(root.path().join("python")).unwrap();
    std::fs::write(root.path().join("package.json"), r#"{"name":"tool"}"#).unwrap();
    std::fs::write(
        root.path().join("python/pyproject.toml"),
        "[project]\nname='tool'\nrequires-python='>=3.13'\n",
    )
    .unwrap();
    std::fs::write(root.path().join(".github/workflows/release.yml"), "on: workflow_dispatch\njobs:\n  pypi:\n    steps:\n      - run: python -m twine upload dist/*\nenv:\n  OTHER: kept\n").unwrap();
    let inspection = inspect_repository(root.path()).unwrap();
    (root, inspection)
}

#[test]
fn adds_oidc_jobs_in_the_shared_release_workflow() {
    let (root, inspection) = fixture();
    let npm = inspection
        .packages
        .iter()
        .find(|p| p.registry == Registry::Npm)
        .unwrap()
        .clone();
    let mut cargo = npm.clone();
    cargo.registry = Registry::CratesIo;
    cargo.manifest = "rust/Cargo.toml".into();
    let workflows = package_registry_manager::workflows::read_workflows(root.path()).unwrap();
    let proposal = workflow_proposal(&inspection, &[npm, cargo], &workflows, None, None).unwrap();
    assert_eq!(proposal.workflow, "release.yml");
    for expected in [
        "npm install --global npm@^11",
        "npm publish --provenance --access public",
        "rust-lang/crates-io-auth-action@v1",
        "cargo publish",
        "id-token: write",
        "github.ref == format(",
    ] {
        assert!(proposal.contents.contains(expected), "{expected}");
    }
    assert!(!proposal.contents.contains("NPM_TOKEN"));
    assert!(proposal.contents.ends_with("env:\n  OTHER: kept\n"));
    let updated = vec![Workflow {
        name: proposal.workflow,
        contents: proposal.contents,
    }];
    for registry in [Registry::Npm, Registry::CratesIo, Registry::PyPi] {
        let publisher =
            package_registry_manager::publishers::detect_publisher(root.path(), &updated, registry);
        assert_eq!(publisher.workflow.as_deref(), Some("release.yml"));
        assert_eq!(publisher.jobs.len(), 1);
    }
}

#[test]
fn creates_pypi_workflow_using_project_python_requirement() {
    let (_root, mut inspection) = fixture();
    inspection
        .packages
        .iter_mut()
        .for_each(|p| p.workflow = None);
    inspection.repository.release_workflow = None;
    let pypi: Vec<Package> = inspection
        .packages
        .iter()
        .filter(|p| p.registry == Registry::PyPi)
        .cloned()
        .collect();
    let proposal = workflow_proposal(&inspection, &pypi, &[], None, None).unwrap();
    assert!(proposal.contents.contains("workflow_dispatch"));
    assert!(proposal
        .contents
        .contains("python-version-file: 'python/pyproject.toml'"));
    assert!(proposal.contents.contains("packages-dir: 'python/dist/'"));
    let mut unconstrained = pypi;
    unconstrained[0].requires_python = None;
    let proposal = workflow_proposal(&inspection, &unconstrained, &[], None, None).unwrap();
    assert!(proposal.contents.contains("python-version: '3.x'"));
    assert!(!proposal.contents.contains("python-version-file"));
}

#[test]
fn rejects_ambiguous_workflows_and_paths_and_preserves_environment_override() {
    let (_root, mut inspection) = fixture();
    inspection.packages[0].workflow = Some("one.yml".into());
    inspection.packages[1].workflow = Some("two.yml".into());
    assert!(
        workflow_proposal(&inspection, &inspection.packages, &[], None, None)
            .unwrap_err()
            .to_string()
            .contains("--workflow")
    );
    let mut unsafe_package = inspection.packages[0].clone();
    unsafe_package.manifest = "../escape/package.json".into();
    assert!(workflow_proposal(
        &inspection,
        &[unsafe_package],
        &[],
        Some("chosen.yml"),
        None
    )
    .is_err());
    let workflow = Workflow {
        name: "chosen.yml".into(),
        contents: "jobs:\n".into(),
    };
    let proposal = workflow_proposal(
        &inspection,
        &inspection.packages,
        &[workflow],
        Some("chosen.yml"),
        Some("release"),
    )
    .unwrap();
    assert!(proposal.contents.contains("environment: 'release'"));
}

#[test]
fn proposals_guard_versions_and_follow_release_output() {
    let (_root, inspection) = fixture();
    let workflows = vec![Workflow { name: "release.yml".into(), contents: "on: push\njobs:\n  release:\n    outputs:\n      published_version: ${{ steps.release.outputs.version }}\n    steps:\n      - run: ./release.sh\n".into() }];
    let proposal =
        workflow_proposal(&inspection, &inspection.packages, &workflows, None, None).unwrap();
    for expected in [
        "needs: release",
        "needs.release.outputs.published_version",
        "id: version-check",
        "Cache-Control",
        "steps.version-check.outputs.publish == 'true'",
        "skip-existing: true",
    ] {
        assert!(proposal.contents.contains(expected), "{expected}");
    }
}
