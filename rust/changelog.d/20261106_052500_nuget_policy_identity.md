### Fixed

- Handle optional NuGet Gallery scope controls and normalize saved policy scopes using server-rendered labels, independently of pending edits.
- Resolve nested NuGet publishing workflow identities separately from the CI caller, retaining exact package scopes, activation and immutable GitHub identity checks.
- Use the reviewed gh-manager 1.5.0 release in both runtimes for encrypted settings and verified expiry metadata.
