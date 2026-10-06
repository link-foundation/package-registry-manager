use package_registry_manager::python::{
    apply_python, choose_python, python_matches, PythonInterpreter,
};
use package_registry_manager::{build_plans, inspect_repository};

#[test]
fn matches_stable_requires_python_specifiers() {
    for (spec, version, expected) in [
        (">=3.13", "3.12.9", false),
        (">=3.13", "3.14.0", true),
        (">=3.13,<3.14", "3.14.0", false),
        ("~=3.13.0", "3.13.7", true),
        ("~=3.13.0", "3.14.0", false),
        ("~=3.13", "3.14.0", true),
        ("==3.13.*", "3.13.2", true),
        ("!=3.13.*", "3.13.2", false),
        (">3.13,<=3.14", "3.14.0", true),
        (">=3.13,!=3.13.1", "3.13.1", false),
        ("garbage", "3.13.0", false),
        (">=3.13", "3.14.0rc1", false),
    ] {
        assert_eq!(python_matches(version, spec), expected, "{version} {spec}");
    }
}

#[test]
fn discovers_and_uses_a_compatible_interpreter() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
    std::fs::write(root.path().join(".github/workflows/release.yml"), "on: workflow_dispatch\njobs:\n  publish:\n    permissions: {id-token: write}\n    steps:\n      - run: python -m twine upload dist/*\n").unwrap();
    std::fs::write(
        root.path().join("pyproject.toml"),
        "[project]\nname=\"tool\"\nrequires-python=\">=3.13\"\n",
    )
    .unwrap();
    let inspection = inspect_repository(root.path()).unwrap();
    assert_eq!(
        inspection.packages[0].requires_python.as_deref(),
        Some(">=3.13")
    );
    let tools = vec![
        PythonInterpreter {
            program: "python".into(),
            version: "3.12.9".into(),
        },
        PythonInterpreter {
            program: "python3.13".into(),
            version: "3.13.7".into(),
        },
    ];
    assert_eq!(
        choose_python(&tools, ">=3.13").unwrap().program,
        "python3.13"
    );
    let mut plan = build_plans(&inspection).remove(0);
    apply_python(&mut plan, &tools);
    assert_eq!(
        plan.steps[0].command.as_ref().unwrap().program,
        "python3.13"
    );
    assert_eq!(plan.prerequisites[0].ok, Some(true));
    apply_python(&mut plan, &tools[..1]);
    assert_eq!(plan.prerequisites[0].ok, Some(false));
}
