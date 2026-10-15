#[cfg(target_os = "linux")]
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub user: String,
    pub state: String,
    pub place: String,
}

pub fn sample_sessions() -> (Vec<SessionRow>, Option<String>) {
    match collect_sessions() {
        Ok(rows) => (rows, None),
        Err(error) => (Vec::new(), Some(error)),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_loginctl(text: &str) -> Vec<SessionRow> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next()?;
            if id.eq_ignore_ascii_case("session") {
                return None;
            }
            let _uid = parts.next()?;
            let user = parts.next().unwrap_or("-").to_string();
            let seat = parts.next().unwrap_or("-");
            let tty = parts.next().unwrap_or("-");
            let state = parts.next().unwrap_or("-");
            let place = if tty != "-" { tty } else { seat }.to_string();
            Some(SessionRow {
                id: id.to_string(),
                user,
                state: session_state(state),
                place,
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn session_state(state: &str) -> String {
    match state {
        "active" => "activa".to_string(),
        "online" => "en línea".to_string(),
        "closing" => "cerrando".to_string(),
        "disconnected" => "desconectada".to_string(),
        other => other.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn collect_sessions() -> Result<Vec<SessionRow>, String> {
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend", "--no-pager"])
        .output()
        .map_err(|error| format!("loginctl: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let error = error.trim();
        return Err(if error.is_empty() {
            format!("loginctl terminó con {}", output.status)
        } else {
            error.to_string()
        });
    }
    Ok(parse_loginctl(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(windows)]
fn collect_sessions() -> Result<Vec<SessionRow>, String> {
    use windows::Win32::System::RemoteDesktop::{
        WTSEnumerateSessionsW, WTSFreeMemory,
    };

    unsafe {
        let mut info = std::ptr::null_mut();
        let mut count = 0u32;
        WTSEnumerateSessionsW(None, 0, 1, &mut info, &mut count).map_err(|error| error.to_string())?;
        let entries = std::slice::from_raw_parts(info, count as usize);
        let mut rows = Vec::with_capacity(entries.len());
        for entry in entries {
            rows.push(session_from_info(entry));
        }
        WTSFreeMemory(info as *mut _);
        Ok(rows)
    }
}

#[cfg(windows)]
fn session_from_info(entry: &windows::Win32::System::RemoteDesktop::WTS_SESSION_INFOW) -> SessionRow {
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSUserName,
    };

    let user = unsafe {
        let mut buffer = windows::core::PWSTR::null();
        let mut bytes = 0u32;
        let user = if WTSQuerySessionInformationW(None, entry.SessionId, WTSUserName, &mut buffer, &mut bytes).is_ok()
        {
            let user = wide(buffer);
            if !buffer.0.is_null() {
                WTSFreeMemory(buffer.0 as *mut _);
            }
            user
        } else {
            String::new()
        };
        user
    };
    SessionRow {
        id: entry.SessionId.to_string(),
        user: if user.is_empty() {
            "—".to_string()
        } else {
            user
        },
        state: windows_session_state(entry.State.0),
        place: wide(entry.pWinStationName),
    }
}

#[cfg(windows)]
fn windows_session_state(state: i32) -> String {
    match state {
        0 => "activa",
        1 => "conectada",
        4 => "desconectada",
        5 => "inactiva",
        6 => "escucha",
        _ => "otra",
    }
    .to_string()
}

#[cfg(windows)]
fn wide(value: windows::core::PWSTR) -> String {
    if value.0.is_null() {
        return String::new();
    }
    unsafe { value.to_string().unwrap_or_default() }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn collect_sessions() -> Result<Vec<SessionRow>, String> {
    Err("esta plataforma no expone sesiones".to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_loginctl;

    #[test]
    fn parses_loginctl_sessions() {
        let rows = parse_loginctl(
            "SESSION UID USER SEAT TTY STATE\n\
             1 1000 matias seat0 tty2 active\n\
             c2 1000 matias - - online\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "1");
        assert_eq!(rows[0].user, "matias");
        assert_eq!(rows[0].place, "tty2");
        assert_eq!(rows[0].state, "activa");
        assert_eq!(rows[1].place, "-");
        assert_eq!(rows[1].state, "en línea");
    }
}
