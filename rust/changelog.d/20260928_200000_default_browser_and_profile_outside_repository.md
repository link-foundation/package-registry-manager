---
bump: minor
---

### Added
- `setup --browser default|automated` chooses where sign-in and approval URLs open; `default`, the default, uses the user's default browser (#9)

### Changed
- npm web-auth URLs (`npm login`, `npm publish` 2FA, `npm trust`) open in the user's default browser (`open`, `xdg-open`, or the Windows URL handler), where they are usually already signed in. The automated browser profile is only used to fill the trusted-publisher form (#9)
- The automation browser profile now defaults to a per-user state directory outside any repository: `~/Library/Application Support/package-registry-manager/browser-profile` on macOS, `$XDG_STATE_HOME` (or `~/.local/state`)`/package-registry-manager/browser-profile` on Linux, and `%LOCALAPPDATA%\package-registry-manager\browser-profile` on Windows (#8)

### Fixed
- A browser profile inside a Git work tree gets a `.gitignore` containing `*` before the browser starts, and setup refuses to continue if Git still does not ignore it or already tracks files in it, so session cookies cannot be committed. A profile left in `.package-registry-manager/` by an earlier release is protected the same way, with a warning to delete it (#8)
- When a web login is not completed and npm falls back to its legacy `Username:` prompt, setup stops npm and says to re-run for a fresh login link instead of failing with `npm exited with status 1` (#9)
