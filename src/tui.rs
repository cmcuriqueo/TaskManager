use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::monitor::{Snapshot, SortKey, SystemMonitor, format_bytes, format_rate, sort_processes};

pub fn run() -> io::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut app = App::open();
    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;
        if event::poll(Duration::from_secs(1))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && app.handle_key(key.code) {
                    break;
                }
            }
        } else {
            app.refresh();
        }
    }
    Ok(())
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    fn draw<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(render)?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Processes,
    Disks,
    Networks,
    Gpu,
    Services,
    Sessions,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::Processes => "Procesos",
            Self::Disks => "Discos",
            Self::Networks => "Red",
            Self::Gpu => "GPU",
            Self::Services => "Servicios",
            Self::Sessions => "Sesiones",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Processes => Self::Disks,
            Self::Disks => Self::Networks,
            Self::Networks => Self::Gpu,
            Self::Gpu => Self::Services,
            Self::Services => Self::Sessions,
            Self::Sessions => Self::Processes,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Processes => Self::Sessions,
            Self::Sessions => Self::Services,
            Self::Services => Self::Gpu,
            Self::Gpu => Self::Networks,
            Self::Networks => Self::Disks,
            Self::Disks => Self::Processes,
        }
    }
}

enum Pending {
    Terminate(u32),
    Suspend(u32),
    Resume(u32),
    StartService(String),
    StopService(String),
}

struct App {
    monitor: SystemMonitor,
    snapshot: Snapshot,
    page: Page,
    sort: SortKey,
    process_table: TableState,
    disk_table: TableState,
    network_table: TableState,
    gpu_table: TableState,
    service_table: TableState,
    session_table: TableState,
    selected_pid: Option<u32>,
    selected_mount: Option<String>,
    selected_interface: Option<String>,
    selected_gpu: Option<String>,
    selected_service: Option<String>,
    pending: Option<Pending>,
    notice: Option<String>,
}

impl App {
    fn open() -> Self {
        let monitor = SystemMonitor::open();
        let mut app = Self {
            snapshot: monitor.latest().clone(),
            monitor,
            page: Page::Processes,
            sort: SortKey::Cpu,
            process_table: TableState::default(),
            disk_table: TableState::default(),
            network_table: TableState::default(),
            gpu_table: TableState::default(),
            service_table: TableState::default(),
            session_table: TableState::default(),
            selected_pid: None,
            selected_mount: None,
            selected_interface: None,
            selected_gpu: None,
            selected_service: None,
            pending: None,
            notice: None,
        };
        sort_processes(&mut app.snapshot.processes, app.sort);
        app.restore_selection();
        app
    }

    fn refresh(&mut self) {
        self.snapshot = self.monitor.pull().clone();
        sort_processes(&mut self.snapshot.processes, self.sort);
        self.restore_selection();
    }

    fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.pending.is_some() {
            match code {
                KeyCode::Enter => {
                    if let Some(pending) = self.pending.take() {
                        self.notice = Some(run_pending(pending));
                        self.refresh();
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.pending = None;
                    self.notice = Some("cancelado".to_string());
                }
                _ => {}
            }
            return false;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Tab => self.page = self.page.next(),
            KeyCode::BackTab => self.page = self.page.previous(),
            KeyCode::Char('1') => self.page = Page::Processes,
            KeyCode::Char('2') => self.page = Page::Disks,
            KeyCode::Char('3') => self.page = Page::Networks,
            KeyCode::Char('4') => self.page = Page::Gpu,
            KeyCode::Char('5') => self.page = Page::Services,
            KeyCode::Char('6') => self.page = Page::Sessions,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('c') if self.page == Page::Processes => self.set_sort(SortKey::Cpu),
            KeyCode::Char('m') if self.page == Page::Processes => self.set_sort(SortKey::Memory),
            KeyCode::Char('n') if self.page == Page::Processes => self.set_sort(SortKey::Name),
            KeyCode::Char('p') if self.page == Page::Processes => self.set_sort(SortKey::Pid),
            KeyCode::Char('x') if self.page == Page::Processes => self.arm_process(Pending::Terminate),
            KeyCode::Char('z') if self.page == Page::Processes => self.arm_process(Pending::Suspend),
            KeyCode::Char('r') if self.page == Page::Processes => self.arm_process(Pending::Resume),
            KeyCode::Char('i') if self.page == Page::Services => self.arm_service(true),
            KeyCode::Char('t') if self.page == Page::Services => self.arm_service(false),
            _ => {}
        }
        false
    }

    fn arm_process(&mut self, kind: fn(u32) -> Pending) {
        let Some(pid) = self.selected_pid else {
            self.notice = Some("elegí un proceso".to_string());
            return;
        };
        let pending = kind(pid);
        self.notice = Some(pending_label(&pending));
        self.pending = Some(pending);
    }

    fn arm_service(&mut self, start: bool) {
        let Some(name) = self.selected_service.clone() else {
            self.notice = Some("elegí un servicio".to_string());
            return;
        };
        let pending = if start {
            Pending::StartService(name)
        } else {
            Pending::StopService(name)
        };
        self.notice = Some(pending_label(&pending));
        self.pending = Some(pending);
    }

    fn set_sort(&mut self, sort: SortKey) {
        if self.sort == sort {
            return;
        }
        self.sort = sort;
        sort_processes(&mut self.snapshot.processes, self.sort);
        self.restore_selection();
    }

    fn move_selection(&mut self, delta: isize) {
        match self.page {
            Page::Processes => {
                let current = self.process_table.selected();
                let len = self.snapshot.processes.len();
                self.selected_pid = move_row(&mut self.process_table, len, current, delta)
                    .and_then(|index| self.snapshot.processes.get(index).map(|row| row.pid));
            }
            Page::Disks => {
                let current = self.disk_table.selected();
                let len = self.snapshot.disks.len();
                self.selected_mount = move_row(&mut self.disk_table, len, current, delta)
                    .and_then(|index| self.snapshot.disks.get(index).map(|row| row.mount.clone()));
            }
            Page::Networks => {
                let current = self.network_table.selected();
                let len = self.snapshot.networks.len();
                self.selected_interface = move_row(&mut self.network_table, len, current, delta)
                    .and_then(|index| {
                        self.snapshot
                            .networks
                            .get(index)
                            .map(|row| row.name.clone())
                    });
            }
            Page::Gpu => {
                let current = self.gpu_table.selected();
                let len = self.snapshot.gpus.len();
                self.selected_gpu = move_row(&mut self.gpu_table, len, current, delta)
                    .and_then(|index| self.snapshot.gpus.get(index).map(|row| row.name.clone()));
            }
            Page::Services => {
                let current = self.service_table.selected();
                let len = self.snapshot.services.len();
                self.selected_service = move_row(&mut self.service_table, len, current, delta)
                    .and_then(|index| {
                        self.snapshot
                            .services
                            .get(index)
                            .map(|row| row.name.clone())
                    });
            }
            Page::Sessions => {
                let current = self.session_table.selected();
                let len = self.snapshot.sessions.len();
                let _ = move_row(&mut self.session_table, len, current, delta);
            }
        }
    }

    fn restore_selection(&mut self) {
        let process_index = self.selected_pid.and_then(|pid| {
            self.snapshot
                .processes
                .iter()
                .position(|row| row.pid == pid)
        });
        self.selected_pid = select_row(
            &mut self.process_table,
            self.snapshot.processes.len(),
            process_index,
        )
        .and_then(|index| self.snapshot.processes.get(index).map(|row| row.pid));

        let disk_index = self.selected_mount.as_ref().and_then(|mount| {
            self.snapshot
                .disks
                .iter()
                .position(|row| &row.mount == mount)
        });
        self.selected_mount =
            select_row(&mut self.disk_table, self.snapshot.disks.len(), disk_index)
                .and_then(|index| self.snapshot.disks.get(index).map(|row| row.mount.clone()));

        let network_index = self.selected_interface.as_ref().and_then(|name| {
            self.snapshot
                .networks
                .iter()
                .position(|row| &row.name == name)
        });
        self.selected_interface = select_row(
            &mut self.network_table,
            self.snapshot.networks.len(),
            network_index,
        )
        .and_then(|index| {
            self.snapshot
                .networks
                .get(index)
                .map(|row| row.name.clone())
        });

        let gpu_index = self
            .selected_gpu
            .as_ref()
            .and_then(|name| self.snapshot.gpus.iter().position(|row| &row.name == name));
        self.selected_gpu = select_row(&mut self.gpu_table, self.snapshot.gpus.len(), gpu_index)
            .and_then(|index| self.snapshot.gpus.get(index).map(|row| row.name.clone()));

        let service_index = self.selected_service.as_ref().and_then(|name| {
            self.snapshot
                .services
                .iter()
                .position(|row| &row.name == name)
        });
        self.selected_service = select_row(
            &mut self.service_table,
            self.snapshot.services.len(),
            service_index,
        )
        .and_then(|index| {
            self.snapshot
                .services
                .get(index)
                .map(|row| row.name.clone())
        });

        let session_index = self.session_table.selected();
        let _ = select_row(
            &mut self.session_table,
            self.snapshot.sessions.len(),
            session_index,
        );
    }
}

fn pending_label(pending: &Pending) -> String {
    match pending {
        Pending::Terminate(pid) => format!("¿Terminar el proceso {pid}? Enter confirma, Esc cancela"),
        Pending::Suspend(pid) => format!("¿Suspender el proceso {pid}? Enter confirma, Esc cancela"),
        Pending::Resume(pid) => format!("¿Reanudar el proceso {pid}? Enter confirma, Esc cancela"),
        Pending::StartService(name) => {
            format!("¿Iniciar el servicio {name}? Enter confirma, Esc cancela")
        }
        Pending::StopService(name) => {
            format!("¿Detener el servicio {name}? Enter confirma, Esc cancela")
        }
    }
}

fn run_pending(pending: Pending) -> String {
    let result = match &pending {
        Pending::Terminate(pid) => crate::actions::terminate_process(*pid),
        Pending::Suspend(pid) => crate::actions::suspend_process(*pid),
        Pending::Resume(pid) => crate::actions::resume_process(*pid),
        Pending::StartService(name) => crate::services::start_service(name),
        Pending::StopService(name) => crate::services::stop_service(name),
    };
    match result {
        Ok(()) => match pending {
            Pending::Terminate(pid) => format!("proceso {pid} terminado"),
            Pending::Suspend(pid) => format!("proceso {pid} suspendido"),
            Pending::Resume(pid) => format!("proceso {pid} reanudado"),
            Pending::StartService(name) => format!("servicio {name} iniciado"),
            Pending::StopService(name) => format!("servicio {name} detenido"),
        },
        Err(error) => error,
    }
}

fn move_row(
    table: &mut TableState,
    len: usize,
    current: Option<usize>,
    delta: isize,
) -> Option<usize> {
    if len == 0 {
        table.select(None);
        return None;
    }
    let current = current.unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    table.select(Some(next));
    Some(next)
}

fn select_row(table: &mut TableState, len: usize, preferred: Option<usize>) -> Option<usize> {
    if len == 0 {
        table.select(None);
        return None;
    }
    let index = preferred.unwrap_or(0).min(len - 1);
    table.select(Some(index));
    Some(index)
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let summary = &app.snapshot.summary;
    let memory = if summary.memory_total == 0 {
        format_bytes(summary.memory_used)
    } else {
        let percent = summary.memory_used as f64 / summary.memory_total as f64 * 100.0;
        format!(
            "{} / {} ({percent:.1}%)",
            format_bytes(summary.memory_used),
            format_bytes(summary.memory_total)
        )
    };
    let gpu = match summary.gpu_percent {
        Some(percent) => format!("{percent:.0}%"),
        None => "—".to_string(),
    };
    let headline = format!(
        "CPU {:>5.1}%    RAM {memory}    GPU {gpu}    Procesos {}",
        summary.cpu_percent, summary.process_count
    );
    let activity = format!(
        "Disco {} lectura  {} escritura    Red {} recepción  {} envío    Página {}",
        format_rate(summary.disk_read_per_sec),
        format_rate(summary.disk_write_per_sec),
        format_rate(summary.net_received_per_sec),
        format_rate(summary.net_transmitted_per_sec),
        app.page.label()
    );
    frame.render_widget(
        Paragraph::new(format!("{headline}\n{activity}")).block(Block::bordered().title("taskmgr")),
        header,
    );

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let highlight = Style::default().add_modifier(Modifier::REVERSED);
    match app.page {
        Page::Processes => {
            let rows = app.snapshot.processes.iter().map(|process| {
                Row::new([
                    Cell::from(process.pid.to_string()),
                    Cell::from(process.name.as_str()),
                    Cell::from(format!("{:.1}", process.cpu)),
                    Cell::from(format_bytes(process.memory)),
                    Cell::from(process.status),
                    Cell::from(process.user.as_str()),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Length(8),
                    Constraint::Min(16),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Length(14),
                    Constraint::Min(10),
                ],
            )
            .header(
                Row::new(["PID", "Nombre", "CPU %", "Memoria", "Estado", "Usuario"]).style(bold),
            )
            .block(Block::bordered().title(format!("Procesos · {}", app.sort.label())))
            .row_highlight_style(highlight)
            .highlight_symbol("› ");
            frame.render_stateful_widget(table, body, &mut app.process_table);
        }
        Page::Disks => {
            let rows = app.snapshot.disks.iter().map(|disk| {
                let percent = if disk.total == 0 {
                    0.0
                } else {
                    disk.used as f64 / disk.total as f64 * 100.0
                };
                Row::new([
                    Cell::from(disk.mount.as_str()),
                    Cell::from(disk.kind.as_str()),
                    Cell::from(disk.file_system.as_str()),
                    Cell::from(format_bytes(disk.used)),
                    Cell::from(format_bytes(disk.total)),
                    Cell::from(format!("{percent:.0}%")),
                    Cell::from(format_rate(disk.read_per_sec)),
                    Cell::from(format_rate(disk.write_per_sec)),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Min(8),
                    Constraint::Length(16),
                    Constraint::Length(8),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Length(5),
                    Constraint::Length(12),
                    Constraint::Length(12),
                ],
            )
            .header(
                Row::new([
                    "Montaje",
                    "Tipo",
                    "FS",
                    "Usado",
                    "Total",
                    "%",
                    "Lectura",
                    "Escritura",
                ])
                .style(bold),
            )
            .block(Block::bordered().title("Discos"))
            .row_highlight_style(highlight)
            .highlight_symbol("› ");
            frame.render_stateful_widget(table, body, &mut app.disk_table);
        }
        Page::Networks => {
            let rows = app.snapshot.networks.iter().map(|network| {
                Row::new([
                    Cell::from(network.name.as_str()),
                    Cell::from(format_rate(network.received_per_sec)),
                    Cell::from(format_rate(network.transmitted_per_sec)),
                    Cell::from(format_bytes(network.total_received)),
                    Cell::from(format_bytes(network.total_transmitted)),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Min(16),
                    Constraint::Length(14),
                    Constraint::Length(14),
                    Constraint::Length(16),
                    Constraint::Length(16),
                ],
            )
            .header(
                Row::new([
                    "Interfaz",
                    "Recepción",
                    "Envío",
                    "Total recibido",
                    "Total enviado",
                ])
                .style(bold),
            )
            .block(Block::bordered().title("Red"))
            .row_highlight_style(highlight)
            .highlight_symbol("› ");
            frame.render_stateful_widget(table, body, &mut app.network_table);
        }
        Page::Gpu => {
            let rows = app.snapshot.gpus.iter().map(|gpu| {
                let used = gpu
                    .memory_used
                    .map(format_bytes)
                    .unwrap_or_else(|| "—".to_string());
                let total = gpu
                    .memory_total
                    .map(format_bytes)
                    .unwrap_or_else(|| "—".to_string());
                let utilization = gpu
                    .utilization
                    .map(|value| format!("{value:.0}%"))
                    .unwrap_or_else(|| "—".to_string());
                Row::new([
                    Cell::from(gpu.name.as_str()),
                    Cell::from(utilization),
                    Cell::from(used),
                    Cell::from(total),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Min(24),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Length(12),
                ],
            )
            .header(Row::new(["Nombre", "Uso", "Memoria", "Total"]).style(bold))
            .block(Block::bordered().title("GPU"))
            .row_highlight_style(highlight)
            .highlight_symbol("› ");
            frame.render_stateful_widget(table, body, &mut app.gpu_table);
        }
        Page::Services => {
            if let Some(note) = &app.snapshot.services_note {
                frame.render_widget(
                    Paragraph::new(note.as_str()).block(Block::bordered().title("Servicios")),
                    body,
                );
            } else {
                let rows = app.snapshot.services.iter().map(|service| {
                    Row::new([
                        Cell::from(service.display.as_str()),
                        Cell::from(service.name.as_str()),
                        Cell::from(service.state.as_str()),
                    ])
                });
                let table = Table::new(
                    rows,
                    [
                        Constraint::Min(24),
                        Constraint::Min(24),
                        Constraint::Length(16),
                    ],
                )
                .header(Row::new(["Nombre", "Servicio", "Estado"]).style(bold))
                .block(Block::bordered().title("Servicios"))
                .row_highlight_style(highlight)
                .highlight_symbol("› ");
                frame.render_stateful_widget(table, body, &mut app.service_table);
            }
        }
        Page::Sessions => {
            if let Some(note) = &app.snapshot.sessions_note {
                frame.render_widget(
                    Paragraph::new(note.as_str()).block(Block::bordered().title("Sesiones")),
                    body,
                );
            } else {
                let rows = app.snapshot.sessions.iter().map(|session| {
                    Row::new([
                        Cell::from(session.id.as_str()),
                        Cell::from(session.user.as_str()),
                        Cell::from(session.state.as_str()),
                        Cell::from(session.place.as_str()),
                    ])
                });
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(8),
                        Constraint::Min(16),
                        Constraint::Length(16),
                        Constraint::Min(12),
                    ],
                )
                .header(Row::new(["Id", "Usuario", "Estado", "Lugar"]).style(bold))
                .block(Block::bordered().title("Sesiones"))
                .row_highlight_style(highlight)
                .highlight_symbol("› ");
                frame.render_stateful_widget(table, body, &mut app.session_table);
            }
        }
    }

    let help = if let Some(notice) = &app.notice {
        notice.clone()
    } else {
        match app.page {
            Page::Processes => {
                "q salir  tab página  1-6  ↑↓  c/m/n/p orden  x terminar  z suspender  r reanudar"
                    .to_string()
            }
            Page::Services => {
                "q salir  tab página  1-6  ↑↓ mover  i iniciar  t detener".to_string()
            }
            _ => "q salir  tab página  1 procesos  2 discos  3 red  4 gpu  5 servicios  6 sesiones"
                .to_string(),
        }
    };
    frame.render_widget(Paragraph::new(help), footer);
}
