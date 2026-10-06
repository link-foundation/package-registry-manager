//! Names the user's default browser, so the tool can say where a link opened.

use anyhow::{bail, Result};
use browser_commander::browser_sources;
use command_stream::StreamingRunner;
use regex::Regex;

use crate::auth_urls::resolve_program;
use crate::browser_catalogue::catalogue_name;
use crate::model::CommandSpec;

fn spec(program: &str, args: &[&str]) -> CommandSpec {
    CommandSpec {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

/// The command that reports the default browser on `os`
/// ([`std::env::consts::OS`]), as an exact argument vector.
#[must_use]
pub fn default_browser_query(os: &str) -> CommandSpec {
    match os {
        "macos" => spec(
            "defaults",
            &[
                "read",
                "com.apple.LaunchServices/com.apple.launchservices.secure",
                "LSHandlers",
            ],
        ),
        "windows" => spec(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
                "/v",
                "ProgId",
            ],
        ),
        _ => spec("xdg-settings", &["get", "default-web-browser"]),
    }
}

/// The top-level dictionaries of macOS's `LSHandlers` array, without their
/// nested dictionaries such as `LSHandlerPreferredVersions`.
fn handler_entries(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut depth = 0_usize;
    let mut entry = String::new();
    for character in text.chars() {
        match character {
            '{' => {
                depth += 1;
                if depth == 1 {
                    entry.clear();
                }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    entries.push(std::mem::take(&mut entry));
                }
            }
            _ if depth == 1 => entry.push(character),
            _ => {}
        }
    }
    entries
}

/// Extracts the browser id from the query's output: the https handler's
/// bundle id on macOS (Safari when none is set), the desktop entry on Linux,
/// and the `ProgId` on Windows.
#[must_use]
pub fn default_browser_id(output: &str, os: &str) -> Option<String> {
    let compile = |pattern: &str| Regex::new(pattern).expect("static pattern must compile");
    match os {
        "macos" => {
            let role = compile(r#"LSHandlerRoleAll\s*=\s*"?([\w.-]+)"?\s*;"#);
            let entries = handler_entries(output);
            for scheme in ["https", "http"] {
                let handles = compile(&format!(r#"LSHandlerURLScheme\s*=\s*"?{scheme}"?\s*;"#));
                let found = entries
                    .iter()
                    .filter(|entry| handles.is_match(entry))
                    .find_map(|entry| role.captures(entry));
                if let Some(captures) = found {
                    return Some(captures[1].to_owned());
                }
            }
            Some("com.apple.safari".to_owned())
        }
        "windows" => compile(r"(?i)\bProgId\s+REG_SZ\s+(\S+)")
            .captures(output)
            .map(|captures| captures[1].to_owned()),
        _ => Some(output.trim().to_owned()).filter(|id| !id.is_empty()),
    }
}

/// A readable browser name for an id, or `None` when it is unknown.
#[must_use]
pub fn browser_name(id: &str) -> Option<&'static str> {
    let needle = id.trim().to_lowercase();
    let sources = browser_sources();
    let known = sources
        .iter()
        .find(|source| {
            source
                .default
                .values()
                .flatten()
                .any(|candidate| needle == candidate.to_lowercase())
        })
        .or_else(|| {
            sources.iter().find(|source| {
                source
                    .default
                    .get("win32")
                    .into_iter()
                    .flatten()
                    .any(|candidate| needle.starts_with(&candidate.to_lowercase()))
                    || (needle.ends_with(".desktop")
                        && source.executable_names.iter().any(|name| {
                            ['-', '_', '.'].iter().any(|separator| {
                                needle.starts_with(&format!("{}{separator}", name.to_lowercase()))
                            })
                        }))
            })
        });
    if let Some(source) = known {
        return Some(catalogue_name(&source.id));
    }
    // Preserve legacy ProgIds absent from the upstream catalogue.
    if needle.starts_with("safariurl") {
        Some(catalogue_name("safari"))
    } else if needle.starts_with("chromiumhtm") {
        Some(catalogue_name("chromium"))
    } else if needle.starts_with("ie.http") {
        Some("Internet Explorer")
    } else {
        None
    }
}

/// Names the browser from the query's exit code and output on `os`.
///
/// Without any handler entry, macOS answers with an error and uses Safari.
#[must_use]
pub fn browser_from_query(code: i32, output: &str, os: &str) -> Option<&'static str> {
    if code != 0 && os != "macos" {
        return None;
    }
    let output = if code == 0 { output } else { "" };
    default_browser_id(output, os).and_then(|id| browser_name(&id))
}

/// Names the user's default browser, or `None` when the platform does not say.
pub async fn detect_default_browser(verbose: bool) -> Option<&'static str> {
    let os = std::env::consts::OS;
    let query = default_browser_query(os);
    match StreamingRunner::from_argv(resolve_program(&query.program), &query.args)
        .collect()
        .await
    {
        Ok(result) => browser_from_query(result.code, result.stdout.as_ref(), os),
        Err(error) => {
            if verbose {
                eprintln!("could not detect the default browser: {error:#}");
            }
            None
        }
    }
}

/// The exact argument vector that opens `url` in a chosen application:
/// `open -a <app>` on macOS, and the application itself elsewhere.
///
/// # Errors
/// Fails for a non-web URL.
pub fn open_with_command(url: &str, app: &str, os: &str) -> Result<CommandSpec> {
    let web = Regex::new(r"(?i)^https?://\S+$").expect("static pattern must compile");
    if !web.is_match(url) {
        bail!("refusing to open a non-web URL: {url}");
    }
    Ok(if os == "macos" {
        spec("open", &["-a", app, url])
    } else {
        spec(app, &[url])
    })
}
