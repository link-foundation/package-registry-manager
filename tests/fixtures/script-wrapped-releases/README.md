# Script-wrapped release fixtures

Minimal workflow and manifest layouts for the three publishers reported in
issue #99. The router fixture adds an unrelated root npm package to exercise
tarball attribution in a repository with multiple manifests. Tests inspect
these files as text and never execute their publishing commands.
