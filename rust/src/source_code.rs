//! Comment-aware source matching used by publishing discovery.

use regex::Regex;

/// Remove comments while preserving quoted text and line boundaries.
#[must_use]
pub fn strip_comments(contents: &str, language: &str) -> String {
    let c_style = ["js", "mjs", "cjs", "ts", "mts", "cts", "rs"]
        .iter()
        .any(|extension| language.ends_with(&format!(".{extension}")));
    let mut quote = None;
    let mut block = false;
    let mut line = false;
    let mut escaped = false;
    let mut result = String::new();
    let mut chars = contents.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\n' && line {
            line = false;
        }
        if line || block {
            if block && character == '*' && chars.peek() == Some(&'/') {
                block = false;
                chars.next();
                result.push_str("  ");
            } else {
                result.push(if character == '\n' { '\n' } else { ' ' });
            }
        } else if let Some(delimiter) = quote {
            result.push(character);
            if !escaped && character == delimiter {
                quote = None;
            }
            escaped = !escaped && character == '\\';
        } else if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            result.push(character);
        } else if (c_style && character == '/' && chars.peek() == Some(&'/'))
            || (!c_style && character == '#')
        {
            line = true;
            result.push(' ');
        } else if c_style && character == '/' && chars.peek() == Some(&'*') {
            block = true;
            chars.next();
            result.push_str("  ");
        } else {
            result.push(character);
        }
    }
    result
}

/// Check whether a prefix places a publishing command in an executed command.
#[must_use]
pub fn command_position(prefix: &str) -> bool {
    static PATTERNS: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
        [
        r"^(?:(?:if|elif|then|else|do|while|until|!|npx|bunx|exec|sudo|env|time|command|pnpm\s+exec|yarn|\w+=\S*)\s+)*$",
        r"(?:\$|shell)\s*`\s*$",
        r"^(?:export\s+)?\w+=`\s*$",
        r"\$\(\s*$",
        r#"(?:exec(?:Sync)?|spawn(?:Sync)?|run(?:_command)?|system|check_call|check_output|Popen)\s*\(\s*["'`]\s*$"#,
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("static pattern"))
        .collect()
    });
    let segment = prefix
        .rsplit([';', '&', '|'])
        .next()
        .unwrap_or_default()
        .trim_start();
    PATTERNS.iter().any(|pattern| pattern.is_match(segment))
}
