//! Interactive commands whose web-authentication URLs open in a browser.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::CommandSpec;

/// Preloaded into npm so it treats piped stdout as a terminal.
///
/// npm only offers web authentication (`Authenticate your account at:`) on a TTY; the tool
/// pipes stdout to find those URLs and open them in a browser.
pub const TTY_SHIM: &str = r#"for (const stream of [process.stdin, process.stdout]) {
  if (!stream.isTTY) {
    Object.defineProperty(stream, "isTTY", { value: true, configurable: true });
  }
}
"#;

/// Matches ANSI escape sequences, such as colors, in command output.
pub const ANSI_PATTERN: &str = r"\x1b\[[0-9;?]*[ -/]*[@-~]";

/// Finds web-authentication URLs in streamed npm output, and npm's legacy
/// `Username:` prompt, which npm prints without a trailing newline when a web
/// login is not completed in time.
#[derive(Debug)]
pub struct AuthUrlScanner {
    pending: String,
    expect_url: bool,
    legacy_login: bool,
    seen: BTreeSet<String>,
    prompt: Regex,
    inline: Regex,
    ansi: Regex,
    url: Regex,
    legacy: Regex,
}

impl Default for AuthUrlScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthUrlScanner {
    /// Create a scanner with no buffered output.
    #[must_use]
    pub fn new() -> Self {
        let compile = |pattern: &str| Regex::new(pattern).expect("static pattern must compile");
        Self {
            pending: String::new(),
            expect_url: false,
            legacy_login: false,
            seen: BTreeSet::new(),
            prompt: compile(r"(?i)^(?:Login at|Authenticate your account at):?$"),
            inline: compile(r"(?i)(?:Login at|Authenticate your account at):?\s+(https?://\S+)"),
            ansi: compile(ANSI_PATTERN),
            url: compile(r"^https?://\S+$"),
            legacy: compile(r"(?i)^Username:"),
        }
    }

    /// Whether npm printed its legacy `Username:` prompt.
    #[must_use]
    pub const fn legacy_login(&self) -> bool {
        self.legacy_login
    }

    fn detect_legacy(&mut self, raw: &str) {
        let cleaned = self.ansi.replace_all(raw, "");
        if self.legacy.is_match(cleaned.trim()) {
            self.legacy_login = true;
        }
    }

    /// Feed a chunk of output; returns URLs not reported before.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.pending.push_str(chunk);
        let mut lines = self
            .pending
            .split('\n')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        self.pending = lines.pop().unwrap_or_default();
        let mut found = Vec::new();
        for raw in lines {
            self.detect_legacy(&raw);
            if let Some(url) = self.scan_line(&raw) {
                if self.seen.insert(url.clone()) {
                    found.push(url);
                }
            }
        }
        let pending = self.pending.clone();
        self.detect_legacy(&pending);
        found
    }

    fn scan_line(&mut self, raw: &str) -> Option<String> {
        let cleaned = self.ansi.replace_all(raw, "");
        let line = cleaned.trim();
        if let Some(captures) = self.inline.captures(line) {
            self.expect_url = false;
            return Some(captures[1].to_owned());
        }
        if self.prompt.is_match(line) {
            self.expect_url = true;
        } else if self.expect_url && self.url.is_match(line) {
            self.expect_url = false;
            return Some(line.to_owned());
        } else if !line.is_empty() {
            self.expect_url = false;
        }
        None
    }
}

/// Append a `--require` of the TTY shim to existing `NODE_OPTIONS`.
#[must_use]
pub fn node_options_with_shim(existing: Option<&str>, shim_path: &str) -> String {
    let quoted = shim_path.replace('\\', "\\\\").replace('"', "\\\"");
    let require = format!("--require \"{quoted}\"");
    match existing.filter(|value| !value.is_empty()) {
        Some(existing) => format!("{existing} {require}"),
        None => require,
    }
}

/// Write the TTY shim to a private temporary directory.
pub fn write_tty_shim() -> Result<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::Builder::new().prefix("prm-shim-").tempdir()?;
    let shim = directory.path().join("tty-shim.cjs");
    let mut file = std::fs::File::create(&shim)?;
    file.write_all(TTY_SHIM.as_bytes())?;
    Ok((directory, shim))
}

/// Exit status and captured stdout of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code; 1 when the process was killed by a signal.
    pub code: i32,
    /// Everything the command wrote to stdout.
    pub stdout: String,
    /// What the command wrote to stderr, when it was captured.
    pub stderr: String,
    /// The command was stopped at npm's legacy `Username:` prompt.
    pub legacy_login: bool,
}

/// Resolve a program on `PATH`, including Windows `PATHEXT` launchers such as
/// `npm.cmd`, so the exact argument vector runs without a shell.
#[must_use]
pub fn resolve_program(program: &str) -> OsString {
    if !cfg!(windows) || Path::new(program).extension().is_some() {
        return program.into();
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .flat_map(|directory| {
            extensions
                .split(';')
                .map(move |extension| directory.join(format!("{program}{extension}")))
                .collect::<Vec<_>>()
        })
        .find(|candidate| candidate.is_file())
        .map_or_else(|| program.into(), PathBuf::into_os_string)
}

/// Run an exact argument vector with the terminal's stdin and stderr.
///
/// Stdout is mirrored and captured. stdin stays a real terminal so masked
/// prompts (`cargo login`, `gh secret set`) keep working. Web-authentication
/// URLs are sent to `urls`. With `stop_on_legacy_login` the command is killed
/// at npm's legacy `Username:` prompt and the output has `legacy_login` set.
pub async fn run_interactive(
    command: &CommandSpec,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    mirror: bool,
    urls: Option<UnboundedSender<String>>,
    stop_on_legacy_login: bool,
) -> Result<CommandOutput> {
    let mut child = tokio::process::Command::new(resolve_program(&command.program))
        .args(&command.args)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to run {}", command.program))?;
    let mut stdout = child.stdout.take().context("stdout is not piped")?;
    let mut scanner = AuthUrlScanner::new();
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stdout.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        captured.extend_from_slice(chunk);
        if mirror {
            let mut terminal = std::io::stdout().lock();
            terminal.write_all(chunk)?;
            terminal.flush()?;
        }
        let found = scanner.push(&String::from_utf8_lossy(chunk));
        if let Some(sender) = &urls {
            for url in found {
                let _ = sender.send(url);
            }
        }
        if stop_on_legacy_login && scanner.legacy_login() {
            child.start_kill()?;
            break;
        }
    }
    if let Some(sender) = &urls {
        for url in scanner.push("\n") {
            let _ = sender.send(url);
        }
    }
    let status = child.wait().await?;
    Ok(CommandOutput {
        code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&captured).into_owned(),
        stderr: String::new(),
        legacy_login: stop_on_legacy_login && scanner.legacy_login(),
    })
}
