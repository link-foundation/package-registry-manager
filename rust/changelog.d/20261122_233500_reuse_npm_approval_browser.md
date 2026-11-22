### Fixed

- Reattach to the dedicated Chromium profile, keep a single npm approval tab,
  and use that tab for fresh links.
- Verify npm's cooldown checkbox after a trusted browser click, then request
  the security key once per link. Retry failed approvals and stop on success.
