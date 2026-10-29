use package_registry_manager::workflows::Workflow;
use package_registry_manager::{build_plans_with, inspect_repository, PlanOptions, Registry};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

#[test]
fn nested_reusable_callers_keep_the_outer_identity_and_leaf_environment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let workflows: Vec<_> = [
        ("outer.yml", "on: push\njobs:\n  call:\n    uses: ./.github/workflows/middle.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
        ("middle.yml", "on: workflow_call\njobs:\n  call:\n    uses: ./.github/workflows/inner.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
        ("inner.yml", "on: workflow_call\njobs:\n  release:\n    environment: release-main\n    steps:\n      - run: cargo publish\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
    ].into_iter().map(|(name, contents)| Workflow {name: name.into(), contents: contents.into()}).collect();
    let publisher = package_registry_manager::publishers::detect_publisher(
        &root,
        &workflows,
        Registry::CratesIo,
    );
    assert_eq!(publisher.workflow.as_deref(), Some("outer.yml"));
    assert_eq!(publisher.environment.as_deref(), Some("release-main"));
    let inspection = inspect_repository(&root).unwrap();
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &inspection.packages,
        &workflows,
    )
    .unwrap();
    assert_eq!(files.len(), 3);
    for contents in files.values() {
        assert!(contents.contains("id-token: write"));
        assert!(contents.contains("secrets.CARGO_TOKEN"));
    }
}

#[test]
fn pypi_conversion_preserves_conditions_and_package_directory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let mut package = inspect_repository(&root).unwrap().packages.remove(0);
    package.registry = Registry::PyPi;
    let contents = "on: push\njobs:\n  publish:\n    steps:\n      - name: Upload Python package\n        id: publish\n        if: steps.release.outputs.version != ''\n        run: python -m twine upload python/dist/*\n        env:\n          TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}\n          OTHER: retained\n";
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &[package.clone()],
        &[Workflow {
            name: "release.yml".into(),
            contents: contents.into(),
        }],
    )
    .unwrap();
    assert!(files["release.yml"].contains("id: publish"));
    assert!(files["release.yml"].contains("if: steps.release.outputs.version"));
    assert!(files["release.yml"].contains("packages-dir: 'python/dist/'"));
    assert!(files["release.yml"].contains("OTHER: retained"));
    assert!(files["release.yml"].contains("password: ${{ secrets.PYPI_API_TOKEN"));
    assert!(
        package_registry_manager::workflow_migration::workflow_migration_proposal(
            &root,
            &[package],
            &[Workflow {
                name: "release.yml".into(),
                contents: contents.replace(
                    "twine upload python/dist/*",
                    "twine upload --skip-existing python/dist/*"
                )
            }]
        )
        .is_err()
    );
}

#[test]
fn reusable_caller_environment_and_migration_match_javascript_golden_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    assert_eq!(
        inspection.packages[0].environment.as_deref(),
        Some("release-main")
    );
    let plans = build_plans_with(
        &inspection,
        &BTreeSet::from([Registry::CratesIo]),
        &PlanOptions {
            migrate: true,
            ..PlanOptions::default()
        },
    );
    assert_eq!(plans[0].steps[0].id, "add-publishing-workflow");
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &inspection.packages,
        &package_registry_manager::workflows::read_workflows(&root).unwrap(),
    )
    .unwrap();
    let expected: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/token-rotation/expected.json"
    ))
    .unwrap();
    assert_eq!(files, expected);
}

#[test]
fn oidc_evidence_requires_successful_authentication_and_an_actual_upload() {
    use package_registry_manager::migration_evidence::published_through_oidc;
    let job = json!({"conclusion":"success","steps":[{"name":"Authenticate with crates.io (trusted publishing)","conclusion":"success"},{"name":"Publish crate","conclusion":"success"}]});
    assert!(published_through_oidc(
        &job,
        "Published crate v1.0.1",
        Registry::CratesIo
    ));
    assert!(!published_through_oidc(
        &job,
        "version already exists; skipping publish",
        Registry::CratesIo
    ));
    assert!(!published_through_oidc(
        &json!({"conclusion":"success","steps":[]}),
        "Published crate v1.0.1",
        Registry::CratesIo
    ));
}

#[test]
fn crates_auth_preserves_multiline_guards_and_handles_different_conditions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    let contents = "on: push\njobs:\n  release:\n    steps:\n      - name: Publish first crate\n        if: >-\n          steps.first.outputs.changed == 'true'\n        run: cargo publish -p first\n";
    let propose = |text: &str| {
        package_registry_manager::workflow_migration::workflow_migration_proposal(
            &root,
            &inspection.packages,
            &[Workflow {
                name: "release.yml".into(),
                contents: text.into(),
            }],
        )
    };
    let files = propose(contents).unwrap();
    assert_eq!(
        files["release.yml"]
            .matches("steps.first.outputs.changed")
            .count(),
        2
    );
    let differing = format!("{contents}      - name: Publish second crate\n        if: steps.second.outputs.changed == 'true'\n        run: cargo publish -p second\n");
    assert!(propose(&differing).unwrap()["release.yml"].contains("prm_auth_2.outputs.token"));
}

#[test]
fn inherited_secrets_preflights_inline_tokens_and_optional_reusable_inputs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    let contents = "on: workflow_call\nenv:\n  CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\njobs:\n  publish:\n    steps:\n      - run: node scripts/publish-preflight.mjs crates\n      - run: cargo publish --token '${{ secrets.CARGO_TOKEN }}'\n";
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &inspection.packages,
        &[Workflow {
            name: "release.yml".into(),
            contents: contents.into(),
        }],
    )
    .unwrap();
    let text = &files["release.yml"];
    assert!(!text.contains("--token"));
    assert!(text.contains("CARGO_REGISTRY_TOKEN:\n        required: false"));
    assert!(text.find("uses: rust-lang").unwrap() < text.find("run: node").unwrap());
    assert_eq!(
        text.matches("CARGO_TOKEN: ${{ steps.prm_auth.outputs.token")
            .count(),
        2
    );
}

#[test]
fn command_substitutions_and_executed_command_variables_are_publishers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("scripts")).unwrap();
    let script = root.path().join("scripts/publish.mjs");
    std::fs::write(&script, "let command = 'cargo publish --allow-dirty';\ncommand += ' --verbose';\nexecSync(command);\n").unwrap();
    for command in [
        "if ! OUT=$(cargo publish 2>&1); then exit 1; fi",
        "OUT=`cargo publish`",
        "node scripts/publish.mjs",
    ] {
        let workflow = Workflow {
            name: "release.yml".into(),
            contents: format!("on: push\njobs:\n  publish:\n    steps:\n      - run: {command}\n"),
        };
        assert_eq!(
            package_registry_manager::publishers::detect_publisher(
                root.path(),
                &[workflow],
                Registry::CratesIo
            )
            .workflow
            .as_deref(),
            Some("release.yml"),
            "{command}"
        );
    }
    std::fs::write(
        &script,
        "const message = 'cargo publish';\nconsole.log(message);",
    )
    .unwrap();
    let workflow = Workflow {
        name: "release.yml".into(),
        contents: "jobs:\n  publish:\n    steps:\n      - run: node scripts/publish.mjs\n".into(),
    };
    assert!(package_registry_manager::publishers::detect_publisher(
        root.path(),
        &[workflow],
        Registry::CratesIo
    )
    .workflow
    .is_none());
}

#[test]
fn policy_evidence_must_come_from_the_same_publishing_job() {
    let workflow = Workflow {name:"release.yml".into(), contents:"on: push\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_TOKEN }}\n  unrelated:\n    permissions:\n      id-token: write\n    steps:\n      - uses: rust-lang/crates-io-auth-action@v1\n        continue-on-error: true\n        env:\n          TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_TOKEN }}\n".into()};
    assert_eq!(
        package_registry_manager::publishing_state::publishing_policy(
            Path::new("."),
            &[workflow],
            Registry::CratesIo
        ),
        "token-only"
    );
}

#[test]
fn migration_preserves_already_converted_jobs_and_flags_variable_gated_auth() {
    use package_registry_manager::workflow_migration::workflow_migration_proposal;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let mut package = inspect_repository(&root).unwrap().packages.remove(0);
    package.registry = Registry::Npm;
    let contents = "on: push\njobs:\n  first:\n    steps:\n      - run: npm publish\n        env:\n          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n";
    let first = workflow_migration_proposal(
        &root,
        &[package.clone()],
        &[Workflow {
            name: "release.yml".into(),
            contents: contents.into(),
        }],
    )
    .unwrap()
    .remove("release.yml")
    .unwrap();
    let second = workflow_migration_proposal(&root, &[package], &[Workflow {name: "release.yml".into(), contents: format!("{first}  second:\n    steps:\n      - run: npm publish\n        env:\n          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n")}]).unwrap().remove("release.yml").unwrap();
    assert_eq!(second.split("  second:").next().unwrap(), first);
    assert_eq!(second.matches("outcome == 'failure'").count(), 2);
    let contents = "on: push\njobs:\n  release:\n    permissions:\n      id-token: write\n    steps:\n      - uses: rust-lang/crates-io-auth-action@v1\n        id: auth\n        if: vars.OIDC_ENABLED == 'true'\n        continue-on-error: true\n      - run: cargo publish\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_TOKEN }}\n";
    let policy = package_registry_manager::publishing_state::publishing_policy(
        &root,
        &[Workflow {
            name: "release.yml".into(),
            contents: contents.into(),
        }],
        Registry::CratesIo,
    );
    assert_eq!(policy, "token-only");
}
