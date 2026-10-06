//! Selection of an installed interpreter satisfying a project's requires-python.

use std::collections::BTreeSet;

use command_stream::StreamingRunner;

use crate::auth_urls::resolve_program;
use crate::model::{CommandSpec, Prerequisite, Registry, SetupPlan};

fn release(value: &str) -> Option<Vec<u64>> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    parts.into_iter().map(|part| part.parse().ok()).collect()
}

fn padded(parts: &[u64]) -> [u64; 3] {
    std::array::from_fn(|index| parts.get(index).copied().unwrap_or(0))
}

/// Tests stable interpreter releases against Python version specifiers.
/// Unsupported or prerelease syntax is rejected rather than guessed.
#[must_use]
pub fn python_matches(version: &str, requirement: &str) -> bool {
    let Some(actual) = release(version) else {
        return false;
    };
    if requirement.trim().is_empty() {
        return true;
    }
    requirement.split(',').all(|clause| {
        let clause = clause.trim();
        let Some(operator) = ["~=", "==", "!=", "<=", ">=", "<", ">"]
            .into_iter()
            .find(|operator| clause.starts_with(operator))
        else {
            return false;
        };
        let text = clause[operator.len()..].trim();
        let wildcard = text.ends_with(".*");
        let Some(expected) = release(text.strip_suffix(".*").unwrap_or(text)) else {
            return false;
        };
        if wildcard {
            let equal = actual.starts_with(&expected);
            return match operator {
                "==" => equal,
                "!=" => !equal,
                _ => false,
            };
        }
        let actual = padded(&actual);
        let minimum = padded(&expected);
        match operator {
            "==" => actual == minimum,
            "!=" => actual != minimum,
            ">=" => actual >= minimum,
            "<=" => actual <= minimum,
            ">" => actual > minimum,
            "<" => actual < minimum,
            "~=" if expected.len() >= 2 => {
                let mut upper = expected[..expected.len() - 1].to_vec();
                let index = upper.len() - 1;
                let Some(next) = upper[index].checked_add(1) else {
                    return false;
                };
                upper[index] = next;
                actual >= minimum && actual < padded(&upper)
            }
            _ => false,
        }
    })
}

/// An installed Python executable and its stable version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonInterpreter {
    /// Executable name on PATH; used as a separate argv entry.
    pub program: String,
    /// Stable major.minor.patch release.
    pub version: String,
}

/// Selects the first installed interpreter satisfying the manifest.
#[must_use]
pub fn choose_python<'a>(
    tools: &'a [PythonInterpreter],
    requirement: &str,
) -> Option<&'a PythonInterpreter> {
    tools
        .iter()
        .find(|tool| python_matches(&tool.version, requirement))
}

fn candidate_programs() -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            if let Ok(entries) = std::fs::read_dir(directory) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let stem = name.strip_suffix(".exe").unwrap_or(&name);
                    if stem.strip_prefix("python3.").is_some_and(|minor| {
                        !minor.is_empty() && minor.chars().all(|ch| ch.is_ascii_digit())
                    }) {
                        names.insert(name);
                    }
                }
            }
        }
    }
    ["python".to_owned(), "python3".to_owned()]
        .into_iter()
        .chain(names)
        .take(32)
        .collect()
}

/// Probes interpreter versions without running repository code.
pub async fn probe_python(verbose: bool) -> Vec<PythonInterpreter> {
    let mut tools = Vec::new();
    for program in candidate_programs() {
        if verbose {
            eprintln!("+ {program} --version");
        }
        let executable = resolve_program(&program);
        if let Ok(output) = StreamingRunner::from_argv(executable, ["--version"])
            .collect()
            .await
        {
            if output.code == 0 {
                if let Some(version) = output
                    .stdout
                    .to_string()
                    .trim()
                    .strip_prefix("Python ")
                    .filter(|version| release(version).is_some())
                {
                    tools.push(PythonInterpreter {
                        program,
                        version: version.to_owned(),
                    });
                }
            }
        }
    }
    tools
}

/// Adds prerequisites and uses the chosen interpreter for the build step.
pub fn apply_python(plan: &mut SetupPlan, tools: &[PythonInterpreter]) {
    if plan.registry != Registry::PyPi {
        return;
    }
    let Some(build) = plan
        .steps
        .iter_mut()
        .find(|step| step.id == "build-package")
    else {
        return;
    };
    let requirement = plan.package.requires_python.as_deref().unwrap_or_default();
    let selected = choose_python(tools, requirement);
    plan.prerequisites.retain(|item| item.id != "python");
    plan.prerequisites.push(Prerequisite {
        id: "python".to_owned(),
        title: "Python".to_owned(),
        detected: selected.map_or_else(
            || "no compatible interpreter found".to_owned(),
            |tool| format!("{} ({})", tool.program, tool.version),
        ),
        required: format!(
            "Install Python satisfying {} and its build module (python -m pip install build)",
            if requirement.is_empty() {
                "the project requirements"
            } else {
                requirement
            }
        ),
        ok: Some(selected.is_some()),
    });
    if let Some(tool) = selected {
        build.command = Some(CommandSpec {
            program: tool.program.clone(),
            args: vec!["-m".to_owned(), "build".to_owned()],
        });
    }
}
