---
bump: minor
---

### Added

- Discover SwiftPM packages (`swift`/`swiftpm` registry) statically from
  `Package.swift` and `swift/VERSION`, distributed as bare Git tags plus a
  GitHub release by the existing workflow. Verification distinguishes a
  missing tag, a wrong commit, a missing or incorrect tagged manifest, a
  missing release, and a failed or skipped public consumer. It never moves
  a tag shared with Composer.
- Discover Kotlin Multiplatform Gradle publications (vanniktech
  `coordinates(...)`, `publishToMavenCentral`) and their platform variants
  without running Gradle. Plans use the Gradle wrapper for `check
publishToMavenLocal` and report macOS and Android SDK requirements. A
  release counts as published only when Central has the root POM, the
  `.module` metadata and every configured variant.
- Handle every generated Central, Packagist and SwiftPM setup step:
  - The Central user-token pair is kept, acquired after confirmation, or
    returned as a prerequisite. It is stored over stdin, read back, and
    then only the causal failed jobs are retried.
  - Namespace, signing and Packagist hook steps stay incomplete without
    read-back.
  - The Packagist exact stable tag check keeps unregistered, auth-rejected,
    transport-unknown, dev-only and stable-version-missing distinct.

### Fixed

- Central and Packagist API steps are no longer routed to the crates.io
  session.
- Account scans fetch the version sidecars local discovery reads
  (`VERSION`, CMake `file(READ)`, Gradle settings), bounded and at the
  scanned commit. Go and C++ therefore keep `v0.30.0`/`0.30.0`. Unreadable
  sidecars leave versions unknown with an error, without dropping peer
  packages.
