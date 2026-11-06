### Fixed

- Complete public Packagist first-publish setup in both CLIs: validate the root
  Composer descriptor, recover stale OAuth in the same browser profile, discover
  before submitting, encrypt Safe update credentials through gh-manager and
  independently verify update acceptance and the GitHub push hook's test delivery.
- Keep registration, queued updates, exact version/source, isolated Composer
  installation and the matching GitHub release as separate verification gates.
