use package_registry_manager::npm_policy::{name_variants, policy_refusal};
use package_registry_manager::workflows::{grants_packages_write, publishing_workflow, Workflow};
use package_registry_manager::{inspect_repository, InspectOptions, Registry};
use std::fs;
use std::process::Command;

#[test]
fn comments_and_container_preflight_are_not_publishers() {
    let workflows = [Workflow {name:"preflight.yml".into(),contents:"on: push\n# docker.io/acme/tool ghcr.io/acme/tool\njobs:\n  preflight:\n    steps:\n      - uses: docker/login-action@v3\n        with:\n          password: ${{ secrets.DOCKERHUB_TOKEN }}\n      - run: echo 'docker push ghcr.io/acme/tool'\n".into()}];
    for registry in [Registry::DockerHub, Registry::Ghcr] {
        assert!(publishing_workflow(&workflows, registry).is_none());
    }
    assert!(!grants_packages_write(
        "permissions: read-all # packages: write"
    ));
}

#[test]
fn registry_wire_names_and_npm_policy_are_consistent() {
    for registry in Registry::ALL {
        assert_eq!(
            serde_json::to_value(registry).unwrap(),
            registry.to_string()
        );
    }
    assert!(name_variants("@acme/gh-upload").contains(&"@acme/ghupload".into()));
    assert!(name_variants(&format!("{}b", "a-".repeat(30))).len() <= 64);
    assert!(policy_refusal(
        "E403 package name is too similar to existing package"
    ));
    assert!(!policy_refusal("EOTP invalid code"));
}

fn write(root: &std::path::Path, name: &str, contents: &str) {
    let file = root.join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, contents).unwrap();
}

#[test]
fn checks_publishing_coverage_for_both_wrapper_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "package.json",
        r#"{"name":"gh-upload","version":"1.0.0"}"#,
    );
    write(
        root,
        "packages/old/package.json",
        r#"{"name":"gh-upload-log","version":"1.0.0","dependencies":{"gh-upload":"1.0.0"}}"#,
    );
    write(
        root,
        "packages/orphan/package.json",
        r#"{"name":"orphan","version":"1.0.0"}"#,
    );
    write(root,".github/workflows/release.yml","on: push\njobs:\n  release:\n    permissions: {id-token: write}\n    steps:\n      - run: npm publish\n      - run: npm publish\n        working-directory: packages/old\n");
    let inspected = inspect_repository(root).unwrap();
    for name in ["gh-upload", "gh-upload-log"] {
        assert_eq!(
            inspected
                .packages
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .workflow
                .as_deref(),
            Some("release.yml")
        );
    }
    let orphan = inspected
        .packages
        .iter()
        .find(|p| p.name == "orphan")
        .unwrap();
    assert!(orphan.workflow.is_none());
    assert!(orphan
        .warnings
        .iter()
        .any(|w| w.contains("CI releases will not reach it")));
    write(
        root,
        "package.json",
        r#"{"name":"gh-upload","version":"2.0.0"}"#,
    );
    let inspected = inspect_repository(root).unwrap();
    assert!(inspected
        .packages
        .iter()
        .find(|p| p.name == "gh-upload-log")
        .unwrap()
        .warnings
        .iter()
        .any(|w| w.contains("wrapper") && w.contains("2.0.0")));
}

#[test]
fn discovers_additional_oidc_and_token_publishers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "tool.gemspec",
        "Gem::Specification.new do |s|\n s.name = 'tool'\n s.version = '1.0.0'\nend",
    );
    write(
        root,
        "jsr.json",
        r#"{"name":"@acme/tool","version":"1.0.0"}"#,
    );
    write(
        root,
        "extension/package.json",
        r#"{"name":"tool","version":"1.0.0","publisher":"acme","engines":{"vscode":"^1.80.0"}}"#,
    );
    write(root,".github/workflows/release.yml","on: workflow_dispatch\njobs:\n  release:\n    permissions: {id-token: write}\n    steps:\n      - run: gem push tool.gem\n      - run: deno publish\n      - run: vsce publish\n        env: {VSCE_PAT: '${{ secrets.VSCE_PAT }}'}\n      - run: ovsx publish\n        env: {OVSX_PAT: '${{ secrets.OVSX_PAT }}'}\n      - uses: acme/chrome-webstore-upload@v1\n        with:\n          token: ${{ secrets.CHROME_WEB_STORE_REFRESH_TOKEN }}\n");
    let inspected = inspect_repository(root).unwrap();
    for registry in [
        Registry::RubyGems,
        Registry::Jsr,
        Registry::VsCodeMarketplace,
        Registry::OpenVsx,
        Registry::ChromeWebStore,
    ] {
        let item = inspected
            .packages
            .iter()
            .find(|p| p.registry == registry)
            .unwrap_or_else(|| panic!("missing {registry}"));
        assert_eq!(item.workflow.as_deref(), Some("release.yml"));
    }
    assert_eq!(
        inspected
            .packages
            .iter()
            .find(|p| p.registry == Registry::VsCodeMarketplace)
            .unwrap()
            .token_secrets,
        ["VSCE_PAT"]
    );
}

#[test]
fn ref_inspection_reports_main_and_preserves_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.test"]);
    git(&["config", "user.name", "Test"]);
    write(
        &root,
        "package.json",
        r#"{"name":"gh-upload-log","version":"1.0.0"}"#,
    );
    git(&["add", "."]);
    git(&["commit", "-m", "main version"]);
    let main = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-b", "rename"]);
    write(
        &root,
        "package.json",
        r#"{"name":"gh-upload","version":"1.1.0"}"#,
    );
    git(&["add", "."]);
    git(&["commit", "-m", "rename"]);
    git(&["checkout", "main"]);
    let origin = temp.path().join("origin.git");
    git(&[
        "clone",
        "--bare",
        root.to_str().unwrap(),
        origin.to_str().unwrap(),
    ]);
    git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    let inspected = package_registry_manager::bootstrap_reference::inspect_reference(
        &root,
        "rename",
        InspectOptions::default(),
    )
    .unwrap();
    assert_eq!(inspected.packages[0].name, "gh-upload");
    assert!(inspected.packages[0]
        .warnings
        .iter()
        .any(|w| w.contains("gh-upload-log@1.0.0")));
    assert_eq!(git(&["rev-parse", "HEAD"]), main);
    assert_eq!(
        git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn multiple_npm_manifests_keep_root_reusable_publishing_workflow() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "package.json", r#"{"name":"tool","version":"1.0.0"}"#);
    write(
        root,
        "packages/orphan/package.json",
        r#"{"name":"orphan","version":"1.0.0"}"#,
    );
    write(root, ".github/workflows/release.yml", "on: push\njobs:\n  publish:\n    permissions: {id-token: write}\n    uses: ./.github/workflows/publish.yml\n");
    write(root, ".github/workflows/publish.yml", "on: workflow_call\njobs:\n  npm:\n    permissions: {id-token: write}\n    steps:\n      - run: npm publish\n");
    let inspected = inspect_repository(root).unwrap();
    assert_eq!(
        inspected
            .packages
            .iter()
            .find(|p| p.name == "tool")
            .unwrap()
            .workflow
            .as_deref(),
        Some("release.yml")
    );
    assert!(inspected
        .packages
        .iter()
        .find(|p| p.name == "orphan")
        .unwrap()
        .workflow
        .is_none());
}

#[test]
fn inert_build_push_names_and_unrelated_push_inputs_are_not_publishers() {
    for contents in [
        "on: push\njobs:\n  preflight:\n    steps:\n      - name: 'uses: docker/build-push-action@v6'\n        run: echo ready\n      - uses: acme/check@v1\n        with:\n          push: true\n          registry: ghcr.io\n",
        "on: push\njobs:\n  preflight:\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          push: false\n          tags: ghcr.io/acme/tool\n      - uses: acme/check@v1\n        with:\n          push: true\n",
    ] {
        let workflows = [Workflow { name: "preflight.yml".into(), contents: contents.into() }];
        for registry in [Registry::DockerHub, Registry::Ghcr] {
            assert!(publishing_workflow(&workflows, registry).is_none());
        }
    }
}
