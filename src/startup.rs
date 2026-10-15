use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupRow {
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub path: String,
}

pub fn sample_startup() -> Vec<StartupRow> {
    let mut rows = Vec::new();
    #[cfg(windows)]
    {
        rows.extend(query_run(
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "Registro (usuario)",
        ));
        rows.extend(query_run(
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
            "Registro",
        ));
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let folder = Path::new(&appdata)
                .join(r"Microsoft\Windows\Start Menu\Programs\Startup");
            rows.extend(folder_entries(&folder, "Carpeta de inicio"));
        }
        rows.extend(folder_entries(
            Path::new(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp"),
            "Carpeta de inicio",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            rows.extend(desktop_entries(&Path::new(&home).join(".config/autostart")));
        }
        rows.extend(desktop_entries(Path::new("/etc/xdg/autostart")));
    }
    rows.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    rows
}

#[cfg(windows)]
fn query_run(key: &str, source: &str) -> Vec<StartupRow> {
    let output = std::process::Command::new("reg")
        .args(["query", key])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_reg_query(&String::from_utf8_lossy(&output.stdout), source)
}

#[cfg(windows)]
fn folder_entries(folder: &Path, source: &str) -> Vec<StartupRow> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            Some(StartupRow {
                name,
                source: source.to_string(),
                enabled: true,
                path: entry.path().display().to_string(),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn desktop_entries(folder: &Path) -> Vec<StartupRow> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            Some(parse_desktop(&text, &path.display().to_string()))
        })
        .collect()
}

pub fn parse_reg_query(text: &str, source: &str) -> Vec<StartupRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let index = line.find("REG_")?;
            let name = line[..index].trim();
            if name.is_empty() || name.starts_with("HKEY_") {
                return None;
            }
            let after = line[index..].trim();
            let mut parts = after.splitn(2, char::is_whitespace);
            let _kind = parts.next().unwrap_or("");
            let command = parts.next().unwrap_or("").trim();
            Some(StartupRow {
                name: name.to_string(),
                source: source.to_string(),
                enabled: true,
                path: prepare_icon_path(command),
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_desktop(text: &str, fallback: &str) -> StartupRow {
    let mut name = None;
    let mut icon = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Name=") {
            if name.is_none() && !value.is_empty() {
                name = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Icon=") {
            if icon.is_none() && !value.is_empty() {
                icon = Some(value.to_string());
            }
        } else if line.eq_ignore_ascii_case("Hidden=true")
            || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        {
            hidden = true;
        }
    }
    StartupRow {
        name: name.unwrap_or_else(|| fallback.to_string()),
        source: "autostart".to_string(),
        enabled: !hidden,
        path: icon.unwrap_or_default(),
    }
}

pub fn command_target(command: &str) -> String {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    let lower = command.to_ascii_lowercase();
    if let Some(index) = lower.find(".exe") {
        return command[..index + 4].trim().to_string();
    }
    command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

pub fn system_root_path(path: &str, root: &str) -> String {
    const PREFIX: &str = "\\systemroot";
    if path.len() >= PREFIX.len() && path[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        format!("{root}{}", &path[PREFIX.len()..])
    } else {
        path.to_string()
    }
}

pub fn prepare_icon_path(command: &str) -> String {
    let expanded = expand_env(command.trim());
    system_root_path(&command_target(&expanded), &system_root())
}

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
}

#[cfg(windows)]
fn expand_env(value: &str) -> String {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::core::PCWSTR;
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;

    let wide: Vec<u16> = std::ffi::OsStr::new(value).encode_wide().chain([0]).collect();
    let mut buffer = vec![0u16; 1024];
    let written = unsafe { ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) };
    if written == 0 {
        return value.to_string();
    }
    let end = (written as usize).saturating_sub(1).min(buffer.len());
    OsString::from_wide(&buffer[..end]).to_string_lossy().into_owned()
}

#[cfg(not(windows))]
fn expand_env(value: &str) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::{command_target, parse_desktop, parse_reg_query, system_root_path};

    #[test]
    fn reads_registry_run_values() {
        let rows = parse_reg_query(
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n    OneDrive    REG_SZ    C:\\OneDrive.exe\n    Steam    REG_SZ    \"C:\\Program Files\\Steam\\steam.exe\" -silent\n",
            "Registro",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "OneDrive");
        assert_eq!(rows[0].path, r"C:\OneDrive.exe");
        assert!(rows[0].enabled);
        assert_eq!(rows[1].name, "Steam");
        assert_eq!(rows[1].path, r"C:\Program Files\Steam\steam.exe");
    }

    #[test]
    fn reads_desktop_autostart() {
        let row = parse_desktop(
            "[Desktop Entry]\nName=Steam\nIcon=steam\nHidden=true\n",
            "steam.desktop",
        );
        assert_eq!(row.name, "Steam");
        assert_eq!(row.path, "steam");
        assert!(!row.enabled);
    }

    #[test]
    fn extracts_the_executable_from_a_command() {
        assert_eq!(
            command_target(r#""C:\Program Files\Steam\steam.exe" -silent"#),
            r"C:\Program Files\Steam\steam.exe"
        );
        assert_eq!(
            command_target(r"C:\Windows\System32\svchost.exe -k netsvcs"),
            r"C:\Windows\System32\svchost.exe"
        );
        assert_eq!(
            system_root_path(r"\SystemRoot\System32\svchost.exe", r"C:\Windows"),
            r"C:\Windows\System32\svchost.exe"
        );
    }
}
