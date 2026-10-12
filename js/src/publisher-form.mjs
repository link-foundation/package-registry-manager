import { npmPrefillScript } from "./browser.mjs";

/** Verify the server-rendered publisher row, and surface form validation errors. */
export function publisherConfirmationScript(config) {
  return `(() => {
    const config = ${JSON.stringify(config)};
    const errors = Array.from(document.querySelectorAll('#pending-github-publisher-form .form-errors, #pending-github-publisher-form .form-group__error, .error-message, .notification-bar--danger')).map(node => node.textContent.trim()).filter(Boolean);
    const rows = Array.from(document.querySelectorAll('tr, .publisher, .card'));
    const confirmed = rows.some(row => {
      const repository = Array.from(row.querySelectorAll('a[href]')).some(link => {
        try { const url = new URL(link.href); return url.hostname === 'github.com' && (url.pathname === '/' + config.organization + '/' + config.repository || url.pathname.startsWith('/' + config.organization + '/' + config.repository + '/')); } catch { return false; }
      });
      return repository && row.querySelector('td')?.textContent.trim() === config.project
        && [config.workflow, config.environment].filter(Boolean).every(value => row.innerText.split(/\\s+/).includes(value));
    });
    return { confirmed, error: errors.join('; ') };
  })()`;
}

/** Wait through sign-in, fill and verify the pending-publisher form. */
export async function fillPublisherForm(session) {
  const page = session.automation;
  const script = npmPrefillScript(session.plan.trusted_publisher, false);
  const deadline = Date.now() + (session.options.waitTimeoutMs ?? 10 * 60_000);
  console.log(
    "  Finish sign-in and 2FA in the browser; waiting for the trusted-publisher form.",
  );
  for (;;) {
    const result = await page.evaluate(script).catch(() => ({}));
    const required = [
      "organization",
      "repository",
      "workflow",
      ...(session.plan.registry === "pypi" ? ["project"] : []),
    ];
    if (required.every((field) => result.filled?.includes(field))) {
      break;
    }
    if (Date.now() >= deadline) {
      throw new Error(
        "timed out waiting for sign-in and the trusted-publisher form; nothing submitted",
      );
    }
    await new Promise((resolve) =>
      setTimeout(resolve, session.options.pollIntervalMs ?? 500),
    );
  }
  const submit =
    session.plan.registry === "pypi" ||
    session.options.yes ||
    /^(?:y|yes)$/i.test(
      await session.prompt(
        "Submit this trusted-publisher configuration? [y/N] ",
      ),
    );
  if (!submit) {
    throw new Error(
      "trusted-publisher submission declined; release not started",
    );
  }
  if (submit) {
    const submission = await page.evaluate(
      npmPrefillScript(session.plan.trusted_publisher, true),
    );
    if (!submission.submitted) {
      throw new Error(
        "the form was not submitted; review the visible browser and submit manually",
      );
    }
    if (session.plan.registry === "pypi") {
      for (;;) {
        const state = await page
          .evaluate(publisherConfirmationScript(session.plan.trusted_publisher))
          .catch(() => ({}));
        if (state.error) {
          throw new Error(`PyPI refused the pending publisher: ${state.error}`);
        }
        if (state.confirmed) {
          session.conditions.delete("trust-missing");
          break;
        }
        if (Date.now() >= deadline) {
          throw new Error(
            "timed out verifying the PyPI pending publisher; release not started",
          );
        }
        await new Promise((resolve) =>
          setTimeout(resolve, session.options.pollIntervalMs ?? 500),
        );
      }
    }
  }
}
