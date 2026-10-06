---
bump: patch
---

### Fixed

- Derive browser imports, launchable channels, and default-browser identification from Browser Commander's catalogue in both CLIs, including Opera GX, Yandex, Vivaldi, Arc, Whale, Safari, and Firefox-family sources.
- Include the full supported import list and discovered installed browsers in invalid-import errors, and copy the selected browser's profile when attaching to a snapshot.

### Changed

- Update direct dependencies to their latest stable releases, including browser-commander 0.25.0 (JavaScript) / 0.18.0 (Rust) and command-stream 1.5.0 / 1.4.1.
