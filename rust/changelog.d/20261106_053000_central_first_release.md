### Added

- Verify authenticated Central namespace ownership before the first public artifact, with authorized GitHub proof setup and a durable cleanup obligation.
- Reuse or explicitly provision first-release signing keys using GnuPG, verify detached signatures through an independent public keyring, and record encrypted signing settings with actual expiry metadata.

### Fixed

- Preserve server-reported Central token names and expiration dates for gh-manager health checks without rotating existing credential pairs.
- Traverse realistic Gradle 1.1 metadata at the exact version, including string module pointers and platform metadata identifying its owning root; verify actual referenced files and keep missing delivery, malformed metadata and transport failures distinct.
- Keep macOS signing keyrings in short temporary paths so GnuPG agent sockets fit platform limits; use relative paths for native and MSYS Windows GnuPG.
