import { parse } from "yaml";

/**
 * Reads `on.workflow_dispatch.inputs` from a workflow file. YAML 1.2 keeps the
 * key `on` a string; older parsers may turn it into `true`.
 */
export function workflowDispatchInputs(yamlText) {
  const document = parse(yamlText);
  const dispatch =
    document?.on?.workflow_dispatch ?? document?.true?.workflow_dispatch;
  if (!dispatch || typeof dispatch !== "object") {
    return {};
  }
  return dispatch.inputs ?? {};
}

/**
 * Chooses values for a non-interactive dispatch. A declared default wins, then
 * the first choice option. Required inputs without either value are refused
 * instead of being sent empty.
 */
export function dispatchInputValues(yamlText) {
  const values = {};
  const missing = [];
  for (const [name, input = {}] of Object.entries(
    workflowDispatchInputs(yamlText),
  )) {
    if (input.default !== undefined) {
      values[name] = String(input.default);
    } else if (Array.isArray(input.options) && input.options.length > 0) {
      values[name] = String(input.options[0]);
    } else if (input.type === "boolean") {
      values[name] = "false";
    } else if (input.required) {
      missing.push(name);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `workflow_dispatch requires values for ${missing.join(", ")}; pass them with --workflow-input name=value`,
    );
  }
  return values;
}

/** Builds the `-f name=value` flags for `gh workflow run`. */
export function dispatchInputArgs(yamlText) {
  return Object.entries(dispatchInputValues(yamlText)).flatMap(
    ([name, value]) => ["-f", `${name}=${value}`],
  );
}
