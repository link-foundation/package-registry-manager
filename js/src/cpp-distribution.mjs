import { readFile } from "node:fs/promises";
import path from "node:path";

/** Read distribution metadata without executing a CMake file or Python recipe. */
export async function cppDistribution(
  contents,
  filename,
  manifest,
  manifestPath,
) {
  let cmake = filename === "CMakeLists.txt" ? contents : "";
  if (!cmake) {
    try {
      cmake = await readFile(
        path.join(path.dirname(manifestPath), "CMakeLists.txt"),
        "utf8",
      );
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }
  }
  const project =
    /\bproject\s*\(\s*([\w.-]+)\s+VERSION\s+(\d+(?:\.\d+)+)/i.exec(cmake);
  let name = project?.[1];
  let version = project?.[2];
  const registry =
    filename === "vcpkg.json"
      ? "vcpkg"
      : filename === "conanfile.py"
        ? "conan"
        : "cmake";
  if (registry === "vcpkg") {
    const data = JSON.parse(contents);
    name = data.name;
    version =
      data.version ??
      data["version-semver"] ??
      data["version-string"] ??
      data["version-date"];
  } else if (registry === "conan") {
    name = /^\s*name\s*=\s*["']([^"']+)/m.exec(contents)?.[1] ?? name;
    version = /^\s*version\s*=\s*["']([^"']+)/m.exec(contents)?.[1] ?? version;
  }
  if (
    !name ||
    (registry === "cmake" &&
      !/install\s*\(|CPack|configure_package_config_file/i.test(contents))
  ) {
    return {
      manifest,
      discovery_skip:
        "CMake build definition has no distribution installation/configuration",
      publishable: false,
    };
  }
  return {
    registry,
    name,
    version: version ?? null,
    manifest,
    publishable: true,
    distribution: {
      version_source: project
        ? path.posix.join(path.posix.dirname(manifest), "CMakeLists.txt")
        : manifest,
      provider_status: "unverified",
      ...(registry === "conan"
        ? {
            remote_required: "CONAN_REMOTE_URL",
            credential_names: ["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD"],
          }
        : {}),
    },
    warnings: [
      registry === "conan"
        ? "Conan remote configuration and publication remain unverified; a local recipe does not establish registry presence"
        : `${registry} distribution metadata does not establish a public registry publication`,
    ],
  };
}

/** Validation and explicit channel prerequisites; no upload is performed. */
export function cppPlan(item) {
  const commands = {
    cmake: {
      program: "cmake",
      args: ["-S", ".", "-B", "build/prm-validation"],
    },
    conan: { program: "conan", args: ["inspect", ".", "--format=json"] },
    vcpkg: {
      program: "vcpkg",
      args: ["install", "--dry-run", "--x-manifest-root=."],
    },
  };
  return [
    {
      id: "validate-package",
      title: `Validate ${item.registry} distribution`,
      kind: "check",
      command: commands[item.registry],
      description:
        "Validate the manifest locally before reviewing any publication settings.",
    },
    ...(item.registry === "conan"
      ? [
          {
            id: "audit-distribution-channel",
            title: "Read Conan remote and credential health",
            kind: "api",
            description:
              "Read effective remote configuration and credential health through gh-manager. Unknown credentials and missing remotes remain explicit; do not upload or rotate credentials.",
          },
        ]
      : []),
    {
      id: "review-distribution-channel",
      title: "Resolve the distribution provider and CI configuration",
      kind: "manual",
      description:
        item.registry === "conan"
          ? "Read CONAN_REMOTE_URL and credential health for CONAN_LOGIN_USERNAME/CONAN_PASSWORD through gh-manager at the effective repository/environment scope. Choose the exact remote, validate conan create and consumer tests in reviewed CI, then configure narrowly scoped existing credentials. No public registry state or permission is established by a recipe."
          : `Review ${item.registry === "vcpkg" ? "the chosen vcpkg registry repository, baseline, port and version database" : "CMake install/export, package config, CPack and GitHub Release workflow"}; validate installation and a consumer in CI. Provider lookup/setup is unsupported until the destination is explicitly configured; publication remains unverified.`,
    },
  ];
}
