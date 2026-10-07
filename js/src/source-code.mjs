/** Removes comments without interpreting comment markers inside quoted strings. */
export function stripComments(contents, language = "shell") {
  const cStyle = /\.(?:[cm]?[jt]s|[cm]ts|rs)$/.test(language);
  let quote = null;
  let block = false;
  let line = false;
  let escaped = false;
  let result = "";
  for (let index = 0; index < contents.length; index += 1) {
    const character = contents[index];
    const next = contents[index + 1];
    if (character === "\n" && line) {
      line = false;
    }
    if (line || block) {
      if (block && character === "*" && next === "/") {
        block = false;
        result += "  ";
        index += 1;
      } else {
        result += character === "\n" ? "\n" : " ";
      }
    } else if (quote) {
      result += character;
      if (!escaped && character === quote) {
        quote = null;
      }
      escaped = !escaped && character === "\\";
    } else if (["'", '"', "`"].includes(character)) {
      quote = character;
      result += character;
    } else if (
      (cStyle && character === "/" && next === "/") ||
      (!cStyle && character === "#")
    ) {
      line = true;
      result += " ";
    } else if (cStyle && character === "/" && next === "*") {
      block = true;
      result += "  ";
      index += 1;
    } else {
      result += character;
    }
  }
  return result;
}

/** Whether a prefix places a publishing command in an executed command. */
export function commandPosition(prefix) {
  const segment = prefix.split(/[;&|]/).at(-1).trimStart();
  return (
    /^(?:(?:npx|bunx|exec|sudo|env|time|command|pnpm\s+exec|yarn|\w+=\S*)\s+)*$/.test(
      segment,
    ) ||
    /(?:\$|shell)\s*`\s*$/.test(segment) ||
    /(?:exec(?:Sync)?|spawn(?:Sync)?|run(?:_command)?|system|check_call|check_output|Popen)\s*\(\s*["'`]\s*$/.test(
      segment,
    )
  );
}
