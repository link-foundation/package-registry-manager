import { readFile, readdir, realpath } from 'node:fs/promises';
import path from 'node:path';

import { REGISTRIES } from './model.mjs';

const IGNORED_DIRECTORIES = new Set([
  '.git',
  '.package-registry-manager',
  '.venv',
  'node_modules',
  'target',
  'vendor',
]);

const MANIFEST_NAMES = new Set([
  'package.json',
  'Cargo.toml',
  'pyproject.toml',
  'setup.py',
  'go.mod',
  'pom.xml',
  'build.gradle',
  'build.gradle.kts',
  'composer.json',
]);

export async function inspectRepository(repository) {
  const root = await realpath(repository);
  const manifests = [];
  await collectManifests(root, manifests);
  manifests.sort();
  const preferredManifests = manifests.filter(
    (manifest) =>
      path.basename(manifest) !== 'setup.py' ||
      !manifests.includes(path.join(path.dirname(manifest), 'pyproject.toml'))
  );
  const packages = (
    await Promise.all(
      preferredManifests.map(async (manifest) =>
        parseManifest(manifest, relativePath(root, manifest))
      )
    )
  )
    .filter(Boolean)
    .sort((left, right) => {
      const registryOrder =
        REGISTRIES.indexOf(left.registry) - REGISTRIES.indexOf(right.registry);
      return (
        registryOrder ||
        [left.manifest, left.name]
          .join('\0')
          .localeCompare([right.manifest, right.name].join('\0'))
      );
    });
  const { github_owner, github_repository } = await githubCoordinates(
    root,
    preferredManifests
  );

  return {
    schema_version: 1,
    repository: {
      root,
      github_owner,
      github_repository,
      release_workflow: await releaseWorkflow(root),
    },
    packages,
  };
}

async function collectManifests(directory, manifests) {
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const item = path.join(directory, entry.name);
    if (entry.isDirectory() && !IGNORED_DIRECTORIES.has(entry.name)) {
      await collectManifests(item, manifests);
    } else if (
      entry.isFile() &&
      (MANIFEST_NAMES.has(entry.name) || /\.(?:csproj|fsproj|vbproj)$/.test(entry.name))
    ) {
      manifests.push(item);
    }
  }
}

async function parseManifest(manifestPath, manifest) {
  const contents = await readFile(manifestPath, 'utf8');
  const filename = path.basename(manifestPath);
  switch (filename) {
    case 'package.json':
      return parseNpm(contents, manifest);
    case 'Cargo.toml':
      return parseCargo(contents, manifest);
    case 'pyproject.toml':
      return parsePyproject(contents, manifest);
    case 'setup.py':
      return packageInfo(
        'pypi',
        capture(contents, /\bname\s*=\s*['"]([^'"]+)/) ??
          'unknown-python-package',
        capture(contents, /\bversion\s*=\s*['"]([^'"]+)/),
        manifest
      );
    case 'go.mod':
      return packageInfo(
        'go-modules',
        capture(contents, /^\s*module\s+(\S+)/m) ?? 'unknown-go-module',
        null,
        manifest
      );
    case 'pom.xml':
      return parseMaven(contents, manifest);
    case 'build.gradle':
    case 'build.gradle.kts':
      return parseGradle(contents, manifest);
    case 'composer.json':
      return parseComposer(contents, manifest);
    default:
      if (/\.(?:csproj|fsproj|vbproj)$/.test(filename)) {
        return parseDotnet(contents, manifestPath, manifest);
      }
      return null;
  }
}

function parseNpm(contents, manifest) {
  const metadata = parseJson(contents, manifest);
  if (!metadata.name) return null;
  const publishable = metadata.private !== true;
  return packageInfo(
    'npm',
    metadata.name,
    metadata.version ?? null,
    manifest,
    publishable,
    publishable ? [] : ['package.json marks this package as private']
  );
}

function parseCargo(contents, manifest) {
  const section = tomlSection(contents, 'package');
  if (!section) return null;
  const name = tomlString(section, 'name');
  if (!name) return null;
  const publishable = !/^\s*publish\s*=\s*false\s*$/m.test(section);
  return packageInfo(
    'crates-io',
    name,
    tomlString(section, 'version'),
    manifest,
    publishable,
    publishable ? [] : ['Cargo.toml disables publishing']
  );
}

function parsePyproject(contents, manifest) {
  const section =
    tomlSection(contents, 'project') ?? tomlSection(contents, 'tool.poetry');
  const name = section && tomlString(section, 'name');
  if (!name) return null;
  return packageInfo('pypi', name, tomlString(section, 'version'), manifest);
}

function parseDotnet(contents, manifestPath, manifest) {
  return packageInfo(
    'nuget',
    xmlTag(contents, 'PackageId') ??
      xmlTag(contents, 'AssemblyName') ??
      path.basename(manifestPath, path.extname(manifestPath)),
    xmlTag(contents, 'PackageVersion') ?? xmlTag(contents, 'Version'),
    manifest
  );
}

function parseMaven(contents, manifest) {
  const artifact = xmlTag(contents, 'artifactId') ?? 'unknown-maven-artifact';
  const group = xmlTag(contents, 'groupId');
  return packageInfo(
    'maven-central',
    group ? `${group}:${artifact}` : artifact,
    xmlTag(contents, 'version'),
    manifest
  );
}

function parseGradle(contents, manifest) {
  const group = capture(contents, /^\s*group\s*=\s*['"]([^'"]+)/m);
  const artifact =
    capture(
      contents,
      /^\s*(?:archivesBaseName|rootProject\.name)\s*=\s*['"]([^'"]+)/m
    ) ?? 'gradle-project';
  return packageInfo(
    'maven-central',
    group ? `${group}:${artifact}` : artifact,
    capture(contents, /^\s*version\s*=\s*['"]([^'"]+)/m),
    manifest
  );
}

function parseComposer(contents, manifest) {
  const metadata = parseJson(contents, manifest);
  if (!metadata.name) return null;
  return packageInfo(
    'packagist',
    metadata.name,
    metadata.version ?? null,
    manifest
  );
}

function packageInfo(
  registry,
  name,
  version,
  manifest,
  publishable = true,
  problems = []
) {
  const result = { registry, name, version: version ?? null, manifest, publishable };
  if (problems.length > 0) result.problems = problems;
  return result;
}

function parseJson(contents, manifest) {
  try {
    return JSON.parse(contents);
  } catch (error) {
    throw new Error(`invalid JSON in ${manifest}: ${error.message}`, { cause: error });
  }
}

function tomlSection(contents, name) {
  const escaped = name.replaceAll('.', '\\.');
  return capture(
    contents,
    new RegExp(
      `^\\[${escaped}\\]\\s*$([\\s\\S]*?)(?=^\\[|(?![\\s\\S]))`,
      'm'
    )
  );
}

function tomlString(section, key) {
  return capture(section, new RegExp(`^\\s*${key}\\s*=\\s*['\"]([^'\"]+)`, 'm'));
}

function xmlTag(contents, tag) {
  return capture(contents, new RegExp(`<${tag}(?:\\s[^>]*)?>\\s*([^<]+?)\\s*</${tag}>`, 's'));
}

function capture(contents, expression) {
  return expression.exec(contents)?.[1] ?? null;
}

function relativePath(root, item) {
  return path.relative(root, item).split(path.sep).join('/');
}

async function githubCoordinates(root, manifests) {
  let contents;
  try {
    contents = await readFile(path.join(root, '.git/config'), 'utf8');
  } catch {
    contents = '';
  }
  const remote = capture(contents, /^\s*url\s*=\s*(\S+)/m);
  const remoteCoordinates = parseGithubUrl(remote);
  if (remoteCoordinates) return remoteCoordinates;

  for (const manifest of manifests) {
    const filename = path.basename(manifest);
    contents = await readFile(manifest, 'utf8');
    let repository;
    if (filename === 'package.json') {
      const metadata = parseJson(contents, relativePath(root, manifest));
      repository =
        typeof metadata.repository === 'string'
          ? metadata.repository
          : metadata.repository?.url;
    } else if (filename === 'Cargo.toml') {
      const packageSection = tomlSection(contents, 'package');
      repository = packageSection && tomlString(packageSection, 'repository');
    }
    const coordinates = parseGithubUrl(repository);
    if (coordinates) return coordinates;
  }
  return { github_owner: null, github_repository: null };
}

function parseGithubUrl(remote) {
  const normalized = remote
    ?.replace(/\.git$/, '')
    .replace(/^git\+/, '')
    .replace('git@github.com:', 'https://github.com/')
    .replace('ssh://git@github.com/', 'https://github.com/');
  if (!normalized?.startsWith('https://github.com/')) {
    return null;
  }
  const [github_owner, github_repository] = normalized
    .slice('https://github.com/'.length)
    .split('/');
  return { github_owner: github_owner ?? null, github_repository: github_repository ?? null };
}

async function releaseWorkflow(root) {
  const directory = path.join(root, '.github/workflows');
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch {
    return null;
  }
  entries.sort((left, right) => left.name.localeCompare(right.name));
  let fallback = null;
  for (const entry of entries) {
    if (!entry.isFile() || !/\.ya?ml$/.test(entry.name)) continue;
    const contents = await readFile(path.join(directory, entry.name), 'utf8');
    if (contents.includes('npm publish') || contents.includes('npm stage publish')) {
      return entry.name;
    }
    if (!fallback && entry.name.includes('release')) fallback = entry.name;
  }
  return fallback;
}
