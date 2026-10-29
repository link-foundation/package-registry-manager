import { readFile } from "node:fs/promises";
import path from "node:path";

/** Explicit source decisions supplement static discovery without executing build files. */
export async function auditConfig(root) {
  const config = JSON.parse(
    await readFile(
      path.join(root, ".package-registry-manager.json"),
      "utf8",
    ).catch((error) => {
      if (error.code === "ENOENT") {
        return "{}";
      }
      throw error;
    }),
  );
  if (
    config.publish !== undefined &&
    (!Array.isArray(config.publish) ||
      config.publish.some(
        (file) =>
          typeof file !== "string" ||
          path.posix.normalize(file) !== file ||
          file.startsWith("/") ||
          file.split("/").includes(".."),
      ))
  ) {
    throw new Error(
      "publish must list exact repository-relative manifest paths",
    );
  }
  return config;
}

/** A reviewed mapping chooses one canonical artifact; references never publish a different tarball. */
export function applyAuditConfig(items, config, workflows) {
  for (const item of items.filter(Boolean)) {
    const gradle = config.gradle?.find(
      (entry) => entry.manifest === item.manifest,
    );
    if (gradle) {
      if (
        ![
          gradle.group,
          gradle.artifact,
          gradle.version,
          gradle.component,
        ].every((value) => typeof value === "string" && /^[\w.-]+$/.test(value))
      ) {
        throw new Error(
          "explicit Gradle publication requires resolved group, artifact, version and component",
        );
      }
      Object.assign(item, {
        registry: "maven-central",
        name: `${gradle.group}:${gradle.artifact}`,
        version: gradle.version,
        publishable: true,
        publication_component: gradle.component,
      });
      delete item.discovery_skip;
    }
    const shared = config.shared_artifacts?.find(
      (entry) =>
        `${entry.registry}:${entry.name}` === `${item.registry}:${item.name}`,
    );
    if (shared) {
      item.shared_artifact = shared;
    }
    if (item.distribution) {
      item.distribution = {
        ...item.distribution,
        ...config.distribution?.[item.manifest],
        workflows: workflows
          .filter((workflow) =>
            item.registry === "conan"
              ? /conan\s+upload/.test(workflow.contents)
              : item.registry === "vcpkg"
                ? /vcpkg.*(?:x-add-version|registry|publish)/.test(
                    workflow.contents,
                  )
                : /cpack|cmake.*--install/.test(workflow.contents),
          )
          .map((workflow) => ({
            workflow: workflow.name,
            remote_variable: /CONAN_REMOTE_URL/.test(workflow.contents)
              ? "CONAN_REMOTE_URL"
              : undefined,
            credential_names: ["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD"].filter(
              (name) => workflow.contents.includes(name),
            ),
          })),
      };
    }
  }
}
