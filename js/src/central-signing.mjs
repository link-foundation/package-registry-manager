import { readFile } from "node:fs/promises";
import path from "node:path";
import { provisionSigning } from "./central-gpg.mjs";
import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";

/** Configuration proof and public-release proof are independent. */
export async function ensureSigningKey(session, step) {
  const slug = repositorySlug(session.plan.repository);
  const names = (step.secrets ?? []).map((entry) => entry.secret);
  if (!slug || names.length !== 2) {
    return {
      status: "unsupported",
      reason: "signing requires exact repository and two selected secrets",
    };
  }
  const github = session.options.github ?? githubServices(session.options);
  const metadata = await Promise.all(
    names.map((name) => github.secrets({ repo: slug }).getMetadata(name)),
  );
  const inherited = await Promise.all(
    names.map(
      async (name, index) =>
        metadata[index] ??
        (await github
          .secrets({ org: slug.split("/")[0] })
          .getMetadata(name)
          .catch(() => null)),
    ),
  );
  const config = JSON.parse(
    await readFile(
      path.join(
        session.options.repository ?? ".",
        ".package-registry-manager.json",
      ),
      "utf8",
    ).catch((e) => {
      if (e.code === "ENOENT") {
        return "{}";
      }
      throw e;
    }),
  );
  const settings = config.central?.signing
    ? { ...config.central.signing }
    : null;
  if (settings?.private_key_file) {
    settings.private_key_file = path.resolve(
      session.options.repository ?? ".",
      settings.private_key_file,
    );
  }
  if (settings?.key_home) {
    settings.key_home = path.resolve(
      session.options.repository ?? ".",
      settings.key_home,
    );
  }
  const saved = github.variables
    ? await github.variables({ repo: slug }).get("CENTRAL_SIGNING_PROOF")
    : null;
  let proof = saved?.value ? JSON.parse(saved.value) : null;
  const bindings = metadata.map((item) => item?.updated_at ?? null);
  if (
    proof &&
    (!bindings.every(Boolean) ||
      JSON.stringify(proof.secret_updates) !== JSON.stringify(bindings) ||
      JSON.stringify(proof.secret_names) !== JSON.stringify(names))
  ) {
    proof = null;
  }
  if (
    !proof &&
    (!settings ||
      (inherited.some(Boolean) &&
        !settings.private_key_file &&
        !settings.fingerprint))
  ) {
    return {
      status: "manual-prerequisite",
      reason:
        "configure central.signing to reuse local private material; existing encrypted secrets cannot be decrypted or silently replaced; public signature presence proves no signing capability",
    };
  }
  if (
    !proof &&
    !session.options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Verify signing material for ${slug}, publish its public key and store the encrypted signing pair? [y/N] `,
      ),
    )
  ) {
    return { status: "interrupted", reason: "signing setup declined" };
  }
  try {
    const result = await provisionSigning(settings ?? {}, slug, proof);
    if (!proof) {
      const values = [result.private_key, result.passphrase];
      for (const [index, name] of names.entries()) {
        await github
          .secrets({ repo: slug })
          .set(name, values[index], { expiresAt: result.proof.expires_at });
      }
      const fresh = await Promise.all(
        names.map((name) => github.secrets({ repo: slug }).getMetadata(name)),
      );
      if (fresh.some((item) => !item)) {
        return {
          status: "unverified",
          reason: "encrypted signing settings read-back failed",
        };
      }
      result.proof.secret_names = names;
      result.proof.secret_updates = fresh.map(
        (item) => item.updated_at ?? null,
      );
      await github
        .variables({ repo: slug })
        .ensure("CENTRAL_SIGNING_PROOF", JSON.stringify(result.proof));
    }
    return {
      status: "verified",
      fingerprint: result.proof.fingerprint,
      expires_at: result.proof.expires_at,
      evidence: {
        configuration: "detached-signature-verified",
        public_key: "keyserver-fingerprint-verified",
        publication: "unverified",
        public_release_signatures: "unverified",
        consumers: "unverified",
      },
      reused: Boolean(proof),
    };
  } catch {
    return {
      status: "unverified",
      reason:
        "signing or independent keyserver verification failed; private diagnostics withheld; signing configuration remains incomplete",
    };
  }
}
