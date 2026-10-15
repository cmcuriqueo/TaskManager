pub fn terminate_process(pid: u32) -> Result<(), String> {
    ensure_actionable(pid)?;
    send(pid, Action::Terminate)
}

pub fn suspend_process(pid: u32) -> Result<(), String> {
    ensure_actionable(pid)?;
    send(pid, Action::Suspend)
}

pub fn resume_process(pid: u32) -> Result<(), String> {
    ensure_actionable(pid)?;
    send(pid, Action::Resume)
}

enum Action {
    Terminate,
    Suspend,
    Resume,
}

fn ensure_actionable(pid: u32) -> Result<(), String> {
    if pid == std::process::id() {
        return Err("no actúo sobre taskmgr".to_string());
    }
    if pid == 0 || (cfg!(target_os = "linux") && pid == 1) || (cfg!(windows) && pid == 4) {
        return Err("ese proceso está protegido".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn send(pid: u32, action: Action) -> Result<(), String> {
    let signal = match action {
        Action::Terminate => libc::SIGKILL,
        Action::Suspend => libc::SIGSTOP,
        Action::Resume => libc::SIGCONT,
    };
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

#[cfg(windows)]
fn send(pid: u32, action: Action) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, OpenThread, ResumeThread, SuspendThread, TerminateProcess,
        PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
    };

    unsafe {
        match action {
            Action::Terminate => {
                let process = OpenProcess(PROCESS_TERMINATE, false, pid)
                    .map_err(|error| error.to_string())?;
                let result = TerminateProcess(process, 1).map_err(|error| error.to_string());
                let _ = CloseHandle(process);
                result
            }
            Action::Suspend | Action::Resume => {
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
                    .map_err(|error| error.to_string())?;
                let mut entry = THREADENTRY32 {
                    dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                    ..Default::default()
                };
                if Thread32First(snapshot, &mut entry).is_err() {
                    let _ = CloseHandle(snapshot);
                    return Err("no se pudieron enumerar los hilos".to_string());
                }
                let mut changed = 0u32;
                loop {
                    if entry.th32OwnerProcessID == pid {
                        if let Ok(thread) =
                            OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                        {
                            let code = match action {
                                Action::Suspend => SuspendThread(thread),
                                Action::Resume => ResumeThread(thread),
                                Action::Terminate => u32::MAX,
                            };
                            if code != u32::MAX {
                                changed += 1;
                            }
                            let _ = CloseHandle(thread);
                        }
                    }
                    if Thread32Next(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
                let _ = CloseHandle(snapshot);
                if changed == 0 {
                    Err("no se pudo cambiar el estado del proceso".to_string())
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn send(_pid: u32, _action: Action) -> Result<(), String> {
    Err("esta plataforma no permite actuar sobre procesos".to_string())
}

#[cfg(test)]
mod tests {
    use super::ensure_actionable;

    #[test]
    fn refuses_to_act_on_itself() {
        let error = ensure_actionable(std::process::id()).unwrap_err();
        assert!(error.contains("taskmgr"));
    }
}
