#[cfg(target_os = "linux")]
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub name: String,
    pub display: String,
    pub state: String,
    pub running: bool,
    pub pid: u32,
}

pub fn sample_services() -> (Vec<ServiceRow>, Option<String>) {
    match collect_services() {
        Ok(mut rows) => {
            rows.sort_by(|left, right| {
                right
                    .running
                    .cmp(&left.running)
                    .then(left.display.to_ascii_lowercase().cmp(&right.display.to_ascii_lowercase()))
            });
            (rows, None)
        }
        Err(error) => (Vec::new(), Some(error)),
    }
}

pub fn start_service(name: &str) -> Result<(), String> {
    control_service(name, true)
}

pub fn stop_service(name: &str) -> Result<(), String> {
    control_service(name, false)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_systemctl(text: &str) -> Vec<ServiceRow> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?.to_string();
            if name.eq_ignore_ascii_case("unit") {
                return None;
            }
            let _load = parts.next()?;
            let active = parts.next()?;
            let sub = parts.next()?;
            let display = {
                let rest: Vec<&str> = parts.collect();
                if rest.is_empty() {
                    name.clone()
                } else {
                    rest.join(" ")
                }
            };
            let (state, running) = linux_state(active, sub);
            Some(ServiceRow {
                name,
                display,
                state,
                running,
                pid: 0,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_state(active: &str, sub: &str) -> (String, bool) {
    let state = match (active, sub) {
        ("active", "running") => "en ejecución",
        ("active", "exited") => "terminó",
        ("failed", _) => "falló",
        ("activating", _) => "iniciando",
        ("deactivating", _) => "deteniendo",
        ("inactive", _) | ("dead", _) => "detenido",
        _ => return (format!("{active} {sub}"), false),
    };
    (state.to_string(), sub == "running")
}

#[cfg(target_os = "linux")]
fn collect_services() -> Result<Vec<ServiceRow>, String> {
    let text = run_command(
        "systemctl",
        &[
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--plain",
            "--no-pager",
        ],
    )?;
    Ok(parse_systemctl(&text))
}

#[cfg(target_os = "linux")]
fn control_service(name: &str, start: bool) -> Result<(), String> {
    let verb = if start { "start" } else { "stop" };
    run_command("systemctl", &[verb, name]).map(|_| ())
}

#[cfg(windows)]
fn collect_services() -> Result<Vec<ServiceRow>, String> {
    use windows::Win32::System::Services::{
        EnumServicesStatusExW, OpenSCManagerW, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT,
        SC_MANAGER_ENUMERATE_SERVICE, SERVICE_STATE_ALL, SERVICE_WIN32,
        ENUM_SERVICE_STATUS_PROCESSW,
    };
    use windows::core::PCWSTR;

    unsafe {
        let manager = OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
        .map_err(|error| error.to_string())?;
        let mut buf = vec![0u64; 8 * 1024];
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut resume = 0u32;
        let mut rows = Vec::new();
        loop {
            let bytes = buf.len() * std::mem::size_of::<u64>();
            let slice = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, bytes);
            let status = EnumServicesStatusExW(
                manager,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(slice),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            );
            if let Err(error) = status {
                if needed as usize > bytes && (needed as usize) < 16 * 1024 * 1024 {
                    buf.resize(needed as usize / 8 + 8, 0);
                    resume = 0;
                    continue;
                }
                return Err(error.to_string());
            }
            let entries = std::slice::from_raw_parts(
                buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
                count as usize,
            );
            for entry in entries {
                let name = wide(entry.lpServiceName);
                if name.is_empty() {
                    continue;
                }
                let display_name = wide(entry.lpDisplayName);
                let (state, running) = windows_state(entry.ServiceStatusProcess.dwCurrentState.0);
                rows.push(ServiceRow {
                    display: if display_name.is_empty() {
                        name.clone()
                    } else {
                        display_name
                    },
                    name,
                    state,
                    running,
                    pid: entry.ServiceStatusProcess.dwProcessId,
                });
            }
            if resume == 0 {
                break;
            }
        }
        Ok(rows)
    }
}

#[cfg(windows)]
fn windows_state(code: u32) -> (String, bool) {
    let (state, running) = match code {
        1 => ("detenido", false),
        2 => ("iniciando", false),
        3 => ("deteniendo", true),
        4 => ("en ejecución", true),
        5 => ("reanudando", true),
        6 => ("pausando", true),
        7 => ("pausado", false),
        _ => ("desconocido", false),
    };
    (state.to_string(), running)
}

#[cfg(windows)]
fn wide(value: windows::core::PWSTR) -> String {
    if value.0.is_null() {
        return String::new();
    }
    unsafe { value.to_string().unwrap_or_default() }
}

#[cfg(windows)]
fn control_service(name: &str, start: bool) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Services::{
        ControlService, OpenSCManagerW, OpenServiceW, StartServiceW, SC_MANAGER_CONNECT,
        SERVICE_CONTROL_STOP, SERVICE_START, SERVICE_STATUS, SERVICE_STOP,
    };
    use windows::core::PCWSTR;

    let mut wide_name: Vec<u16> = OsStr::new(name).encode_wide().chain([0]).collect();
    unsafe {
        let manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
            .map_err(|error| error.to_string())?;
        let access = if start { SERVICE_START } else { SERVICE_STOP };
        let service = OpenServiceW(manager, PCWSTR(wide_name.as_mut_ptr()), access)
            .map_err(|error| error.to_string())?;
        if start {
            StartServiceW(service, None).map_err(|error| error.to_string())
        } else {
            let mut status = SERVICE_STATUS::default();
            ControlService(service, SERVICE_CONTROL_STOP, &mut status)
                .map_err(|error| error.to_string())
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn collect_services() -> Result<Vec<ServiceRow>, String> {
    Err("esta plataforma no expone servicios".to_string())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn control_service(_name: &str, _start: bool) -> Result<(), String> {
    Err("esta plataforma no expone servicios".to_string())
}

#[cfg(target_os = "linux")]
fn run_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        let error = error.trim();
        if error.is_empty() {
            Err(format!("{program} terminó con {}", output.status))
        } else {
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_systemctl;

    #[test]
    fn parses_systemctl_units() {
        let rows = parse_systemctl(
            "accounts-daemon.service loaded active running Accounts Service\n\
             ssh.service loaded inactive dead OpenSSH server\n\
             UNIT LOAD ACTIVE SUB DESCRIPTION\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "accounts-daemon.service");
        assert_eq!(rows[0].display, "Accounts Service");
        assert!(rows[0].running);
        assert_eq!(rows[0].state, "en ejecución");
        assert!(!rows[1].running);
        assert_eq!(rows[1].state, "detenido");
    }
}
