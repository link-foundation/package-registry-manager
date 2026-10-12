# First-publish investigation and requirement coverage

Issue [#54](https://github.com/link-foundation/package-registry-manager/issues/54) groups [#50](https://github.com/link-foundation/package-registry-manager/issues/50), [#51](https://github.com/link-foundation/package-registry-manager/issues/51), and [#52](https://github.com/link-foundation/package-registry-manager/issues/52). All four issue bodies and paginated comment lists were reviewed; there were no comments. PR [#55](https://github.com/link-foundation/package-registry-manager/pull/55) is the single implementation and closes each issue with its own keyword.

## Investigation plan

1. Read all issue details, all three PR comment/review endpoints, contributing guidance, recent merged PRs, branch state and CI timestamps/SHAs.
2. Map every requirement to both implementations and the shared batch, cleanup, browser and publisher paths.
3. Research upstream registry APIs, Warehouse's current form templates, browser migration capabilities and Python build frontends.
4. Add minimal failing tests before changes; preserve reproduction and CI output under ignored `ci-logs/`.
5. Implement shared behavior in Rust and JavaScript; test navigation, server errors, timeouts, stderr refusals, scoped credential imports and deferred cleanup.
6. Run complete local checks sequentially for memory-heavy work; run a real browser fixture and parity smoke, and a Python build with a frontend-free interpreter.
7. Add a release fragment, verify main ancestry, review the complete PR diff, commit and push only the prepared branch, update the PR, and inspect CI at the final SHA and mark it ready for review.

## Every requirement, alternatives and selected plan

| Requirement | Possible solutions | Selected implementation and verification |
| --- | --- | --- |
| #54.1 Read and implement all listed issues, including comments | Independent fixes or shared investigation | Read all issues/comments and implement the requirements below in both languages. |
| #54.2 One PR, no deferred listed issue | Separate PRs or one integrated change | All implementation, tests and evidence in PR #55. |
| #54.3–4 Close parent and each child with full syntax | One combined reference (insufficient) or separate keywords | PR contains `Fixes #54`, `Fixes #50`, `Fixes #51`, `Fixes #52` on separate lines. |
| #54.5 Explain already resolved/non-reproducible issues and retain references | Reimplement inherited fixes or identify existing coverage | PR #53 already corrected profile names versus paths; retained and regression-tested. The remaining reported failures were reproduced. |
| #50.1 Reuse npm's browser sign-in for login and publish approval | System opener for all pages or one automated profile | npm auth links use the same automation object as publisher forms; explicit `--open-with` remains available. Await link navigation before process completion. |
| #50.2 Poll signed-in PyPI form without Enter, including `--yes` and closed stdin | Fixed sleep, terminal prompt or page polling | Wait for all required GitHub form controls, submit, then verify the server-rendered saved row. Pending projects require a project field; existing projects do not. Navigation errors retry; rejection and timeout stop the release. |
| #50.3 Supply Python's missing `build` frontend | Install globally, `uv build`, `uvx`, or temporary venv | Selected interpreter creates a disposable venv, installs PyPA `build` there and builds wheel/sdist; cleanup on success/failure. No selected-interpreter package installation. |
| #50.4 Surface Safari EPERM normally, open Full Disk Access, request terminal restart | Verbose-only exception or actionable regular diagnostic | Identify EPERM/EACCES from profile discovery and cookie access, print Full Disk Access/restart instructions and open Privacy settings on macOS. |
| #50 expected: at most one sign-in and no terminal input in automatic first publish | Independent browsers/manual gates or session reuse and polling | Browser reuse, PyPI polling and existing npm `--yes`/web-auth approvals. Site-required 2FA remains in the visible browser. |
| #51.1 Before sign-in flag punctuation-normalized collisions and small edit distances to popular packages | Exhaustive registry crawl, distance library, or bounded candidate search | Existing variant lookups plus punctuation-free comparisons, bounded optimal-string-alignment distance (transpositions included), common monikers and up to two public search queries of 250 candidates. Report “may be refused as too similar”. |
| #51.1 Provide alternatives | Blind suffixes or fresh availability checks | Check up to eight valid proposed/scoped/descriptive names. Only fresh 404 responses mean currently free; other responses are unknown. |
| #51.2 Classify real “too similar” 403 as naming, list free scope/proposed alternatives, keep session | Retry login, or stop with naming recovery | Capture npm stderr, classify the specific refusal, use npm's proposed scoped names plus repository-owner scope, verify availability and retain npm auth even across deferred batch logout. Permission-only 403 remains an ordinary authorization failure. |
| #52.1 With no cookie import saved logins scoped to registry domains, including passkeys when supported | Full profile import, new credential readers, or upstream scoped migration | Probe disposable migrations for usable domain-scoped passwords/passkeys, then seed the dedicated Chromium profile through Browser Commander. Cookies stay the first choice. |
| #52.2 Explain no saved login before opening form, naming installed browsers | Silent fresh profile or explicit discovery diagnostic | Report requested domains and discovered installed browser names before opening the login page; distinguish unavailable import capabilities. |
| #52.3 Never repeat credentials within a run; keep/reuse sessions across packages and registry steps | Relaunch per step or existing shared executor | Preserve the shared browser object and union of registry domains across packages; retain `--keep-session` behavior and the dedicated per-user profile. |

## Root causes and codebase audit

- npm links previously used the system opener while publisher forms used a separate automated profile. JavaScript also did not wait for asynchronous opener callbacks and omitted captured stderr from the process result.
- PyPI reused a generic form helper with an Enter gate and one fixed retry. `--yes` did not mean the authenticated publisher page existed. Generic controls could match other providers' duplicated field names.
- Python prerequisites selected an interpreter but the build step assumed its environment already contained the frontend.
- Cookie discovery caught inaccessible stores as if they were empty and looked only at cookies. Upstream profile discovery can also return an error-bearing profile rather than throw.
- npm's preflight checked a finite set of punctuation variants, missing punctuation moved inside words and edit-distance refusals. Deferred batch cleanup could discard a retained session after another package's earlier successful step.

The audit covered `setup`, `setup-batch`/Rust batch, auth URL runners, browser scripts, automation launch options, crates.io's deferred import, registry domain lists, Python prerequisite messages/build steps, both manifests/locks, tests and release triggers. Manual `--no-browser` operation and explicit opener selection remain available. No registry login, package publication or personal browser profile was used for testing.

## Online research and reusable components

- [npm's moniker rules](https://blog.npmjs.org/post/168978377570/new-package-moniker-rules.html) describe punctuation stripping throughout a name and scopes as a namespace escape. Public metadata and dry-run do not expose every server-side naming rule.
- [npm registry search API](https://github.com/npm/registry/blob/main/docs/REGISTRY-API.md) supports bounded search and ranking weights. Live probes on 2026-10-12 found `rc-upload` in `gh-upload`/`upload` results, but `lodahs` did not return `lodash`; empty and wildcard queries returned HTTP 400. The implementation therefore has no empty/global query and checks common monikers separately. This is an inference-driven warning heuristic, not a reproduction of npm's private policy.
- [validate-npm-package-name](https://github.com/npm/validate-npm-package-name) checks syntax, not private naming eligibility. [fastest-levenshtein](https://github.com/ka-weihe/fastest-levenshtein) and Rust [strsim](https://docs.rs/strsim/latest/strsim/) provide distance calculations. A small identical bounded implementation keeps Rust/JS behavior consistent and includes adjacent transpositions without new dependencies.
- [PyPA build](https://build.pypa.io/en/latest/tutorial/getting-started.html) provides the frontend and [isolates build backends](https://build.pypa.io/en/latest/explanation/how-it-works.html). [uv build](https://docs.astral.sh/uv/guides/package/) or uvx could avoid manual frontend provisioning, but would add an external runtime requirement. A venv works with the interpreter already selected by the CLI.
- [PyPI pending publishers](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/) permit the initial OIDC release to create the project; registering one does not reserve its name. Current [Warehouse account form](https://github.com/pypi/warehouse/blob/main/warehouse/templates/manage/account/publishing.html), [existing-project form](https://github.com/pypi/warehouse/blob/main/warehouse/templates/manage/project/publishing.html) and [saved-row macro](https://github.com/pypi/warehouse/blob/main/warehouse/templates/manage/manage_base.html) informed provider scoping and verification. Existing projects omit the project field and project column, which are handled separately.
- Existing [Browser Commander](https://github.com/link-foundation/browser-commander) supplies domain filters and source-to-target password re-encryption. Chromium and Firefox sources can supply usable passwords to Chromium targets. Safari passwords require an explicit export; native passkeys currently report non-exportable. Firefox automation targets support cookie transfer, not password migration. These limits are surfaced instead of claiming an import succeeded.
- [Apple Privacy & Security settings](https://support.apple.com/guide/mac-help/change-privacy-security-settings-on-mac-mchl211c911f/mac) provide Full Disk Access. Permission failures are distinct from a missing session and require a terminal-app restart after changing access.

## Reproduction and validation

The original seven new JavaScript regressions failed before fixes (`ci-logs/reproduction-js.log`). Focused tests cover a login page persisting for multiple polls, no stdin, selected Python without a frontend, similarity candidates, saved-password fallback and Safari denial. Additional tests cover actual npm stderr refusal, retained cleanup, server rejection, timeout and missing-login diagnostics. An existing-project regression also failed before the follow-up fix (`ci-logs/pypi-attach-reproduction.log`); Rust and JavaScript real-browser tests verify both publisher form shapes. Trailing-punctuation names exposed an empty search query in both implementations; failing regressions (`ci-logs/empty-search-*-reproduction.log`) now verify those queries are excluded.

Reusable read-only experiments live in `experiments/issue-54/`: npm preflight against public APIs and a local Warehouse-shaped publisher page. `examples/first-publish/python-build.mjs` demonstrates an isolated Python package build with a frontend-free selected interpreter. Browser screenshots are fixture evidence, not live authenticated PyPI publication. Complete local and final CI results are recorded in the PR description.

## Limits

npm can still refuse a name not found by bounded public preflight. Availability checks are point-in-time and scopes require ownership. Browser autofill/2FA remains subject to the installed browser and account protections; the CLI neither extracts credentials into logs nor bypasses authentication. Live account verification is unavailable in this environment, so form behavior is checked against current upstream templates and a real browser fixture. Unsupported native passkey/Safari/Firefox-target migration cannot be supplied by this repository; the applicable diagnostic explains the supported route.
