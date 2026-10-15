use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupRow {
    pub name: String,
    pub source: String,
    pub enabled: bool,
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
            Some(StartupRow {
                name: name.to_string(),
                source: source.to_string(),
                enabled: true,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_desktop(text: &str, fallback: &str) -> StartupRow {
    let mut name = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Name=") {
            if name.is_none() && !value.is_empty() {
                name = Some(value.to_string());
            }
        } else if line.eq_ignore_ascii_case("Hidden=true") || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        {
            hidden = true;
        }
    }
    StartupRow {
        name: name.unwrap_or_else(|| fallback.to_string()),
        source: "autostart".to_string(),
        enabled: !hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_desktop, parse_reg_query};

    #[test]
    fn reads_registry_run_values() {
        let rows = parse_reg_query(
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\n    OneDrive    REG_SZ    C:\\OneDrive.exe\n    Steam    REG_SZ    C:\\Steam.exe\n",
            "Registro",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "OneDrive");
        assert!(rows[0].enabled);
        assert_eq!(rows[1].name, "Steam");
    }

    #[test]
    fn reads_desktop_autostart() {
        let row = parse_desktop(
            "[Desktop Entry]\nName=Steam\nHidden=true\n",
            "steam.desktop",
        );
        assert_eq!(row.name, "Steam");
        assert!(!row.enabled);
    }
}
