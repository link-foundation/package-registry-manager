import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { test } from 'node:test';

function finding(id, name, version) {
  return { advisory: { id }, package: { name, version } };
}

const sqlite = finding('RUSTSEC-2026-0459', 'libsqlite3-sys', '0.38.2');
const chrono = finding('RUSTSEC-2026-0543', 'chrono', '0.4.45');

function auditReport(list, warnings) {
  return JSON.stringify({
    vulnerabilities: { found: list.length > 0, count: list.length, list },
    warnings,
  });
}

const baseline = auditReport([sqlite], { unmaintained: [chrono] });

function runAudit(t, event, report = baseline, status = 1) {
  const bin = mkdtempSync(join(tmpdir(), 'prm-audit-test-'));
  t.after(() => rmSync(bin, { recursive: true, force: true }));
  writeFileSync(
    join(bin, 'cargo'),
    `#!/usr/bin/env bash
[[ "$*" == 'audit --file rust/Cargo.lock --deny warnings --json' ]] || exit 42
[[ "$RUST_LOG" == warn ]] || exit 42
printf '%s\\n' "$AUDIT_TEST_REPORT"
exit "$AUDIT_TEST_STATUS"
`,
    { mode: 0o755 }
  );
  const result = spawnSync('bash', ['scripts/audit-cargo-lock.sh'], {
    encoding: 'utf8',
    env: {
      ...process.env,
      PATH: `${bin}${delimiter}${process.env.PATH}`,
      GITHUB_EVENT_NAME: event,
      AUDIT_TEST_REPORT: report,
      AUDIT_TEST_STATUS: String(status),
    },
  });
  assert.ifError(result.error);
  return result;
}

test('PR audit passes with the exact baseline, including after either fix lands', (t) => {
  for (const report of [
    baseline,
    auditReport([sqlite], {}),
    auditReport([], { unmaintained: [chrono] }),
  ]) {
    const result = runAudit(t, 'pull_request', report);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.stdout.includes(report));
    assert.match(result.stdout, /::warning title=Temporary Cargo audit exception/);
  }
});

test('the default branch, scheduled audits, and manual releases stay strict', (t) => {
  for (const event of ['push', 'schedule', 'workflow_dispatch']) {
    const result = runAudit(t, event);
    assert.equal(result.status, 1);
    assert.match(result.stdout, /::error title=Upgrade Browser Commander.*to crates.io/);
    assert.match(result.stdout, /::error title=Upgrade Command Stream.*to crates.io/);
  }
});

test('additional advisories and changed versions still fail PR audits', (t) => {
  const additional = finding('RUSTSEC-2026-9999', 'other-crate', '1.0.0');
  for (const report of [
    auditReport([sqlite, additional], { unmaintained: [chrono] }),
    auditReport([sqlite], { unmaintained: [chrono], unsound: [additional] }),
    auditReport([finding('RUSTSEC-2026-0459', 'libsqlite3-sys', '0.38.3')], {}),
  ]) {
    const result = runAudit(t, 'pull_request', report);
    assert.equal(result.status, 1);
    assert.doesNotMatch(result.stdout, /Temporary Cargo audit exception/);
  }
});

test('audit errors fail closed and a clean strict audit passes', (t) => {
  assert.equal(runAudit(t, 'pull_request', '', 1).status, 1);
  assert.equal(runAudit(t, 'pull_request', baseline, 2).status, 2);
  assert.equal(runAudit(t, 'push', auditReport([], {}), 0).status, 0);
});
