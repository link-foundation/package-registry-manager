---
bump: minor
---

### Added
- Both CLIs print their version with `--version`, and the npm package also installs its command as `package-registry-manager`, so `npm i -g package-registry-manager` and `npx package-registry-manager` give the command the package is named after; `package-registry-manager-js` stays as the name to use beside the Rust command (#22)

### Fixed
- The installed npm command runs again: started through npm's `node_modules/.bin` symlink (npm, npx, and global installs) it exited 0 without output for every command, `--help` included, because the entry-point check compared the symlink with the real module path (#22)
- The npm bootstrap's "Verify the packed bin entries" step fails when an installed bin prints nothing for `--version` instead of accepting a silent exit 0, and the pack step reads npm 12's `npm pack --json`, which lists packages in an object keyed by name instead of an array (#22)
