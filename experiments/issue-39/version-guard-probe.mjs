import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { versionGuard } from '../../js/src/version-guard.mjs';

/** Execute ten finite cases in one interpreter so parallel CI builds stay cheap. */
export async function probeVersionGuard() {
  const root = await mkdtemp(path.join(os.tmpdir(), 'prm-guard-'));
  try {
    await writeFile(path.join(root, 'package.json'), '{"name":"@acme/tool","version":"1.0.0"}');
    await writeFile(path.join(root, 'Cargo.toml'), '[package]\nname="tool"\nversion="1.0.0"\n');
    const scripts = Object.fromEntries(['npm', 'crates-io'].map(registry => [
      registry, versionGuard(registry).slice(4, -1).map(line => line.slice(10)).join('\n'),
    ]));
    const probe = `import contextlib, io, json, os, urllib.request, urllib.error
scripts = json.loads(${JSON.stringify(JSON.stringify(scripts))})
cases = []
for registry, source in scripts.items():
    for status in ['existing', 'missing', 'forbidden', 'offline', 'mismatch']:
        output = 'output'
        open(output, 'w').close()
        os.environ['GITHUB_OUTPUT'] = output
        os.environ['RELEASE_VERSION'] = '2.0.0' if status == 'mismatch' else ''
        def lookup(request, timeout):
            assert timeout == 30
            assert '?_=' in request.full_url
            assert request.get_header('Cache-control') == 'no-cache, no-store'
            if status == 'missing': raise urllib.error.HTTPError(request.full_url, 404, 'missing', {}, None)
            if status == 'forbidden': raise urllib.error.HTTPError(request.full_url, 403, 'forbidden', {}, None)
            if status == 'offline': raise urllib.error.URLError('offline')
            return io.BytesIO(b'{}')
        urllib.request.urlopen = lookup
        code = 0
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                exec(compile(source, '<generated-guard>', 'exec'), {})
        except (SystemExit, urllib.error.HTTPError, urllib.error.URLError):
            code = 1
        with open(output) as recorded:
            value = recorded.read()
        cases.append({'registry': registry, 'status': status, 'exit': code, 'output': value})
print(json.dumps(cases))
`;
    const result = spawnSync('python3', ['-c', probe], { cwd: root, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const cases = JSON.parse(result.stdout);
    for (const { status, exit, output } of cases) {
      if (status === 'existing' || status === 'missing') {
        assert.equal(exit, 0);
        assert.equal(output, `publish=${status === 'missing'}\n`);
      } else {
        assert.notEqual(exit, 0);
        assert.equal(output, '');
      }
    }
    return cases;
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}
if (process.argv[1] === fileURLToPath(import.meta.url)) console.log(JSON.stringify(await probeVersionGuard(), null, 2));
