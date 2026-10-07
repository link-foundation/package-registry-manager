//! Reusing a registry sign-in from an installed browser (#26).
//!
//! The automated profile stays fresh and clean by default; when a step needs
//! a sign-in it does not hold, the tool offers to import only the cookies of
//! the registry's sign-in domains from a browser that has them. Cookie values are never
//! printed, logged, or cached: they are only counted.

use anyhow::Result;
use browser_commander::{
    browser_ids, list_browser_profiles, read_browser_cookies, BrowserCookieReadOptions,
    BrowserProfile, BrowserProfileOptions,
};

use crate::browser_catalogue::catalogue_name;
use crate::model::Registry;

/// `--browser-import` values that pick the source browser for the user.
pub const IMPORT_CHOICES: [&str; 2] = ["default", "auto"];

/// Every browser in Browser Commander's public source catalogue.
#[must_use]
pub fn import_sources() -> Vec<&'static str> {
    browser_ids()
}

/// The domains whose cookies hold a registry's sign-in, or none.
#[must_use]
pub const fn sign_in_domains(registry: Registry) -> &'static [&'static str] {
    match registry {
        Registry::CratesIo => &["crates.io", "github.com"],
        Registry::Npm => &["npmjs.com"],
        Registry::PyPi => &["pypi.org", "github.com"],
        Registry::RubyGems => &["rubygems.org", "github.com"],
        Registry::NuGet => &["nuget.org", "login.microsoftonline.com"],
        Registry::Jsr => &["jsr.io", "github.com"],
        Registry::DockerHub => &["docker.com", "docker.io"],
        Registry::MavenCentral => &["sonatype.com"],
        Registry::VsCodeMarketplace => &["dev.azure.com", "login.microsoftonline.com"],
        Registry::OpenVsx => &["open-vsx.org", "github.com"],
        Registry::ChromeWebStore => &["google.com"],
        _ => &[],
    }
}

/// The display name of a source browser.
#[must_use]
pub fn source_name(browser: &str) -> &str {
    catalogue_name(browser)
}

/// The source id of a default-browser display name, when it can be imported.
#[must_use]
pub fn source_id(display_name: &str) -> Option<&'static str> {
    import_sources()
        .into_iter()
        .find(|id| source_name(id) == display_name)
}

/// An installed browser profile that holds a sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInSource {
    pub browser: String,
    /// The profile name, or `None` for the browser's default profile.
    pub profile: Option<String>,
    /// How the question names the source.
    pub label: String,
    /// The sign-in domains it holds cookies for.
    pub domains: Vec<String>,
}

/// How [`find_sign_in_sources`] reaches the installed browsers; tests replace it.
pub trait CookieStores {
    /// The profiles of one browser.
    fn profiles(&self, browser: &str) -> Result<Vec<BrowserProfile>>;
    /// How many cookies a profile holds for `domain`; the values are dropped.
    fn count(&self, browser: &str, profile: &str, domain: &str) -> Result<usize>;
}

/// The installed browsers, read through Browser Commander without its cache.
pub struct InstalledBrowsers;

impl CookieStores for InstalledBrowsers {
    fn profiles(&self, browser: &str) -> Result<Vec<BrowserProfile>> {
        list_browser_profiles(BrowserProfileOptions::default().browser(browser))
    }

    fn count(&self, browser: &str, profile: &str, domain: &str) -> Result<usize> {
        let mut options = BrowserCookieReadOptions::new(browser);
        options.profile = Some(profile.to_owned());
        options.domain_filter = Some(domain.to_owned());
        options.cache = false;
        options.ignore_decryption_errors = true;
        Ok(read_browser_cookies(options)?.len())
    }
}

/// Lists the installed browser profiles that hold cookies for any of
/// `domains`, the `preferred` (default) browser first. Cookie values are
/// discarded.
pub fn find_sign_in_sources(
    stores: &impl CookieStores,
    domains: &[&str],
    preferred: Option<&str>,
    verbose: bool,
) -> Vec<SignInSource> {
    let mut browsers = import_sources();
    browsers.sort_by_key(|browser| Some(*browser) != preferred);
    let mut found = Vec::new();
    for browser in browsers {
        let profiles = stores.profiles(browser).unwrap_or_else(|error| {
            if verbose {
                eprintln!("could not list {browser} profiles: {error:#}");
            }
            Vec::new()
        });
        for profile in &profiles {
            let matched: Vec<String> = domains
                .iter()
                .filter(
                    |domain| match stores.count(browser, &profile.name, domain) {
                        Ok(count) => count > 0,
                        Err(error) => {
                            if verbose {
                                eprintln!(
                                    "could not read {browser} cookies for {domain}: {error:#}"
                                );
                            }
                            false
                        }
                    },
                )
                .map(|domain| (*domain).to_owned())
                .collect();
            if matched.is_empty() {
                continue;
            }
            let name = if profile.display_name.is_empty() {
                &profile.name
            } else {
                &profile.display_name
            };
            let label = if profiles.len() > 1 {
                format!("{} ({name})", source_name(browser))
            } else {
                source_name(browser).to_owned()
            };
            found.push(SignInSource {
                browser: browser.to_owned(),
                profile: Some(profile.name.clone()),
                label,
                domains: matched,
            });
        }
    }
    found
}

/// The default browser as the only source, for `--browser-import default`.
#[must_use]
pub fn default_sources(preferred: Option<&str>, domains: &[&str]) -> Vec<SignInSource> {
    preferred
        .map(|browser| SignInSource {
            browser: browser.to_owned(),
            profile: None,
            label: source_name(browser).to_owned(),
            domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
        })
        .into_iter()
        .collect()
}

/// Asks once which installed browser's sign-in to import, the default browser
/// first; `auto` takes it without asking. Returns the chosen source, if any.
pub fn choose_sign_in_source(
    sources: Vec<SignInSource>,
    domains: &[&str],
    ask: impl FnOnce(&str) -> Result<String>,
    auto: bool,
) -> Result<Option<SignInSource>> {
    let Some(first) = sources.first() else {
        return Ok(None);
    };
    let question = format!(
        "Import your {} sign-in from {} into the automated profile?",
        domains.join(" / "),
        first.label
    );
    if auto {
        println!("  {question} yes (--browser-import auto)");
        return Ok(sources.into_iter().next());
    }
    if sources.len() > 1 {
        for (index, source) in sources.iter().enumerate() {
            println!(
                "  {}. {} ({})",
                index + 1,
                source.label,
                source.domains.join(", ")
            );
        }
    }
    let choices = if sources.len() > 1 {
        format!("[Y/n/1-{}]", sources.len())
    } else {
        "[Y/n]".to_owned()
    };
    let answer = ask(&format!("{question} {choices} "))?;
    let answer = answer.trim();
    if matches!(answer.to_ascii_lowercase().as_str(), "n" | "no") {
        return Ok(None);
    }
    let index = answer
        .parse::<usize>()
        .ok()
        .filter(|index| (1..=sources.len()).contains(index))
        .unwrap_or(1);
    Ok(sources.into_iter().nth(index - 1))
}

/// The error of `--browser-import default` when the default browser is unknown.
#[must_use]
pub fn default_unreadable() -> String {
    format!(
        "--browser-import default: the default browser is unknown or cannot be imported (Browser Commander sources: {})",
        import_sources().join(", ")
    )
}
