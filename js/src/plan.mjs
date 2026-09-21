export function buildPlans(inspection, selected = []) {
  const registries = new Set(selected);
  return inspection.packages
    .filter((item) => registries.size === 0 || registries.has(item.registry))
    .map((item) => buildPlan(inspection, item));
}

function buildPlan(inspection, packageInfo) {
  const command = (program, args) => ({ program, args });
  const check = (id, title, description, commandSpec) => ({
    id,
    title,
    kind: 'check',
    description,
    command: commandSpec,
  });
  const browser = (id, title, description, url) => ({
    id,
    title,
    kind: 'browser',
    description,
    url,
  });
  let steps;
  let trustedPublisher;

  switch (packageInfo.registry) {
    case 'npm':
      steps = [
        check(
          'validate-package',
          'Validate npm metadata',
          'Read the package metadata with npm before changing registry settings.',
          command('npm', ['pkg', 'get', 'name', 'version', 'repository'])
        ),
        browser(
          'configure-trusted-publisher',
          'Configure npm trusted publishing',
          'Sign in in the isolated browser profile, review the prefilled GitHub Actions identity, and explicitly confirm submission.',
          `https://www.npmjs.com/package/${urlPathSegment(packageInfo.name)}/access`
        ),
      ];
      if (
        inspection.repository.github_owner &&
        inspection.repository.github_repository &&
        inspection.repository.release_workflow
      ) {
        trustedPublisher = {
          provider: 'github-actions',
          organization: inspection.repository.github_owner,
          repository: inspection.repository.github_repository,
          workflow: inspection.repository.release_workflow,
        };
      }
      break;
    case 'crates-io':
      steps = [
        check(
          'validate-package',
          'Validate the crate',
          'Package the crate without uploading it.',
          command('cargo', ['publish', '--dry-run'])
        ),
        browser(
          'review-account',
          'Review crates.io account settings',
          'Sign in with GitHub and review API-token or trusted-publishing settings. The tool never creates or prints a token.',
          'https://crates.io/settings/tokens'
        ),
      ];
      break;
    case 'pypi':
      steps = [
        check(
          'build-package',
          'Build the Python distribution',
          'Build source and wheel distributions locally.',
          command('python', ['-m', 'build'])
        ),
        browser(
          'configure-trusted-publisher',
          'Configure a PyPI trusted publisher',
          "Sign in and add the repository's GitHub Actions workflow as a trusted publisher.",
          `https://pypi.org/manage/project/${urlPathSegment(packageInfo.name)}/settings/publishing/`
        ),
      ];
      break;
    case 'go-modules':
      steps = [
        check(
          'test-module',
          'Test the Go module',
          'Run all module tests before tagging a semantic version.',
          command('go', ['test', './...'])
        ),
        {
          id: 'publish-tag',
          title: 'Push a semantic-version tag',
          kind: 'manual',
          description:
            'Go modules are published from repository tags; after pushing the tag, request it through proxy.golang.org.',
          url: 'https://go.dev/ref/mod#publishing-a-module',
        },
      ];
      break;
    case 'nuget':
      steps = [
        check(
          'pack-package',
          'Build the NuGet package',
          'Create the package locally without pushing it.',
          command('dotnet', ['pack', '--configuration', 'Release'])
        ),
        browser(
          'configure-trusted-publishing',
          'Configure NuGet trusted publishing',
          'Sign in and add a GitHub Actions federated credential for this package.',
          'https://www.nuget.org/account/TrustedPublishing'
        ),
      ];
      break;
    case 'maven-central':
      steps = [
        check(
          'verify-build',
          'Verify the Maven build',
          'Run the build lifecycle without deploying an artifact.',
          command('mvn', ['--batch-mode', 'verify'])
        ),
        browser(
          'verify-namespace',
          'Verify a Central namespace',
          'Sign in to the Central Portal and verify the namespace used by the package coordinates.',
          'https://central.sonatype.com/publishing/namespaces'
        ),
      ];
      break;
    case 'packagist':
      steps = [
        check(
          'validate-package',
          'Validate Composer metadata',
          'Strictly validate composer.json before submitting it.',
          command('composer', ['validate', '--strict'])
        ),
        browser(
          'submit-repository',
          'Submit the repository to Packagist',
          'Sign in and submit the public VCS repository URL. Packagist reads package versions from tags.',
          'https://packagist.org/packages/submit'
        ),
      ];
      break;
    default:
      throw new Error(`no setup plan for registry ${packageInfo.registry}`);
  }

  const plan = {
    schema_version: 1,
    registry: packageInfo.registry,
    package: structuredClone(packageInfo),
    repository: structuredClone(inspection.repository),
    steps,
  };
  if (trustedPublisher) plan.trusted_publisher = trustedPublisher;
  return plan;
}

export function packageDirectory(manifest) {
  const separator = manifest.lastIndexOf('/');
  return separator === -1 ? '.' : manifest.slice(0, separator);
}

function urlPathSegment(value) {
  return encodeURIComponent(value).replaceAll('%40', '@');
}
