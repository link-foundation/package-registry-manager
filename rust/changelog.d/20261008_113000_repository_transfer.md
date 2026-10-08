### Fixed

- Detect repository transfers by comparing the canonical GitHub repository with
  npm, Cargo and Python manifest URLs, latest-release provenance, and available
  trusted-publisher settings. Historical provenance alone no longer completes
  setup when the current publisher cannot be verified.
- Repair transferred repositories by attaching and verifying the replacement
  publisher before removing old trust. Offer stale manifest URLs as a draft PR
  and pause release retries until the corrected metadata is on the remote branch.
  Retry the original failed jobs only when their SHA has correct metadata;
  otherwise dispatch a fresh workflow from the corrected default branch.
