//! Shared Packagist engine. Node (already required for gh-manager) performs
//! encrypted GitHub operations; the Rust CLI owns Browser Commander and prompts.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{io::Write, process::Stdio};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, Command},
};

/// Byte-identical engine sources shipped in the npm and crate distributions.
#[must_use]
pub fn engine_sources() -> Value {
    json!({
        "page": include_str!("packagist-page.mjs"),
        "setup": include_str!("packagist-setup.mjs"),
        "publication": include_str!("packagist-publication.mjs"),
    })
}

/// Engine process and its nonsensitive temporary program. The file stays alive
/// until the subprocess finishes; no credential is written to it.
pub struct EngineProcess {
    /// Child process carrying only structured protocol output.
    pub child: Child,
    _program: tempfile::NamedTempFile,
}

/// Start the pinned installed gh-manager with the shared engine. No secret is
/// passed in argv or environment; browser replies travel over stdin only.
pub async fn start(input: &Value, cwd: &std::path::Path) -> Result<EngineProcess> {
    let driver = STANDARD.encode(include_str!("packagist-driver.mjs"));
    let script = format!("const {{runPackagistDriver}} = await import('data:text/javascript;base64,{driver}'); await runPackagistDriver({});", engine_sources());
    let mut program = tempfile::Builder::new()
        .prefix("prm-packagist-")
        .suffix(".mjs")
        .tempfile()?;
    program.write_all(script.as_bytes())?;
    program.flush()?;
    let mut child = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" })
        .args([
            "--yes",
            "--package",
            crate::github::GH_MANAGER_PACKAGE,
            "--",
            "node",
            program
                .path()
                .to_str()
                .context("invalid Packagist program path")?,
        ])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Node.js/npm are required for the Packagist engine")?;
    let stdin = child
        .stdin
        .as_mut()
        .context("Packagist engine stdin missing")?;
    stdin.write_all(format!("{input}\n").as_bytes()).await?;
    Ok(EngineProcess {
        child,
        _program: program,
    })
}

/// Read protocol replies, failing closed if the engine terminates or emits an
/// invalid document. No raw subprocess output is included in diagnostics.
pub async fn message(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> Result<Value> {
    let Some(line) = lines.next_line().await? else {
        bail!("Packagist engine ended before verification");
    };
    serde_json::from_str(&line)
        .map_err(|_| anyhow::anyhow!("invalid Packagist engine protocol (output withheld)"))
}
