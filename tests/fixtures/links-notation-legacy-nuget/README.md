# links-notation legacy NuGet workflows

Frozen copies of the workflows (`js.yml`, `release.yml`, `publish-csharp.yml` and the reusable workflows they call) from
links-notation before it adopted NuGet trusted publishing upstream. The
NuGet migration test runs against them so the API-key-to-OIDC rewrite
stays covered after `tests/fixtures/links-notation` was refreshed (issue #81).
