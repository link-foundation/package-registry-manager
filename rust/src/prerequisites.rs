//! Manual prerequisites listed before a plan's steps: the Node.js and npm on
//! `PATH`, the npm that runs `npm trust`, the npm account's 2FA state, the
//! GitHub CLI's scopes, and where browser pages open.

use command_stream::StreamingRunner;
use serde_json::Value;

use crate::auth_urls::resolve_program;
use crate::model::{Prerequisite, Registry, SetupPlan};
use crate::registry_state::{Endpoints, Lookup, RegistryClient};

/// The Node.js versions npm 11 runs on.
pub const NPM_11_ENGINES: &str = "^20.17.0 || >=22.9.0";
/// The Node.js versions npm 12 runs on.
pub const NPM_12_ENGINES: &str = "^22.22.2 || ^24.15.0 || >=26.0.0";
/// Where an npm account turns on two-factor authentication.
pub const NPM_TFA_URL: &str = "https://docs.npmjs.com/configuring-two-factor-authentication/";
/// The npm that runs `npm trust` when Node.js cannot run npm 12.
pub const DEFAULT_TRUST_NPM: &str = "npm@^11.10";

/// Parses `v24.15.0` or `24.15.0` into numbers.
#[must_use]
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.splitn(3, '.');
    let mut version = [0; 3];
    for (index, slot) in version.iter_mut().enumerate() {
        let part = parts.next()?;
        let digits = if index == 2 {
            let end = part
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(part.len());
            &part[..end]
        } else {
            part
        };
        *slot = digits.parse().ok()?;
    }
    Some(version)
}

/// Whether a Node.js version satisfies npm 11's engines.
#[must_use]
pub fn supports_npm11(node_version: &str) -> bool {
    parse_version(node_version).is_some_and(|version| {
        (version[0] == 20 && version >= [20, 17, 0]) || version >= [22, 9, 0]
    })
}

/// Whether a Node.js version satisfies npm 12's engines.
#[must_use]
pub fn supports_npm12(node_version: &str) -> bool {
    parse_version(node_version).is_some_and(|version| {
        (version[0] == 22 && version >= [22, 22, 2])
            || (version[0] == 24 && version >= [24, 15, 0])
            || version[0] >= 26
    })
}

/// The npm package spec that runs `npm trust` through npx: npm 12 only when the
/// Node.js on `PATH` satisfies its engines, otherwise npm 11.10 or newer.
#[must_use]
pub fn trust_npm_spec(node_version: Option<&str>) -> &'static str {
    if node_version.is_some_and(supports_npm12) {
        "npm@^12"
    } else {
        DEFAULT_TRUST_NPM
    }
}

/// The version a `npm@^N` spec resolves to, from the registry's `next-N`
/// dist-tag (or `latest` when it has the same major version).
#[must_use]
pub fn resolve_trust_npm(spec: &str, dist_tags: Option<&Value>) -> Option<String> {
    let major: u64 = spec
        .split_once('^')?
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    let tags = dist_tags?;
    [&tags[format!("next-{major}")], &tags["latest"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|version| parse_version(version).is_some_and(|parsed| parsed[0] == major))
        .map(str::to_owned)
}

/// Reads `tfa` from `npm profile get --json`: the 2FA mode, or `None` when
/// two-factor authentication is off or still pending.
///
/// # Errors
/// Fails when the output is not JSON.
pub fn two_factor_mode(output: &str) -> serde_json::Result<Option<String>> {
    let document: Value = serde_json::from_str(output)?;
    let tfa = &document["tfa"];
    if !tfa.is_object() || truthy(&tfa["pending"]) || !truthy(&tfa["mode"]) {
        return Ok(None);
    }
    Ok(Some(match &tfa["mode"] {
        Value::String(mode) => mode.clone(),
        other => other.to_string(),
    }))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The signed-in account from `gh auth status --json hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubAuth {
    pub login: Option<String>,
    pub scopes: Vec<String>,
}

/// Summarizes `gh auth status --json hosts`, or returns `None`.
#[must_use]
pub fn github_auth(output: &str) -> Option<GithubAuth> {
    let document: Value = serde_json::from_str(output).ok()?;
    let hosts = document["hosts"].as_object().cloned().unwrap_or_default();
    let entries =
        |value: Option<&Value>| value.and_then(Value::as_array).cloned().unwrap_or_default();
    let accounts = entries(hosts.get("github.com")).into_iter().chain(
        hosts
            .iter()
            .filter(|(host, _)| host.as_str() != "github.com")
            .flat_map(|(_, value)| entries(Some(value))),
    );
    let account = accounts
        .into_iter()
        .find(|entry| entry["active"] == Value::Bool(true) && entry["state"] == "success");
    let Some(account) = account else {
        return Some(GithubAuth {
            login: None,
            scopes: Vec::new(),
        });
    };
    Some(GithubAuth {
        login: account["login"].as_str().map(str::to_owned),
        scopes: account["scopes"]
            .as_str()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

/// The npm account's 2FA state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwoFactor {
    Unchecked,
    Unknown,
    Off,
    Enabled(String),
}

/// The GitHub CLI's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Github {
    Unchecked,
    Missing,
    Unknown,
    SignedOut,
    SignedIn { login: String, scopes: Vec<String> },
}

/// The local tools the setup flows depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub offline: bool,
    pub node: Option<String>,
    pub npm: Option<String>,
    pub trust_npm: String,
    pub trust_npm_version: Option<String>,
    pub two_factor: TwoFactor,
    pub github: Github,
}

/// Where browser pages open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BrowserSummary {
    /// The user's default browser; forms open in the automated profile.
    #[default]
    Default,
    /// Everything opens in the automated profile.
    Automated,
    /// URLs are printed (`--no-browser`).
    None,
}

/// The browser summary shown with the prerequisites.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserDisplay {
    pub mode: BrowserSummary,
    pub channel: String,
    pub profile: Option<String>,
}

struct Probe {
    code: i32,
    stdout: String,
}

async fn probe(program: &str, args: &[&str], verbose: bool) -> Probe {
    if verbose {
        eprintln!("+ {program} {}", args.join(" "));
    }
    // StreamingRunner gives the child no stdin, so a probe can never prompt.
    match StreamingRunner::from_argv(resolve_program(program), args)
        .collect()
        .await
    {
        Ok(result) => Probe {
            code: result.code,
            stdout: result.stdout.to_string(),
        },
        Err(_) => Probe {
            code: 127,
            stdout: String::new(),
        },
    }
}

/// Looks up the local tools the setup flows depend on.
///
/// These are the Node.js and npm on `PATH`, the npm that runs `npm trust`, the
/// npm account's 2FA state, and the GitHub CLI's scopes. Network lookups are
/// skipped with `offline`.
pub async fn probe_environment(
    offline: bool,
    verbose: bool,
    npm: bool,
    endpoints: &Endpoints,
) -> Environment {
    let optional = |run: bool, program: &'static str, args: &'static [&'static str]| async move {
        if run {
            Some(probe(program, args, verbose).await)
        } else {
            None
        }
    };
    let (node, npm_version, profile, gh) = tokio::join!(
        probe("node", &["--version"], verbose),
        optional(npm, "npm", &["--version"]),
        optional(npm && !offline, "npm", &["profile", "get", "--json"]),
        optional(!offline, "gh", &["auth", "status", "--json", "hosts"]),
    );
    let node = (node.code == 0).then(|| node.stdout.trim().to_owned());
    let spec = trust_npm_spec(node.as_deref());
    let dist_tags = if npm && !offline {
        let base = endpoints.base(Registry::Npm).unwrap_or_default();
        match RegistryClient::new(endpoints.clone(), verbose)
            .get_json(&format!("{base}/-/package/npm/dist-tags"))
            .await
        {
            Lookup::Found(document) => Some(document),
            _ => None,
        }
    } else {
        None
    };
    Environment {
        offline,
        node,
        npm: npm_version
            .filter(|result| result.code == 0)
            .map(|result| result.stdout.trim().to_owned()),
        trust_npm: spec.to_owned(),
        trust_npm_version: resolve_trust_npm(spec, dist_tags.as_ref()),
        two_factor: two_factor_state(profile),
        github: github_state(gh),
    }
}

fn two_factor_state(result: Option<Probe>) -> TwoFactor {
    let Some(result) = result else {
        return TwoFactor::Unchecked;
    };
    if result.code != 0 {
        return TwoFactor::Unknown;
    }
    match two_factor_mode(&result.stdout) {
        Ok(Some(mode)) => TwoFactor::Enabled(mode),
        Ok(None) => TwoFactor::Off,
        Err(_) => TwoFactor::Unknown,
    }
}

fn github_state(result: Option<Probe>) -> Github {
    let Some(result) = result else {
        return Github::Unchecked;
    };
    if result.code == 127 {
        return Github::Missing;
    }
    match github_auth(&result.stdout) {
        None => Github::Unknown,
        Some(GithubAuth {
            login: Some(login),
            scopes,
        }) => Github::SignedIn { login, scopes },
        Some(_) => Github::SignedOut,
    }
}

fn prerequisite(
    id: &str,
    title: &str,
    detected: String,
    required: String,
    ok: Option<bool>,
) -> Prerequisite {
    Prerequisite {
        id: id.to_owned(),
        title: title.to_owned(),
        detected,
        required,
        ok,
    }
}

/// The manual prerequisites of a plan, listed before its steps: tool versions,
/// the npm account's 2FA state, the GitHub CLI scopes, and where browser pages
/// open.
#[must_use]
pub fn plan_prerequisites(
    plan: &SetupPlan,
    environment: Option<&Environment>,
    browser: &BrowserDisplay,
) -> Vec<Prerequisite> {
    let Some(environment) = environment else {
        return Vec::new();
    };
    if plan.steps.is_empty() {
        return Vec::new();
    }
    let mut items = Vec::new();
    let unchecked = if environment.offline {
        "not checked (--offline)"
    } else {
        "not checked"
    };
    if plan.registry == Registry::Npm {
        let node_ok = environment.node.as_deref().is_some_and(supports_npm11);
        items.push(prerequisite(
            "node",
            "Node.js",
            environment
                .node
                .clone()
                .unwrap_or_else(|| "not found".to_owned()),
            NPM_11_ENGINES.to_owned(),
            Some(node_ok),
        ));
        items.push(prerequisite(
            "npm",
            "npm",
            environment
                .npm
                .clone()
                .unwrap_or_else(|| "not found".to_owned()),
            "installed".to_owned(),
            Some(environment.npm.is_some()),
        ));
        items.push(prerequisite(
            "npm-trust",
            "npm for npm trust",
            environment.trust_npm_version.as_ref().map_or_else(
                || environment.trust_npm.clone(),
                |version| format!("{}, resolves to {version}", environment.trust_npm),
            ),
            format!("npm 11.10 or newer; npm 12 only on Node.js {NPM_12_ENGINES}"),
            Some(node_ok),
        ));
        let (detected, ok) = match &environment.two_factor {
            TwoFactor::Enabled(mode) => (mode.clone(), Some(true)),
            TwoFactor::Off => ("off".to_owned(), Some(false)),
            TwoFactor::Unknown => ("unknown (sign in to npm first)".to_owned(), None),
            TwoFactor::Unchecked => (unchecked.to_owned(), None),
        };
        items.push(prerequisite(
            "npm-2fa",
            "npm two-factor authentication",
            detected,
            format!("enabled at {NPM_TFA_URL}; npm trust requires it"),
            ok,
        ));
    }
    if plan.steps.iter().any(|step| {
        step.command
            .as_ref()
            .is_some_and(|command| command.program == "gh")
    }) {
        let (detected, ok) = match &environment.github {
            Github::SignedIn { login, scopes } => {
                let listed = if scopes.is_empty() {
                    "none listed".to_owned()
                } else {
                    scopes.join(", ")
                };
                (
                    format!("signed in as {login} (scopes: {listed})"),
                    (!scopes.is_empty()).then(|| scopes.iter().any(|scope| scope == "repo")),
                )
            }
            Github::SignedOut => ("not signed in".to_owned(), Some(false)),
            Github::Missing => ("not installed".to_owned(), Some(false)),
            Github::Unknown => ("unknown".to_owned(), None),
            Github::Unchecked => (unchecked.to_owned(), None),
        };
        items.push(prerequisite(
            "gh",
            "GitHub CLI",
            detected,
            "signed in with the repo scope, for gh secret and gh run".to_owned(),
            ok,
        ));
    }
    items.push(prerequisite(
        "browser",
        "Browser",
        browser_description(browser),
        "signed in to the registry, or ready to sign in".to_owned(),
        None,
    ));
    items
}

fn browser_description(browser: &BrowserDisplay) -> String {
    let channel = if browser.channel.is_empty() {
        "chrome"
    } else {
        browser.channel.as_str()
    };
    match browser.mode {
        BrowserSummary::None => "none; URLs are printed (--no-browser)".to_owned(),
        BrowserSummary::Automated => {
            let location = browser
                .profile
                .as_ref()
                .map(|profile| format!(" at {profile}"))
                .unwrap_or_default();
            format!("the automated {channel} profile{location}")
        }
        BrowserSummary::Default => {
            format!("your default browser; forms open in the automated {channel} profile")
        }
    }
}

/// Renders prerequisites as indented text lines.
#[must_use]
pub fn render_prerequisites(items: &[Prerequisite]) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    std::iter::once("  prerequisites:".to_owned())
        .chain(items.iter().map(|item| {
            let action = if item.ok == Some(false) {
                " [action needed]"
            } else {
                ""
            };
            format!(
                "    - {}: {}; needs {}{action}",
                item.title, item.detected, item.required
            )
        }))
        .collect()
}
