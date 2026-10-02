---
bump: minor
---

### Added
- `plan` and `setup` list the prerequisites before the steps: the Node.js and npm versions, the npm that runs `npm trust` and the version it resolves to, the npm account's 2FA state, `gh` sign-in and scopes, and where browser pages open (#13)
- npm bootstrap checks account-level 2FA with `npm profile get --json` after signing in, opens https://docs.npmjs.com/configuring-two-factor-authentication/ when it is off, and stops before publishing until it is on (#13)
- npm attach reads the latest release run; when npm rejected it with E404 or `invalid-publisher`, setup says so and, once trust is attached, re-runs its failed jobs with `gh run rerun <id> --failed`, which `--verify-release` then watches instead of dispatching a new run (#13)
- The packed tarball is installed into a scratch directory without lifecycle scripts, its bin entries are compared with package.json, and each installed bin is run with `--version`; npm's pack warnings are reported, without ever suggesting or running `npm pkg fix` (#13)

- `--browser-executable <path>`, `--browser-import <chrome|edge|brave|firefox>[:<profile>]`, `--browser-attach snapshot[:<profile>]|extension`, `--browser-pref <key=value>`, and `--browser-restriction <name>` choose the browser binary, import data from a real profile into the dedicated one, fill forms in a temporary copy of your own profile or in your running browser through the Browser Commander extension, and set preferences and launch restrictions; without them the dedicated profile starts exactly as before (#12)
- A smoke test, run in CI, launches the automated profile and asserts with browser-commander's `measureParity` that `navigator.webdriver` is `false` and that no switch differs from a browser started by hand (#12)
- CI fails when browser-commander, command-stream, or lino-arguments falls behind its latest release, and Dependabot proposes the bump (#12)

### Changed
- `npm trust` runs through `npx -y npm@^11.10`, or `npm@^12` when the local Node.js satisfies npm 12's engines (`^22.22.2 || ^24.15.0 || >=26.0.0`), instead of `npm@latest`, which fails on older Node.js (#13)
- Sign-in and approval URLs open through browser-commander's `open_in_user_browser`, the automated browser starts through `launch_real_browser`, and every subprocess, including the openers, runs through command-stream, except the commands that must read masked input from the terminal (`cargo login`, `gh secret set`), because command-stream only lets its shell-string runner inherit stdin; the local copies of these helpers are removed (#12)
- browser-commander 0.14.1 and command-stream 1.2.0 (#12)
