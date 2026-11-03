//! Gradle-built Maven Central publications, read as text and never run.
//!
//! Classic `maven-publish` coordinates and the vanniktech plugin's
//! `coordinates(group, artifact, version)` with Kotlin Multiplatform platform
//! publications. A root POM alone never certifies the platform variants.
use crate::model::{Package, Registry, SetupStep, StepKind};
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Kotlin Multiplatform targets and the artifact suffix each publishes under.
const KMP_TARGETS: [(&str, &str); 26] = [
    ("jvm", "jvm"),
    ("js", "js"),
    ("wasmJs", "wasm-js"),
    ("wasmWasi", "wasm-wasi"),
    ("androidTarget", "android"),
    ("android", "android"),
    ("iosArm64", "iosarm64"),
    ("iosX64", "iosx64"),
    ("iosSimulatorArm64", "iossimulatorarm64"),
    ("macosArm64", "macosarm64"),
    ("macosX64", "macosx64"),
    ("tvosArm64", "tvosarm64"),
    ("tvosX64", "tvosx64"),
    ("tvosSimulatorArm64", "tvossimulatorarm64"),
    ("watchosArm32", "watchosarm32"),
    ("watchosArm64", "watchosarm64"),
    ("watchosX64", "watchosx64"),
    ("watchosSimulatorArm64", "watchossimulatorarm64"),
    ("watchosDeviceArm64", "watchosdevicearm64"),
    ("linuxX64", "linuxx64"),
    ("linuxArm64", "linuxarm64"),
    ("mingwX64", "mingwx64"),
    ("androidNativeArm32", "androidnativearm32"),
    ("androidNativeArm64", "androidnativearm64"),
    ("androidNativeX86", "androidnativex86"),
    ("androidNativeX64", "androidnativex64"),
];

/// Gradle tasks that publish to the Central Portal, by plugin.
pub const GRADLE_PUBLISH_TASKS: [&str; 4] = [
    "publishToMavenCentral",
    "publishAndReleaseToMavenCentral",
    "publishAllPublicationsToMavenCentralRepository",
    "publish",
];

const SKIP_REASON: &str =
    "Gradle application/build definition has no resolved Maven publication coordinates and component";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}

fn capture(pattern: &str, text: &str) -> Option<String> {
    regex(pattern)
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
}

/// Removes Kotlin/Groovy comments while keeping `//` inside URLs.
#[must_use]
pub fn strip_gradle_comments(contents: &str) -> String {
    let text = regex(r"(?s)/\*.*?\*/").replace_all(contents, "");
    regex(r#"(^|[^:"\\])//[^\n]*"#)
        .replace_all(&text, "${1}")
        .into_owned()
}

fn parse_properties(contents: &str) -> BTreeMap<String, String> {
    let pattern = regex(r"^\s*([\w.-]+)\s*[=:]\s*(.*?)\s*$");
    contents
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| pattern.captures(line))
        .map(|captures| (captures[1].to_owned(), captures[2].to_owned()))
        .collect()
}

fn read_sibling(directory: &Path, name: &str) -> Result<Option<String>> {
    let path = directory.join(name);
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Ok(None);
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// A top-level assignment of `key`; plugin declarations such as
/// `kotlin("jvm") version "2.0.0"` never name the project version.
fn literal(key: &str, text: &str) -> Option<String> {
    let mut patterns = vec![
        format!(r#"(?m)^{key}\s*(?:=\s*)?['"]([^'"$]+)['"]"#),
        format!(r#"(?m)(?:^|[\s{{;]){key}\s*=\s*['"]([^'"$]+)['"]"#),
    ];
    if key != "version" {
        patterns.push(format!(r#"(?m)(?:^|[\s{{;]){key}\s+['"]([^'"$]+)['"]"#));
    }
    patterns.iter().find_map(|pattern| capture(pattern, text))
}

fn catalog_version(toml: &str, key: &str) -> Option<String> {
    let mut inside = false;
    let key = regex::escape(key).replace(r"\.", "[-_.]");
    let pattern = regex(&format!(r#"^\s*{key}\s*=\s*"([^"]+)""#));
    for line in toml.lines() {
        if line.trim_start().starts_with('[') {
            inside = line.trim() == "[versions]";
        } else if inside {
            if let Some(captures) = pattern.captures(line) {
                return Some(captures[1].to_owned());
            }
        }
    }
    None
}

struct Context<'a> {
    code: &'a str,
    directory: &'a Path,
    properties: BTreeMap<String, String>,
    problems: Vec<String>,
}

impl Context<'_> {
    fn resolve(&mut self, expression: Option<&str>, role: &str) -> Result<Option<String>> {
        let Some(value) = expression.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if let Some(captures) = regex(r#"^"([^"$]+)"$|^'([^'$]+)'$"#).captures(value) {
            return Ok(captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|m| m.as_str().to_owned()));
        }
        if regex(r#"^(?:project\.)?version(?:\.toString\(\))?$|^"\$\{?version\}?"$"#)
            .is_match(value)
        {
            return Ok(
                literal("version", self.code).or_else(|| self.properties.get("version").cloned())
            );
        }
        if regex(r"^(?:project\.)?group(?:\.toString\(\))?$").is_match(value) {
            return Ok(
                literal("group", self.code).or_else(|| self.properties.get("group").cloned())
            );
        }
        if let Some(key) = capture(r"^libs\.versions\.([\w.]+)\.get\(\)$", value) {
            let toml =
                read_sibling(self.directory, "gradle/libs.versions.toml")?.unwrap_or_default();
            if let Some(found) = catalog_version(&toml, &key) {
                return Ok(Some(found));
            }
        }
        if let Some(name) = capture(
            r#"^(?:providers\.gradleProperty|property|findProperty)\(\s*"([\w.-]+)"\s*\)(?:\.get\(\))?$"#,
            value,
        ) {
            if let Some(found) = self.properties.get(&name).filter(|v| !v.is_empty()) {
                return Ok(Some(found.clone()));
            }
        }
        self.problems.push(format!(
            "dynamic Gradle {role} expression {value}; list the reviewed value under \"gradle\" in .package-registry-manager.json"
        ));
        Ok(None)
    }
}

fn skip(manifest: String, reason: &str) -> Package {
    let mut item = Package::new(Registry::MavenCentral, String::new(), None, manifest)
        .unpublishable("unresolved Gradle Maven publication");
    item.audit.insert("discovery_skip".into(), json!(reason));
    item
}

/// Builds the Maven Central record for one Gradle build script.
pub fn gradle_distribution(
    contents: &str,
    manifest: String,
    manifest_path: &Path,
) -> Result<Package> {
    let code = strip_gradle_comments(contents);
    let directory = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let properties =
        parse_properties(&read_sibling(directory, "gradle.properties")?.unwrap_or_default());
    let settings = match read_sibling(directory, "settings.gradle.kts")? {
        Some(text) => text,
        None => read_sibling(directory, "settings.gradle")?.unwrap_or_default(),
    };
    let root_name = literal(r"rootProject\.name", &strip_gradle_comments(&settings));
    let vanniktech = code.contains("com.vanniktech.maven.publish")
        && regex(r"\bmavenPublishing\s*\{").is_match(&code);
    if !vanniktech {
        return Ok(classic_gradle(&code, manifest, root_name));
    }
    let coordinates = regex(
        r"\bcoordinates\s*\(\s*([^,()]+(?:\([^)]*\))?)\s*,\s*([^,()]+(?:\([^)]*\))?)\s*,\s*([^()]+?(?:\([^)]*\))?)\s*\)",
    )
    .captures(&code)
    .map(|captures| [1, 2, 3].map(|index| captures[index].to_owned()));
    let mut context = Context {
        code: &code,
        directory,
        properties,
        problems: Vec::new(),
    };
    let part = |index: usize| coordinates.as_ref().map(|values| values[index].clone());
    let group = context
        .resolve(part(0).as_deref(), "group")?
        .or_else(|| literal("group", &code))
        .or_else(|| context.properties.get("GROUP").cloned());
    let artifact = match context.resolve(part(1).as_deref(), "artifact")? {
        Some(artifact) => Some(artifact),
        None => context
            .properties
            .get("POM_ARTIFACT_ID")
            .cloned()
            .or(root_name),
    };
    let version = match context.resolve(part(2).as_deref(), "version")? {
        Some(version) => Some(version),
        None if coordinates.is_some() => None,
        None => literal("version", &code)
            .or_else(|| context.properties.get("VERSION_NAME").cloned())
            .or_else(|| context.properties.get("version").cloned()),
    };
    let problems = context.problems;
    let (Some(group), Some(artifact)) = (group, artifact) else {
        let detail = if problems.is_empty() {
            String::new()
        } else {
            format!(": {}", problems.join("; "))
        };
        return Ok(skip(manifest, &format!("{SKIP_REASON}{detail}")));
    };
    let multiplatform =
        regex(r#"kotlin\s*\(\s*["']multiplatform["']\s*\)|org\.jetbrains\.kotlin\.multiplatform"#)
            .is_match(&code);
    let wrapper = if directory.join("gradlew").is_file() {
        "./gradlew"
    } else {
        "gradle"
    };
    let task = if regex(r"publishToMavenCentral\s*\(").is_match(&code) {
        "publishToMavenCentral"
    } else {
        "publishAllPublicationsToMavenCentralRepository"
    };
    let working_directory = manifest
        .rsplit_once('/')
        .map_or(".", |(directory, _)| directory)
        .to_owned();
    let mut gradle = json!({
        "plugin": "com.vanniktech.maven.publish",
        "wrapper": wrapper,
        "working_directory": working_directory,
        "publish_task": task,
        "automatic_release": regex(r"automaticRelease\s*=\s*true").is_match(&code),
        "signing": regex(r"signAllPublications\s*\(").is_match(&code),
        "multiplatform": multiplatform,
    });
    let mut item = Package::new(
        Registry::MavenCentral,
        format!("{group}:{artifact}"),
        version,
        manifest,
    );
    item.audit.insert("build_tool".into(), json!("gradle"));
    if multiplatform {
        let variants = kmp_variants(&code, &group, &artifact);
        gradle["requirements"] = gradle_requirements(&variants);
        item.audit.insert("variants".into(), Value::Array(variants));
    }
    item.audit.insert("gradle".into(), gradle);
    item.warnings = problems;
    Ok(item)
}

fn classic_gradle(code: &str, manifest: String, root_name: Option<String>) -> Package {
    let group = literal("groupId", code).or_else(|| literal("group", code));
    let artifact = literal("artifactId", code)
        .or_else(|| literal("archivesBaseName", code))
        .or(root_name);
    let version = literal("version", code);
    let publish = regex(r"maven-publish|MavenPublication").is_match(code);
    let input = regex(r"from\s*\(?\s*components|artifact\s*\(").is_match(code);
    match (group, artifact, version) {
        (Some(group), Some(artifact), Some(version)) if publish && input => {
            let mut item = Package::new(
                Registry::MavenCentral,
                format!("{group}:{artifact}"),
                Some(version),
                manifest,
            );
            item.audit.insert("build_tool".into(), json!("gradle"));
            item
        }
        _ => skip(manifest, SKIP_REASON),
    }
}

/// The body of the top-level `kotlin { ... }` block.
fn kotlin_block(code: &str) -> &str {
    let Some(start) = regex(r"(?:^|\n)\s*kotlin\s*\{").find(code) else {
        return "";
    };
    let open = start.end() - 1;
    let mut depth = 0_usize;
    for (index, byte) in code.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open + 1..index];
                }
            }
            _ => {}
        }
    }
    &code[open + 1..]
}

/// Whether the byte position sits inside an `if (...) { ... }` block.
fn inside_condition(text: &str, position: usize) -> bool {
    let condition = regex(r"\bif\s*\([^{}]*\)\s*$");
    let mut stack = Vec::new();
    for (index, byte) in text.bytes().enumerate().take(position) {
        match byte {
            b'{' => {
                let mut start = index.saturating_sub(200);
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                stack.push(condition.is_match(&text[start..index]));
            }
            b'}' => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.into_iter().any(|conditional| conditional)
}

/// The platform publications a Kotlin Multiplatform build declares, each
/// marked when it only exists behind a condition or needs a macOS host.
#[must_use]
pub fn kmp_variants(code: &str, group: &str, artifact: &str) -> Vec<Value> {
    let block = kotlin_block(code);
    let names = KMP_TARGETS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join("|");
    let expression = regex(&format!(
        r#"(?:^|[^\w.])({names})\s*(?:\(\s*(?:"[^"]*")?\s*\)|\{{)|KotlinMultiplatformAndroidLibraryTarget>\s*\(\s*"(android)""#
    ));
    let mut variants: Vec<Value> = Vec::new();
    for captures in expression.captures_iter(block) {
        let Some(found) = captures.get(1).or_else(|| captures.get(2)) else {
            continue;
        };
        let target = found.as_str();
        let Some((_, suffix)) = KMP_TARGETS.iter().find(|(name, _)| *name == target) else {
            continue;
        };
        if variants.iter().any(|item| item["suffix"] == *suffix) {
            continue;
        }
        let mut variant = json!({
            "target": if target == "androidTarget" { "android" } else { target },
            "suffix": suffix,
            "coordinates": format!("{group}:{artifact}-{suffix}"),
        });
        if regex(r"^(?:ios|macos|tvos|watchos)").is_match(target) {
            variant["host"] = json!("macos");
        }
        if target.starts_with("android") && !target.starts_with("androidNative") {
            variant["requires"] = json!("Android SDK");
        }
        if inside_condition(block, found.start()) {
            variant["conditional"] = json!(true);
        }
        variants.push(variant);
    }
    variants
}

fn gradle_requirements(variants: &[Value]) -> Value {
    let mut requirements = Vec::new();
    let apple = variants
        .iter()
        .filter(|item| item["host"] == "macos")
        .map(|item| item["target"].clone())
        .collect::<Vec<_>>();
    if !apple.is_empty() {
        requirements.push(json!({
            "kind": "runner",
            "requirement": "macOS",
            "reason": "Kotlin/Native publishes Apple targets only from a macOS host; a Linux runner silently omits them",
            "targets": apple,
        }));
    }
    if variants
        .iter()
        .any(|item| item["requires"] == "Android SDK")
    {
        requirements.push(json!({
            "kind": "sdk",
            "requirement": "Android SDK (ANDROID_HOME or ANDROID_SDK_ROOT)",
            "reason": "the Android publication exists only when the build finds an SDK; without it the release lacks the -android artifact",
            "targets": ["android"],
        }));
    }
    requirements.push(json!({
        "kind": "toolchain",
        "requirement": "JDK and the Gradle wrapper",
        "reason": "the wrapper pins the Gradle version that builds every variant",
    }));
    Value::Array(requirements)
}

/// Local validation and toolchain steps for a Gradle-built Central package.
#[must_use]
pub fn gradle_validation_steps(item: &Package) -> Vec<SetupStep> {
    let gradle = item
        .audit
        .get("gradle")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let wrapper = gradle["wrapper"].as_str().unwrap_or("./gradlew");
    let mut steps = vec![SetupStep::new(
        "validate-package",
        "Validate the Gradle publication without uploading",
        StepKind::Check,
        format!(
            "Run the official Gradle check and local publication tasks in {}; nothing reaches Central.",
            gradle["working_directory"].as_str().unwrap_or("the build directory")
        ),
    )
    .command(wrapper, &["--no-daemon", "--console=plain", "check", "publishToMavenLocal"])];
    if let Some(requirements) = gradle["requirements"].as_array().filter(|r| !r.is_empty()) {
        let reasons = requirements
            .iter()
            .map(|entry| {
                format!(
                    "{}: {}",
                    entry["requirement"].as_str().unwrap_or_default(),
                    entry["reason"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(". ");
        steps.push(
            SetupStep::new(
                "verify-gradle-toolchain",
                "Confirm the build hosts and SDKs for every variant",
                StepKind::Manual,
                format!("{reasons}. A missing runner, SDK or toolchain is separate from Central authentication."),
            )
            .detail("state", json!("manual-prerequisite"))
            .detail("requirements", Value::Array(requirements.clone())),
        );
    }
    steps
}

/// Exact Central read-back: the root POM, the Gradle module metadata and
/// every platform publication it references.
#[must_use]
pub fn central_publication_step(item: &Package) -> SetupStep {
    let (group, artifact) = item
        .name
        .split_once(':')
        .unwrap_or((item.name.as_str(), ""));
    let variants = item
        .audit
        .get("variants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let gradle = item.audit.get("gradle");
    let multiplatform = gradle.is_some_and(|gradle| gradle["multiplatform"] == true);
    let suffixes = variants
        .iter()
        .filter_map(|entry| entry["suffix"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    SetupStep::new(
        "verify-central-publication",
        &format!(
            "Verify {}:{}{} on Central",
            item.name,
            item.version.as_deref().unwrap_or("(unresolved)"),
            if variants.is_empty() { String::new() } else { format!(" and its {} platform publications", variants.len()) }
        ),
        StepKind::Api,
        format!(
            "Read the exact root POM and {} from repo1.maven.org, then every platform coordinate the metadata references ({}). Missing module metadata or a missing variant keeps the release partial; a skipped or unlogged {} job is never publication. Published versions are immutable and never re-uploaded.",
            if multiplatform { "Gradle module metadata (.module)" } else { "artifact" },
            if suffixes.is_empty() { "none" } else { &suffixes },
            gradle.and_then(|gradle| gradle["publish_task"].as_str()).unwrap_or("publish"),
        ),
    )
    .detail("coordinates", json!({"group": group, "artifact": artifact, "version": item.version}))
    .detail("variants", json!(variants.iter().map(|entry| entry["coordinates"].clone()).collect::<Vec<_>>()))
    .url(format!("https://central.sonatype.com/artifact/{group}/{artifact}"))
}
