//! Static evidence from the argument vectors of Node and Rust release scripts.
use super::regex;
use crate::Registry;
use std::collections::BTreeSet;

pub(super) fn publishes(lines: &[String], registry: Registry, script: &str) -> bool {
    match script.rsplit('.').next() {
        Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts")
            if matches!(registry, Registry::Npm | Registry::CratesIo) =>
        {
            node_publishes(&lines.join("\n"), registry)
        }
        Some("rs") if registry == Registry::CratesIo => rust_publishes(&lines.join("\n")),
        _ => false,
    }
}

fn publish_array(args: &str) -> bool {
    let Some(end) = args.find(']') else {
        return false;
    };
    let array = &args[..=end];
    regex(r#"^\[\s*(?:"publish"|'publish')\s*[,\]]"#).is_match(array)
        && !array.contains("--dry-run")
}

fn node_publishes(source: &str, registry: Registry) -> bool {
    let functions = regex(r"\bfunction\s+(\w+)\s*\(\s*(\w+)[^\n]*?\)\s*\{");
    let functions: Vec<_> = functions.captures_iter(source).collect();
    let mut forwards = BTreeSet::new();
    let calls = regex(
        r#"\b(?:exec(?:File)?|spawn|run)(?:Sync)?\s*\(\s*(?:"(npm|pnpm|yarn|cargo)"|'(npm|pnpm|yarn|cargo)')\s*,\s*"#,
    );
    for call in calls.captures_iter(source) {
        let program = call.get(1).or_else(|| call.get(2)).expect("program name");
        if (program.as_str() == "cargo") != (registry == Registry::CratesIo) {
            continue;
        }
        let call = call.get(0).expect("call prefix");
        let args = &source[call.end()..];
        if publish_array(args) {
            return true;
        }
        if let Some(enclosing) = functions
            .iter()
            .rev()
            .find(|func| func.get(0).expect("function header").start() < call.start())
        {
            let header = enclosing.get(0).expect("function header");
            let between = &source[header.end()..call.start()];
            if !between.lines().any(|line| line.starts_with('}'))
                && regex(&format!(r"^{}\s*[,)]", &enclosing[2])).is_match(args)
            {
                forwards.insert(enclosing[1].to_owned());
            }
        }
    }
    // A callback can default to the release helper, e.g. run = runNpm.
    for capture in regex(r"\b(\w+)\s*=\s*(\w+)\s*[,)]").captures_iter(source) {
        if forwards.contains(&capture[2]) {
            forwards.insert(capture[1].to_owned());
        }
    }
    regex(r"\b(\w+)\s*\(\s*").captures_iter(source).any(|call| {
        forwards.contains(&call[1])
            && publish_array(&source[call.get(0).expect("call prefix").end()..])
    })
}

fn rust_publishes(source: &str) -> bool {
    const COMMAND: &str = r#"Command::new\s*\(\s*"cargo"\s*\)"#;
    const PUBLISH: &str = r#"\s*\.\s*arg\s*\(\s*"publish"\s*\)"#;
    if regex(&format!("{COMMAND}{PUBLISH}[^;]*"))
        .find_iter(source)
        .any(|statement| !statement.as_str().contains("--dry-run"))
    {
        return true;
    }
    regex(&format!(r"\blet\s+(?:mut\s+)?(\w+)\s*=\s*{COMMAND}\s*;"))
        .captures_iter(source)
        .any(|binding| {
            let tail = &source[binding.get(0).expect("command binding").end()..];
            let args = regex(&format!(r"\b{}\s*\.\s*args?\s*\([^;]*", &binding[1]));
            let publish = regex(&format!("^{}{PUBLISH}", &binding[1]));
            let statements: Vec<_> = args.find_iter(tail).map(|item| item.as_str()).collect();
            statements
                .iter()
                .any(|statement| publish.is_match(statement))
                && statements
                    .iter()
                    .all(|statement| !statement.contains("--dry-run"))
        })
}

pub(super) fn checks_script(line: &str, script: &str, source: &str) -> bool {
    // An argv-based release dispatcher can also serve preflight and verification.
    regex(r"\bprocess\.argv\b").is_match(source)
        && line.split_once(script).is_some_and(|(_, suffix)| {
            regex(r#"^["']?\s+["']?(?:check|verify)(?:["'\s;&|]|$)"#).is_match(suffix)
        })
}

pub(super) fn references(lines: &[String]) -> impl Iterator<Item = String> + '_ {
    let workspace = regex(r"\$\{\{\s*github\.workspace\s*\}\}");
    let prefix = regex(r"^\$(?:\{\w+\}|\w+)/");
    let separator = regex(r#"[\s"'=;()|&]+"#);
    let reference =
        regex(r"^(?:\.{1,2}/)?[\w@.-]+(?:/[\w@.-]+)*\.(?:mjs|cjs|js|mts|cts|ts|rs|sh|bash|py)$");
    lines.iter().flat_map(move |line| {
        // Only the literal suffix of a script under a runtime directory can
        // be resolved against the checkout.
        let text = workspace.replace_all(line, "$$GITHUB_WORKSPACE");
        separator
            .split(&text)
            .filter_map(|token| {
                let file = prefix.replace(token, "");
                reference.is_match(&file).then(|| file.into_owned())
            })
            .collect::<Vec<_>>()
    })
}
