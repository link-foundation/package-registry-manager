# Issue 86 experiments

`node experiments/issue-86/reproduce.mjs` compares the 0.27.0 optional NuGet
staging crash and Gradle string-pointer defect with the repaired paths, using
local fixtures and no credentials.

`node experiments/issue-86/live-gradle.mjs` reads links-notation 0.31.0 metadata
and checks all seven platform coordinates and referenced files via the official
Maven Central download host. It does not build, sign or upload any artifact.
Network failures remain explicit incomplete results. Signature/hash/consumer
verification is not inferred from file availability.

Research sources and alternatives are recorded in
[the requirements inventory](../../docs/issue-86-requirements.md).
