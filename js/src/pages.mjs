// GitHub Pages readiness. A workflow that deploys with actions/deploy-pages
// fails with "Get Pages site failed ... Not Found" until Pages is enabled with
// GitHub Actions as its source, which only a repository administrator can do.

const PAGES_ACTION =
  /^\s*(?:-\s*)?uses\s*:\s*['"]?actions\/(?:configure-pages|deploy-pages|upload-pages-artifact)@/m;

/** Where Pages is configured by hand: Settings -> Pages -> Source. */
export function pagesSettingsUrl(slug) {
  return `https://github.com/${slug}/settings/pages`;
}

/** Returns the name of the first workflow that deploys to GitHub Pages. */
export function pagesWorkflow(workflows) {
  return (
    workflows.find((workflow) => PAGES_ACTION.test(workflow.contents))?.name ??
    null
  );
}

/**
 * Steps that check the repository's Pages site and, after a confirmation
 * each, enable it or switch it to GitHub Actions as its source.
 */
export function pagesSteps(slug, workflow) {
  const api = (args) => ({ program: "gh", args: ["api", ...args] });
  return [
    {
      id: "check-pages",
      title: "Check that GitHub Pages deploys from GitHub Actions",
      kind: "check",
      description: `${workflow} deploys to GitHub Pages, which must be enabled with GitHub Actions as its source.`,
      command: api([`repos/${slug}/pages`]),
      cwd: ".",
    },
    {
      id: "enable-pages",
      title: "Enable GitHub Pages with GitHub Actions as its source",
      kind: "command",
      description:
        "Create the Pages site so the workflow's deployment no longer fails with Not Found. This needs repository administrator rights.",
      command: api([
        "-X",
        "POST",
        `repos/${slug}/pages`,
        "-f",
        "build_type=workflow",
      ]),
      when: "pages-missing",
      cwd: ".",
      confirm: true,
    },
    {
      id: "use-pages-workflow",
      title: "Switch GitHub Pages to GitHub Actions as its source",
      kind: "command",
      description:
        "Pages builds from a branch, so the workflow's deployment is not served. This needs repository administrator rights.",
      command: api([
        "-X",
        "PUT",
        `repos/${slug}/pages`,
        "-f",
        "build_type=workflow",
      ]),
      when: "pages-legacy",
      cwd: ".",
      confirm: true,
    },
  ];
}

/**
 * Reads `gh api repos/{slug}/pages`: `workflow` when Pages deploys from
 * GitHub Actions, `legacy` when it builds from a branch, `missing` when the
 * API answers 404, and `undefined` when the answer is unknown.
 */
export function pagesState(result) {
  if (result.code === 0) {
    try {
      const site = JSON.parse(String(result.stdout ?? ""));
      return site.build_type === "workflow" ? "workflow" : "legacy";
    } catch {
      return undefined;
    }
  }
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  return /\bHTTP 404\b|"message"\s*:\s*"Not Found"/.test(output)
    ? "missing"
    : undefined;
}
