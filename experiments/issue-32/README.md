# Issue 32: browser engines

The smallest reproduction is parsing `--browser-channel firefox`; before this
change the local catalogue validator rejected every BiDi entry. The new
JavaScript browser-engine suite reproduced nine failures (eight Firefox-family
channels and the stubbed launcher). The Rust catalogue test checks the same
protocol-based list and engine selection, and its native launcher test uses a
stub instead of a browser process.

The latest Browser Commander releases inspected were JavaScript 0.25.0 and
Rust 0.18.0. Contrary to the issue's premise, their `launchRealBrowser` helpers
still reject non-Chromium channels, and migration validation rejects
non-Chromium target writers. Their public WebDriver launchers do support
Firefox and BiDi. This implementation uses those APIs, imports scoped cookies
through native BiDi storage and gives errors for unsupported full migration,
snapshots, preferences and Chromium restrictions. Safari and STP have no control
protocol in these releases; upstream issue 126 tracks Safari launch support.

Run the deterministic regression tests:

```sh
node --test js/test/browser-engines.test.mjs
cargo test --manifest-path rust/Cargo.toml browser_catalogue
cargo test --manifest-path rust/Cargo.toml bidi_channel_uses_stubbed_native_launcher
```

The real Firefox experiment uses local fixture forms and a local API; it needs
no registry account and publishes nothing. It imports a synthetic HttpOnly
session cookie, fills the shared npm/PyPI form, awaits the crates.io page-fetch
script and checks the cookie is removed before closing the owned driver.

```sh
# Install Firefox and geckodriver; JavaScript installs selenium-webdriver.
PRM_FIREFOX_EXECUTABLE=/path/to/firefox xvfb-run -a \
  node experiments/issue-32/firefox-smoke.mjs
# Native Rust uses the same Firefox/geckodriver on PATH.
PRM_FIREFOX_SMOKE=1 PRM_FIREFOX_EXECUTABLE=/path/to/firefox xvfb-run -a \
  cargo test --manifest-path rust/Cargo.toml --test integration firefox_forms
```

The Chrome parity smoke tests remain unchanged. Large validation and CI logs
are saved locally outside Git. For small containers, disable Rust debug
information and build with one job to compile chromiumoxide within memory:
`CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.

CI's upstream freshness check also required command-stream 1.6.2 (JS) and
1.5.1 (Rust). The JS release introduces the unpatched braces advisory
GHSA-vfj7-8cjw-p6xm through ShellJS/fast-glob, but guards that path before
recursive parsing. We reuse upstream's single-advisory exception and audit
wrapper. `dependency-security.test.mjs` verifies the installed guard with a
finite input in a child limited to 64 MB of heap and 256 KB of stack; all
other high/critical advisories and audit transport failures remain fatal.
An attempted ShellJS downgrade broke startup and was discarded.
