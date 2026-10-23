---
bump: patch
---

### Changed
- `browser-commander` is required at `0.21.1`, its latest release. A launch still names its browser channel (`chrome` by default), so the upstream switch of channel-less launches to Chrome for Testing does not change which browser opens

### Security
- `rustls` 0.23.46, `rustls-webpki` 0.103.16 and `webpki-root-certs` 1.0.10 in `Cargo.lock` fix RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348, RUSTSEC-2026-0349, RUSTSEC-2026-0382 and RUSTSEC-2026-0383
