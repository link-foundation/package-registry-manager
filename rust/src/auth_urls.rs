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

use crate::approvals::{LinkKind, EXPIRED_APPROVAL};
use crate::model::CommandSpec;

/// How much of npm's stderr is kept to recognize an expired approval.
const STDERR_TAIL: usize = 4096;

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
    expect_url: Option<LinkKind>,
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
            expect_url: None,
            legacy_login: false,
            seen: BTreeSet::new(),
            prompt: compile(r"(?i)^(Login at|Authenticate your account at):?$"),
            inline: compile(r"(?i)(Login at|Authenticate your account at):?\s+(https?://\S+)"),
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
        self.push_links(chunk)
            .into_iter()
            .map(|(url, _)| url)
            .collect()
    }

    /// Feed a chunk of output; returns links not reported before, each with
    /// whether it signs in or approves.
    pub fn push_links(&mut self, chunk: &str) -> Vec<(String, LinkKind)> {
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
            if let Some((url, kind)) = self.scan_line(&raw) {
                if self.seen.insert(url.clone()) {
                    found.push((url, kind));
                }
            }
        }
        let pending = self.pending.clone();
        self.detect_legacy(&pending);
        found
    }

    fn scan_line(&mut self, raw: &str) -> Option<(String, LinkKind)> {
        let cleaned = self.ansi.replace_all(raw, "");
        let line = cleaned.trim();
        if let Some(captures) = self.inline.captures(line) {
            self.expect_url = None;
            return Some((captures[2].to_owned(), link_kind(&captures[1])));
        }
        if let Some(captures) = self.prompt.captures(line) {
            self.expect_url = Some(link_kind(&captures[1]));
        } else if let Some(kind) = self.expect_url.filter(|_| self.url.is_match(line)) {
            self.expect_url = None;
            return Some((line.to_owned(), kind));
        } else if !line.is_empty() {
            self.expect_url = None;
        }
        None
    }
}

fn link_kind(label: &str) -> LinkKind {
    if label.to_ascii_lowercase().starts_with("login") {
        LinkKind::Login
    } else {
        LinkKind::Approve
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
    /// The watched stderr reported that npm's approval session expired.
    pub approval_expired: bool,
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
/// prompts (`cargo login`, `gh secret set`) keep working. This is the one
/// subprocess not run through command-stream, whose argument-vector runner
/// only pipes or closes stdin. Web-authentication
/// links are sent to `urls`. With `stop_on_legacy_login` the command is killed
/// at npm's legacy `Username:` prompt and the output has `legacy_login` set.
/// With `watch_stderr` stderr is piped, mirrored, and checked for npm's
/// expired-approval error, which sets `approval_expired`.
pub async fn run_interactive(
    command: &CommandSpec,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    mirror: bool,
    urls: Option<UnboundedSender<(String, LinkKind)>>,
    stop_on_legacy_login: bool,
    watch_stderr: bool,
) -> Result<CommandOutput> {
    let mut child = tokio::process::Command::new(resolve_program(&command.program))
        .args(&command.args)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(if watch_stderr {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .with_context(|| format!("failed to run {}", command.program))?;
    let stderr = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(watch(stderr)));
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
        let found = scanner.push_links(&String::from_utf8_lossy(chunk));
        if let Some(sender) = &urls {
            for link in found {
                let _ = sender.send(link);
            }
        }
        if stop_on_legacy_login && scanner.legacy_login() {
            child.start_kill()?;
            break;
        }
    }
    if let Some(sender) = &urls {
        for link in scanner.push_links("\n") {
            let _ = sender.send(link);
        }
    }
    let status = child.wait().await?;
    let stderr = match stderr {
        Some(task) => task.await??,
        None => String::new(),
    };
    let expired = Regex::new(EXPIRED_APPROVAL).expect("static pattern must compile");
    Ok(CommandOutput {
        code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&captured).into_owned(),
        approval_expired: expired.is_match(&stderr),
        stderr,
        legacy_login: stop_on_legacy_login && scanner.legacy_login(),
    })
}

/// Mirrors a piped stderr to the terminal and returns its last
/// [`STDERR_TAIL`] bytes without ANSI escape sequences.
async fn watch(mut stderr: tokio::process::ChildStderr) -> Result<String> {
    let mut tail = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stderr.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let mut terminal = std::io::stderr().lock();
        terminal.write_all(&buffer[..read])?;
        terminal.flush()?;
        tail.extend_from_slice(&buffer[..read]);
        let excess = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..excess);
    }
    let ansi = Regex::new(ANSI_PATTERN).expect("static pattern must compile");
    Ok(ansi
        .replace_all(&String::from_utf8_lossy(&tail), "")
        .into_owned())
}
