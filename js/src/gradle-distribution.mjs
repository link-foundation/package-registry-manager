import { readFile, stat } from "node:fs/promises";
import path from "node:path";

/**
 * Kotlin Multiplatform targets and the artifact suffix the Kotlin Gradle
 * plugin publishes each one under (`<artifact>-<suffix>`), next to the root
 * publication that carries the Gradle module metadata.
 */
const KMP_TARGETS = new Map([
  ["jvm", "jvm"],
  ["js", "js"],
  ["wasmJs", "wasm-js"],
  ["wasmWasi", "wasm-wasi"],
  ["androidTarget", "android"],
  ["android", "android"],
  ["iosArm64", "iosarm64"],
  ["iosX64", "iosx64"],
  ["iosSimulatorArm64", "iossimulatorarm64"],
  ["macosArm64", "macosarm64"],
  ["macosX64", "macosx64"],
  ["tvosArm64", "tvosarm64"],
  ["tvosX64", "tvosx64"],
  ["tvosSimulatorArm64", "tvossimulatorarm64"],
  ["watchosArm32", "watchosarm32"],
  ["watchosArm64", "watchosarm64"],
  ["watchosX64", "watchosx64"],
  ["watchosSimulatorArm64", "watchossimulatorarm64"],
  ["watchosDeviceArm64", "watchosdevicearm64"],
  ["linuxX64", "linuxx64"],
  ["linuxArm64", "linuxarm64"],
  ["mingwX64", "mingwx64"],
  ["androidNativeArm32", "androidnativearm32"],
  ["androidNativeArm64", "androidnativearm64"],
  ["androidNativeX86", "androidnativex86"],
  ["androidNativeX64", "androidnativex64"],
]);

/** Gradle tasks that publish to the Central Portal, by plugin. */
export const GRADLE_PUBLISH_TASKS = Object.freeze([
  "publishToMavenCentral",
  "publishAndReleaseToMavenCentral",
  "publishAllPublicationsToMavenCentralRepository",
  "publish",
]);

const SKIP_REASON =
  "Gradle application/build definition has no resolved Maven publication coordinates and component";

/**
 * Reads a Gradle build script as text, never running Gradle, and resolves
 * the Maven publication: classic `maven-publish` coordinates, or the
 * vanniktech plugin's `coordinates(group, artifact, version)` with Kotlin
 * Multiplatform platform publications. `version.toString()` and
 * `libs.versions.*` references resolve from the script, gradle.properties
 * or the version catalog; anything else stays unknown with its reason.
 */
export async function gradleDistribution(contents, manifest, manifestPath) {
  const code = stripGradleComments(contents);
  const directory = path.dirname(manifestPath);
  const sibling = (name) => readSibling(directory, name);
  const properties = parseProperties(
    (await sibling("gradle.properties")) ?? "",
  );
  const settings =
    (await sibling("settings.gradle.kts")) ??
    (await sibling("settings.gradle")) ??
    "";
  // A top-level assignment wins; plugin declarations such as
  // `kotlin("jvm") version "2.0.0"` never name the project version.
  const literal = (key, text = code) =>
    [
      `^${key}\\s*(?:=\\s*)?['"]([^'"$]+)['"]`,
      `(?:^|[\\s{;])${key}\\s*=\\s*['"]([^'"$]+)['"]`,
      ...(key === "version"
        ? []
        : [`(?:^|[\\s{;])${key}\\s+['"]([^'"$]+)['"]`]),
    ]
      .map((source) => new RegExp(source, "m").exec(text)?.[1])
      .find(Boolean) ?? null;
  const rootName = literal("rootProject\\.name", stripGradleComments(settings));
  const vanniktech =
    /com\.vanniktech\.maven\.publish/.test(code) &&
    /\bmavenPublishing\s*\{/.test(code);
  const coordinates =
    /\bcoordinates\s*\(\s*([^,()]+(?:\([^)]*\))?)\s*,\s*([^,()]+(?:\([^)]*\))?)\s*,\s*([^()]+?(?:\([^)]*\))?)\s*\)/.exec(
      code,
    );
  if (!vanniktech) {
    return classicGradle(code, manifest, literal, rootName);
  }
  const problems = [];
  const resolve = async (expression, role) => {
    const value = expression?.trim();
    if (!value) {
      return null;
    }
    const quoted = /^"([^"$]+)"$|^'([^'$]+)'$/.exec(value);
    if (quoted) {
      return quoted[1] ?? quoted[2];
    }
    if (
      /^(?:project\.)?version(?:\.toString\(\))?$|^"\$\{?version\}?"$/.test(
        value,
      )
    ) {
      return literal("version") ?? properties.version ?? null;
    }
    if (/^(?:project\.)?group(?:\.toString\(\))?$/.test(value)) {
      return literal("group") ?? properties.group ?? null;
    }
    const catalog = /^libs\.versions\.([\w.]+)\.get\(\)$/.exec(value);
    if (catalog) {
      const toml =
        (await readSibling(directory, "gradle/libs.versions.toml")) ?? "";
      const key = catalog[1].replaceAll(".", "[-_.]");
      const found = new RegExp(`^\\s*${key}\\s*=\\s*"([^"]+)"`, "m").exec(
        /^\[versions\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(toml)?.[1] ?? "",
      )?.[1];
      if (found) {
        return found;
      }
    }
    const property =
      /^(?:providers\.gradleProperty|property|findProperty)\(\s*"([\w.-]+)"\s*\)(?:\.get\(\))?$/.exec(
        value,
      );
    if (property && properties[property[1]]) {
      return properties[property[1]];
    }
    problems.push(
      `dynamic Gradle ${role} expression ${value}; list the reviewed value under "gradle" in .package-registry-manager.json`,
    );
    return null;
  };
  const group =
    (await resolve(coordinates?.[1], "group")) ??
    literal("group") ??
    properties.GROUP ??
    null;
  const artifact =
    (await resolve(coordinates?.[2], "artifact")) ??
    properties.POM_ARTIFACT_ID ??
    rootName;
  const version =
    (await resolve(coordinates?.[3], "version")) ??
    (coordinates
      ? null
      : (literal("version") ??
        properties.VERSION_NAME ??
        properties.version ??
        null));
  if (!group || !artifact) {
    return {
      manifest,
      publishable: false,
      discovery_skip: `${SKIP_REASON}${problems.length ? `: ${problems.join("; ")}` : ""}`,
    };
  }
  const multiplatform =
    /kotlin\s*\(\s*["']multiplatform["']\s*\)|org\.jetbrains\.kotlin\.multiplatform/.test(
      code,
    );
  const wrapper = (await exists(path.join(directory, "gradlew")))
    ? "./gradlew"
    : "gradle";
  const task = /publishToMavenCentral\s*\(/.test(code)
    ? "publishToMavenCentral"
    : "publishAllPublicationsToMavenCentralRepository";
  const item = {
    registry: "maven-central",
    name: `${group}:${artifact}`,
    version,
    manifest,
    publishable: true,
    build_tool: "gradle",
    gradle: {
      plugin: "com.vanniktech.maven.publish",
      wrapper,
      working_directory: path.posix.dirname(manifest),
      publish_task: task,
      automatic_release: /automaticRelease\s*=\s*true/.test(code),
      signing: /signAllPublications\s*\(/.test(code),
      multiplatform,
    },
  };
  if (multiplatform) {
    item.variants = kmpVariants(code, group, artifact);
    item.gradle.requirements = gradleRequirements(item.variants);
  }
  if (problems.length) {
    item.warnings = problems;
  }
  return item;
}

function classicGradle(code, manifest, literal, rootName) {
  const group = literal("groupId") ?? literal("group");
  const artifact =
    literal("artifactId") ?? literal("archivesBaseName") ?? rootName;
  const version = literal("version");
  if (
    !group ||
    !artifact ||
    !version ||
    !/maven-publish|MavenPublication/.test(code) ||
    !/from\s*\(?\s*components|artifact\s*\(/.test(code)
  ) {
    return { manifest, publishable: false, discovery_skip: SKIP_REASON };
  }
  return {
    registry: "maven-central",
    name: `${group}:${artifact}`,
    version,
    manifest,
    publishable: true,
    build_tool: "gradle",
  };
}

/**
 * The platform publications a Kotlin Multiplatform build declares, each
 * marked when it only exists behind a condition (such as an Android SDK)
 * or needs a macOS host (Apple targets).
 */
export function kmpVariants(code, group, artifact) {
  const block = kotlinBlock(code);
  const variants = [];
  const names = [...KMP_TARGETS.keys()].join("|");
  const expression = new RegExp(
    `(?<![\\w.])(${names})\\s*(?:\\(\\s*(?:"[^"]*")?\\s*\\)|\\{)|KotlinMultiplatformAndroidLibraryTarget>\\s*\\(\\s*"(android)"`,
    "g",
  );
  for (const match of block.text.matchAll(expression)) {
    const target = match[1] ?? match[2];
    const suffix = KMP_TARGETS.get(target);
    if (variants.some((item) => item.suffix === suffix)) {
      continue;
    }
    const conditional = insideCondition(block.text, match.index);
    const apple = /^(?:ios|macos|tvos|watchos)/.test(target);
    variants.push({
      target: target === "androidTarget" ? "android" : target,
      suffix,
      coordinates: `${group}:${artifact}-${suffix}`,
      ...(apple ? { host: "macos" } : {}),
      ...(target.startsWith("android") && !target.startsWith("androidNative")
        ? { requires: "Android SDK" }
        : {}),
      ...(conditional ? { conditional: true } : {}),
    });
  }
  return variants;
}

function gradleRequirements(variants) {
  const requirements = [];
  if (variants.some((item) => item.host === "macos")) {
    requirements.push({
      kind: "runner",
      requirement: "macOS",
      reason:
        "Kotlin/Native publishes Apple targets only from a macOS host; a Linux runner silently omits them",
      targets: variants
        .filter((item) => item.host === "macos")
        .map((item) => item.target),
    });
  }
  if (variants.some((item) => item.requires === "Android SDK")) {
    requirements.push({
      kind: "sdk",
      requirement: "Android SDK (ANDROID_HOME or ANDROID_SDK_ROOT)",
      reason:
        "the Android publication exists only when the build finds an SDK; without it the release lacks the -android artifact",
      targets: ["android"],
    });
  }
  requirements.push({
    kind: "toolchain",
    requirement: "JDK and the Gradle wrapper",
    reason: "the wrapper pins the Gradle version that builds every variant",
  });
  return requirements;
}

function kotlinBlock(code) {
  const start = /(?:^|\n)\s*kotlin\s*\{/.exec(code);
  if (!start) {
    return { text: "" };
  }
  const open = code.indexOf("{", start.index);
  let depth = 0;
  for (let index = open; index < code.length; index += 1) {
    if (code[index] === "{") {
      depth += 1;
    } else if (code[index] === "}") {
      depth -= 1;
      if (depth === 0) {
        return { text: code.slice(open + 1, index) };
      }
    }
  }
  return { text: code.slice(open + 1) };
}

/** Whether the position sits inside an `if (...) { ... }` block. */
function insideCondition(text, position) {
  const stack = [];
  for (let index = 0; index < position; index += 1) {
    if (text[index] === "{") {
      stack.push(
        /\bif\s*\([^{}]*\)\s*$/.test(
          text.slice(Math.max(0, index - 200), index),
        ),
      );
    } else if (text[index] === "}") {
      stack.pop();
    }
  }
  return stack.some(Boolean);
}

/** Removes Kotlin/Groovy comments while keeping `//` inside URLs. */
export function stripGradleComments(contents) {
  return contents
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:"\\])\/\/[^\n]*/g, "$1");
}

function parseProperties(contents) {
  const values = {};
  for (const line of contents.split(/\r?\n/)) {
    const match = /^\s*([\w.-]+)\s*[=:]\s*(.*?)\s*$/.exec(line);
    if (match && !line.trimStart().startsWith("#")) {
      values[match[1]] = match[2];
    }
  }
  return values;
}

async function readSibling(directory, name) {
  try {
    return await readFile(path.join(directory, name), "utf8");
  } catch (error) {
    if (["ENOENT", "ENOTDIR"].includes(error.code)) {
      return null;
    }
    throw error;
  }
}

async function exists(file) {
  try {
    return (await stat(file)).isFile();
  } catch {
    return false;
  }
}

/** Validation and Central steps for a Gradle-built Maven Central package. */
export function gradleValidationSteps(item) {
  const gradle = item.gradle ?? {};
  const wrapper = gradle.wrapper ?? "./gradlew";
  const steps = [
    {
      id: "validate-package",
      title: "Validate the Gradle publication without uploading",
      kind: "check",
      description: `Run the official Gradle check and local publication tasks in ${gradle.working_directory ?? "the build directory"}; nothing reaches Central.`,
      command: {
        program: wrapper,
        args: [
          "--no-daemon",
          "--console=plain",
          "check",
          "publishToMavenLocal",
        ],
      },
    },
  ];
  if (gradle.requirements?.length) {
    steps.push({
      id: "verify-gradle-toolchain",
      title: "Confirm the build hosts and SDKs for every variant",
      kind: "manual",
      state: "manual-prerequisite",
      requirements: gradle.requirements,
      description: `${gradle.requirements.map((entry) => `${entry.requirement}: ${entry.reason}`).join(". ")}. A missing runner, SDK or toolchain is separate from Central authentication.`,
    });
  }
  return steps;
}

/**
 * Exact Central read-back for a Gradle publication: the root POM, the Gradle
 * module metadata and every platform publication it references. A root POM
 * alone never certifies the platform variants.
 */
export function gradlePublicationStep(item) {
  const [group, artifact] = (item.name ?? "").split(":");
  const variants = item.variants ?? [];
  return {
    id: "verify-central-publication",
    title: `Verify ${item.name}:${item.version ?? "(unresolved)"} and its ${variants.length} platform publications on Central`,
    kind: "api",
    coordinates: { group, artifact, version: item.version ?? null },
    variants: variants.map((entry) => entry.coordinates),
    description: `Read the exact root POM and ${item.gradle?.multiplatform ? "Gradle module metadata (.module)" : "artifact"} from repo1.maven.org, then every platform coordinate the metadata references (${variants.map((entry) => entry.suffix).join(", ") || "none"}). Missing module metadata or a missing variant keeps the release partial; a skipped or unlogged ${item.gradle?.publish_task ?? "publish"} job is never publication. Published versions are immutable and never re-uploaded.`,
    url: `https://central.sonatype.com/artifact/${group}/${artifact}`,
  };
}
