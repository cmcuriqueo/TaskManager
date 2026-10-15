mod actions;
mod gpu;
mod gui;
mod monitor;
mod services;
mod sessions;
mod startup;
mod tui;

use std::io::{self, Write};
use std::process::ExitCode;

use monitor::{SortKey, SystemMonitor, format_bytes, format_rate, sort_processes};

fn main() -> ExitCode {
    match parse_args() {
        Ok(Mode::Help) => {
            print_usage();
            ExitCode::SUCCESS
        }
        Ok(Mode::Once) => match print_once() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("taskmgr: {err}");
                ExitCode::from(1)
            }
        },
        Ok(Mode::Tui) => match tui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("taskmgr: {err}");
                ExitCode::from(1)
            }
        },
        Ok(Mode::Gui) => match gui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("taskmgr: {err}");
                eprintln!("para usar la terminal: taskmgr --tui");
                ExitCode::from(1)
            }
        },
        Err(message) => {
            eprintln!("taskmgr: {message}");
            print_usage();
            ExitCode::from(2)
        }
    }
}

enum Mode {
    Gui,
    Tui,
    Once,
    Help,
}

fn parse_args() -> Result<Mode, String> {
    let mut tui = false;
    let mut once = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-t" | "--tui" | "-ti" => tui = true,
            "--once" => once = true,
            "-h" | "--help" => return Ok(Mode::Help),
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }
    match (tui, once) {
        (true, true) => Err("usá --tui o --once por separado".to_string()),
        (false, true) => Ok(Mode::Once),
        (true, false) => Ok(Mode::Tui),
        (false, false) => Ok(Mode::Gui),
    }
}

fn print_usage() {
    eprintln!("uso: taskmgr [--tui] [--once]");
    eprintln!("  (sin flags)  abre la ventana");
    eprintln!("  -t, --tui    abre la vista en la terminal");
    eprintln!("  --once       imprime un muestreo y termina");
}

fn print_once() -> io::Result<()> {
    let monitor = SystemMonitor::open();
    let mut snapshot = monitor.latest().clone();
    sort_processes(&mut snapshot.processes, SortKey::Cpu);
    let summary = &snapshot.summary;
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "CPU  {:>5.1}%\nRAM  {} / {} ({:.1}%)\nGPU  {}\nDisco  {} lectura  {} escritura\nRed  {} recepción  {} envío\nProcesos  {}\n",
        summary.cpu_percent,
        format_bytes(summary.memory_used),
        format_bytes(summary.memory_total),
        memory_percent(summary.memory_used, summary.memory_total),
        summary
            .gpu_percent
            .map(|percent| format!("{percent:.0}%"))
            .unwrap_or_else(|| "—".to_string()),
        format_rate(summary.disk_read_per_sec),
        format_rate(summary.disk_write_per_sec),
        format_rate(summary.net_received_per_sec),
        format_rate(summary.net_transmitted_per_sec),
        summary.process_count
    )?;
    writeln!(
        out,
        "{:<16} {:<16} {:<8} {:>10} {:>10} {:>5} {:>12} {:>12}",
        "MONTAJE", "TIPO", "FS", "USADO", "TOTAL", "%", "LECTURA", "ESCRITURA"
    )?;
    for disk in &snapshot.disks {
        let percent = if disk.total == 0 {
            0.0
        } else {
            disk.used as f64 / disk.total as f64 * 100.0
        };
        writeln!(
            out,
            "{:<16} {:<16} {:<8} {:>10} {:>10} {:>4.0}% {:>12} {:>12}",
            clip(&disk.mount, 16),
            clip(&disk.kind, 16),
            clip(&disk.file_system, 8),
            format_bytes(disk.used),
            format_bytes(disk.total),
            percent,
            format_rate(disk.read_per_sec),
            format_rate(disk.write_per_sec)
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{:<24} {:>14} {:>14} {:>16} {:>16}",
        "INTERFAZ", "RECEPCIÓN", "ENVÍO", "TOTAL RECIBIDO", "TOTAL ENVIADO"
    )?;
    for network in &snapshot.networks {
        writeln!(
            out,
            "{:<24} {:>14} {:>14} {:>16} {:>16}",
            clip(&network.name, 24),
            format_rate(network.received_per_sec),
            format_rate(network.transmitted_per_sec),
            format_bytes(network.total_received),
            format_bytes(network.total_transmitted)
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{:<32} {:>8} {:>12} {:>12}",
        "GPU", "USO", "MEMORIA", "TOTAL"
    )?;
    for gpu in &snapshot.gpus {
        writeln!(
            out,
            "{:<32} {:>8} {:>12} {:>12}",
            clip(&gpu.name, 32),
            gpu.utilization
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "—".to_string()),
            gpu.memory_used
                .map(format_bytes)
                .unwrap_or_else(|| "—".to_string()),
            gpu.memory_total
                .map(format_bytes)
                .unwrap_or_else(|| "—".to_string())
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{:<8} {:<28} {:>8} {:>12} {:<14} {}",
        "PID", "NOMBRE", "CPU %", "MEMORIA", "ESTADO", "USUARIO"
    )?;
    for process in &snapshot.processes {
        writeln!(
            out,
            "{:<8} {:<28} {:>8.1} {:>12} {:<14} {}",
            process.pid,
            clip(&process.name, 28),
            process.cpu,
            format_bytes(process.memory),
            process.status,
            process.user
        )?;
    }
    writeln!(out)?;
    let running = snapshot.services.iter().filter(|service| service.running).count();
    match &snapshot.services_note {
        Some(note) => writeln!(out, "Servicios  {note}")?,
        None => writeln!(
            out,
            "Servicios  {running} en ejecución / {}",
            snapshot.services.len()
        )?,
    }
    for service in snapshot.services.iter().filter(|service| service.running) {
        writeln!(
            out,
            "  {:<40} {}",
            clip(&service.display, 40),
            service.state
        )?;
    }
    writeln!(out)?;
    match &snapshot.sessions_note {
        Some(note) => writeln!(out, "Sesiones  {note}")?,
        None => writeln!(out, "Sesiones  {}", snapshot.sessions.len())?,
    }
    for session in &snapshot.sessions {
        writeln!(
            out,
            "  {:<8} {:<24} {:<16} {}",
            clip(&session.id, 8),
            clip(&session.user, 24),
            clip(&session.state, 16),
            session.place
        )?;
    }
    Ok(())
}

fn memory_percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

fn clip(value: &str, width: usize) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if out.chars().count() == width.saturating_sub(1) && value.chars().count() > width {
            out.push('…');
            return out;
        }
        if out.chars().count() == width {
            break;
        }
        out.push(ch);
    }
    out
}
