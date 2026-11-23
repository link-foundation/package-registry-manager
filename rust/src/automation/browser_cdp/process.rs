use std::path::Path;

#[cfg(any(target_os = "macos", windows, test))]
use std::sync::LazyLock;

#[cfg(any(target_os = "macos", windows, test))]
static SWITCH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"\s+"?--"#).expect("static pattern"));

pub(super) async fn debugging_port(profile: &Path) -> Option<u16> {
    // The OS query occasionally starts a subprocess. Keep it off the async
    // runtime worker, and out of the common path with a recorded endpoint.
    let profile = profile.to_owned();
    tokio::task::spawn_blocking(move || discover(&profile))
        .await
        .ok()?
}

fn port_for_profile<'a>(flags: impl Iterator<Item = &'a str>, profile: &Path) -> Option<u16> {
    let mut directory = None;
    let mut port = None;
    for flag in flags {
        if flag.starts_with("type=") {
            return None;
        }
        if let Some(value) = flag.strip_prefix("user-data-dir=") {
            if directory.replace(value.trim().trim_matches('"')).is_some() {
                return None;
            }
        }
        if let Some(value) = flag.strip_prefix("remote-debugging-port=") {
            if port.is_some() {
                return None;
            }
            port = Some(value.split_whitespace().next()?.trim_end_matches('"'));
        }
    }
    let port = port?.parse::<u16>().ok().filter(|port| *port != 0)?;
    let directory = Path::new(directory?);
    if !directory.is_absolute() {
        return None;
    }
    let directory = directory.canonicalize().ok()?;
    if cfg!(windows) {
        directory
            .as_os_str()
            .eq_ignore_ascii_case(profile)
            .then_some(port)
    } else {
        (directory == profile).then_some(port)
    }
}

#[cfg(any(target_os = "macos", windows, test))]
fn listed_flags(command: &str) -> impl Iterator<Item = &str> {
    // ps flattens argv. Windows may quote the switch or just its value.
    SWITCH.split(command.trim()).skip(1)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn discover(profile: &Path) -> Option<u16> {
    let profile = profile.canonicalize().ok()?;
    let lock = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let (_, pid) = lock.to_str()?.rsplit_once('-')?;
    let pid = pid.parse::<u32>().ok().filter(|pid| *pid != 0)?;
    #[cfg(target_os = "linux")]
    {
        let command = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
        port_for_profile(
            command
                .split('\0')
                .filter_map(|flag| flag.strip_prefix("--")),
            &profile,
        )
    }
    #[cfg(target_os = "macos")]
    {
        let command = std::process::Command::new("ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        port_for_profile(
            listed_flags(std::str::from_utf8(&command.stdout).ok()?),
            &profile,
        )
    }
}

#[cfg(windows)]
fn discover(profile: &Path) -> Option<u16> {
    use std::os::windows::process::CommandExt;

    let profile = profile.canonicalize().ok()?;
    if !profile.join("Local State").is_file() {
        return None;
    }
    // Query only processes with both flags, without interpolating the profile.
    let command = std::process::Command::new("powershell.exe")
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            concat!(
                "[Console]::OutputEncoding = [Text.Encoding]::UTF8; ",
                "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process ",
                r#"-Filter "CommandLine LIKE '%--user-data-dir=%' AND CommandLine LIKE '%--remote-debugging-port=%'" "#,
                "| ForEach-Object { $_.CommandLine })",
            ),
        ])
        .output()
        .ok()?;
    let commands: Vec<String> = serde_json::from_slice(&command.stdout).ok()?;
    let mut ports = commands
        .iter()
        .filter_map(|line| port_for_profile(listed_flags(line), &profile));
    let port = ports.next()?;
    ports.all(|candidate| candidate == port).then_some(port)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn discover(_profile: &Path) -> Option<u16> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_arguments_must_identify_the_dedicated_browser() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("dedicated profile");
        std::fs::create_dir(&directory)?;
        let profile = directory.canonicalize()?;
        let process = format!(
            r#"chrome "--user-data-dir={}" --remote-debugging-port=12345 about:blank"#,
            directory.display()
        );
        assert_eq!(
            port_for_profile(listed_flags(&process), &profile),
            Some(12345)
        );
        let renderer = format!("{process} --type=renderer");
        assert_eq!(port_for_profile(listed_flags(&renderer), &profile), None);
        let other = root.path().canonicalize()?;
        assert_eq!(port_for_profile(listed_flags(&process), &other), None);
        Ok(())
    }
}
