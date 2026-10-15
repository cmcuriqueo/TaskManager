use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Shape, Stroke, TextEdit, pos2, vec2,
};

use crate::monitor::{ProcessRow, Snapshot, SystemMonitor};
use crate::startup::StartupRow;

const BG: Color32 = Color32::from_rgb(32, 32, 32);
const TEXT: Color32 = Color32::from_rgb(243, 243, 243);
const MUTED: Color32 = Color32::from_rgb(166, 166, 166);
const SELECTED: Color32 = Color32::from_rgb(55, 55, 55);
const HOVER: Color32 = Color32::from_rgb(45, 45, 45);
const HEAT_BG: Color32 = Color32::from_rgb(17, 28, 46);
const TEAL: Color32 = Color32::from_rgb(78, 196, 206);
const HISTORY: usize = 60;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 640.0])
            .with_title("Administrador de tareas"),
        ..Default::default()
    };
    eframe::run_native(
        "Administrador de tareas",
        options,
        Box::new(|cc| Ok(Box::new(TaskApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Processes,
    Performance,
    History,
    Startup,
    Users,
    Details,
    Services,
    Settings,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::Processes => "Procesos",
            Self::Performance => "Rendimiento",
            Self::History => "Historial de aplicaciones",
            Self::Startup => "Aplicaciones de arranque",
            Self::Users => "Usuarios",
            Self::Details => "Detalles",
            Self::Services => "Servicios",
            Self::Settings => "Configuración",
        }
    }

    fn all() -> &'static [Page] {
        &[
            Self::Processes,
            Self::Performance,
            Self::History,
            Self::Startup,
            Self::Users,
            Self::Details,
            Self::Services,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Name,
    Cpu,
    Memory,
    Disk,
}

#[derive(Clone, PartialEq, Eq)]
enum Meter {
    Cpu,
    Memory,
    Disk(String),
    Net(String),
    Gpu(String),
}

#[derive(Clone)]
enum Pending {
    Terminate(u32),
    Suspend(u32),
    Resume(u32),
    StartService(String),
    StopService(String),
}

impl Pending {
    fn label(&self) -> String {
        match self {
            Pending::Terminate(pid) => format!("¿Finalizar la tarea {pid}?"),
            Pending::Suspend(pid) => format!("¿Suspender el proceso {pid}?"),
            Pending::Resume(pid) => format!("¿Reanudar el proceso {pid}?"),
            Pending::StartService(name) => format!("¿Iniciar {name}?"),
            Pending::StopService(name) => format!("¿Detener {name}?"),
        }
    }
}

struct HistoryBuf {
    cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
    disk: HashMap<String, VecDeque<f32>>,
    net: HashMap<String, VecDeque<f32>>,
    gpu: HashMap<String, VecDeque<f32>>,
}

impl HistoryBuf {
    fn new() -> Self {
        Self {
            cpu: VecDeque::new(),
            memory: VecDeque::new(),
            disk: HashMap::new(),
            net: HashMap::new(),
            gpu: HashMap::new(),
        }
    }

    fn push(&mut self, snapshot: &Snapshot) {
        let summary = &snapshot.summary;
        push_cap(&mut self.cpu, summary.cpu_percent);
        let memory = if summary.memory_total == 0 {
            0.0
        } else {
            summary.memory_used as f32 / summary.memory_total as f32 * 100.0
        };
        push_cap(&mut self.memory, memory);
        for disk in &snapshot.disks {
            let rate = disk.read_per_sec.saturating_add(disk.write_per_sec) as f32;
            push_cap(self.disk.entry(disk.mount.clone()).or_default(), rate);
        }
        for network in &snapshot.networks {
            let rate = network
                .received_per_sec
                .saturating_add(network.transmitted_per_sec) as f32;
            push_cap(self.net.entry(network.name.clone()).or_default(), rate);
        }
        for gpu in &snapshot.gpus {
            push_cap(
                self.gpu.entry(gpu.name.clone()).or_default(),
                gpu.utilization.unwrap_or(0.0),
            );
        }
    }
}

fn push_cap(values: &mut VecDeque<f32>, value: f32) {
    values.push_back(value);
    while values.len() > HISTORY {
        values.pop_front();
    }
}

struct TaskApp {
    monitor: SystemMonitor,
    snapshot: Snapshot,
    page: Page,
    sort: SortCol,
    last_refresh: Instant,
    history: HistoryBuf,
    cpu_seconds: HashMap<u32, f32>,
    expanded: HashSet<String>,
    selected_pid: Option<u32>,
    selected_service: Option<String>,
    meter: Meter,
    filter: String,
    pending: Option<Pending>,
    notice: Option<String>,
    run_open: bool,
    run_text: String,
    startup: Vec<StartupRow>,
    startup_at: Instant,
}

impl TaskApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_font(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx);
        let monitor = SystemMonitor::open();
        let snapshot = monitor.latest().clone();
        let mut history = HistoryBuf::new();
        history.push(&snapshot);
        Self {
            monitor,
            snapshot,
            page: Page::Processes,
            sort: SortCol::Memory,
            last_refresh: Instant::now(),
            history,
            cpu_seconds: HashMap::new(),
            expanded: HashSet::new(),
            selected_pid: None,
            selected_service: None,
            meter: Meter::Cpu,
            filter: String::new(),
            pending: None,
            notice: None,
            run_open: false,
            run_text: String::new(),
            startup: crate::startup::sample_startup(),
            startup_at: Instant::now(),
        }
    }

    fn refresh_if_due(&mut self) {
        if self.last_refresh.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        let dt = self.last_refresh.elapsed().as_secs_f32();
        self.snapshot = self.monitor.pull().clone();
        for process in &self.snapshot.processes {
            *self.cpu_seconds.entry(process.pid).or_insert(0.0) += process.cpu / 100.0 * dt;
        }
        self.history.push(&self.snapshot);
        if self.startup_at.elapsed() > Duration::from_secs(10) {
            self.startup = crate::startup::sample_startup();
            self.startup_at = Instant::now();
        }
        self.last_refresh = Instant::now();
        if self.selected_pid.is_some_and(|pid| {
            !self.snapshot.processes.iter().any(|row| row.pid == pid)
        }) {
            self.selected_pid = None;
            if matches!(
                self.pending,
                Some(Pending::Terminate(_) | Pending::Suspend(_) | Pending::Resume(_))
            ) {
                self.pending = None;
            }
        }
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.page.label()).size(20.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.tools(ui);
            });
        });
        if self.run_open {
            ui.horizontal(|ui| {
                ui.label("Abrir:");
                let edit = TextEdit::singleline(&mut self.run_text)
                    .hint_text("programa o comando")
                    .desired_width(360.0);
                let submitted = ui.add(edit).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Aceptar").clicked() || submitted {
                    self.notice = Some(match launch(&self.run_text) {
                        Ok(()) => "tarea iniciada".to_string(),
                        Err(error) => error,
                    });
                    self.run_open = false;
                }
                if ui.button("Cancelar").clicked() {
                    self.run_open = false;
                }
            });
        }
        if let Some(pending) = self.pending.clone() {
            ui.horizontal(|ui| {
                ui.label(pending.label());
                if ui.button("Confirmar").clicked() {
                    self.notice = Some(run_pending(pending));
                    self.pending = None;
                    self.refresh();
                }
                if ui.button("Cancelar").clicked() {
                    self.pending = None;
                }
            });
        } else if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(MUTED).size(12.0));
        }
        ui.add_space(4.0);
    }

    fn tools(&mut self, ui: &mut egui::Ui) {
        let page = self.page;
        let pid = self.selected_pid;
        let service = self.selected_service.clone();
        if page == Page::Services && tool(ui, "Detener") {
            self.arm_service(service.clone(), false);
        }
        if page == Page::Services && tool(ui, "Iniciar") {
            self.arm_service(service, true);
        }
        if matches!(page, Page::Processes | Page::Users | Page::Details) && tool(ui, "Reanudar") {
            self.arm_process(pid, Pending::Resume);
        }
        if matches!(page, Page::Processes | Page::Users | Page::Details) && tool(ui, "Suspender") {
            self.arm_process(pid, Pending::Suspend);
        }
        if matches!(page, Page::Processes | Page::Users | Page::Details) && tool(ui, "Finalizar tarea")
        {
            self.arm_process(pid, Pending::Terminate);
        }
        if matches!(
            page,
            Page::Processes | Page::Users | Page::Details | Page::Services | Page::Startup | Page::History
        ) && tool(ui, "Ejecutar nueva tarea")
        {
            self.run_open = true;
            self.pending = None;
        }
    }

    fn arm_process(&mut self, pid: Option<u32>, kind: fn(u32) -> Pending) {
        let Some(pid) = pid else {
            self.notice = Some("elegí un proceso".to_string());
            return;
        };
        self.pending = Some(kind(pid));
        self.notice = None;
    }

    fn arm_service(&mut self, name: Option<String>, start: bool) {
        let Some(name) = name else {
            self.notice = Some("elegí un servicio".to_string());
            return;
        };
        self.pending = Some(if start {
            Pending::StartService(name)
        } else {
            Pending::StopService(name)
        });
        self.notice = None;
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        self.title_bar(ui);
        match self.page {
            Page::Processes => self.process_page(ui),
            Page::Performance => self.performance_page(ui),
            Page::History => self.history_page(ui),
            Page::Startup => self.startup_page(ui),
            Page::Users => self.users_page(ui),
            Page::Details => self.details_page(ui),
            Page::Services => self.services_page(ui),
            Page::Settings => self.settings_page(ui),
        }
    }

    fn process_page(&mut self, ui: &mut egui::Ui) {
        let lines = process_lines(
            &self.snapshot.processes,
            &self.filter,
            self.sort,
            &self.expanded,
            self.selected_pid,
        );
        self.show_lines(ui, metric_heads(&self.snapshot, true), lines, true);
    }

    fn users_page(&mut self, ui: &mut egui::Ui) {
        let lines = user_lines(
            &self.snapshot.processes,
            &self.filter,
            self.sort,
            &self.expanded,
            self.selected_pid,
        );
        self.show_lines(ui, metric_heads(&self.snapshot, true), lines, true);
    }

    fn details_page(&mut self, ui: &mut egui::Ui) {
        let lines = detail_lines(&self.snapshot.processes, &self.filter, self.sort, self.selected_pid);
        let heads = vec![
            Head::flex("Nombre", None),
            Head::fixed("PID", None, 72.0, false),
            Head::fixed("Estado", None, 120.0, false),
            Head::fixed("Usuario", None, 140.0, false),
            Head::fixed("CPU", Some(percent_text(self.snapshot.summary.cpu_percent)), 80.0, true),
            Head::fixed(
                "Memoria",
                Some(memory_percent(&self.snapshot)),
                120.0,
                true,
            ),
        ];
        self.show_lines(ui, heads, lines, true);
    }

    fn services_page(&mut self, ui: &mut egui::Ui) {
        if let Some(note) = &self.snapshot.services_note {
            ui.label(note);
            return;
        }
        let selected = self.selected_service.clone();
        let lines = self
            .snapshot
            .services
            .iter()
            .filter(|service| {
                matches_filter(&self.filter, &[&service.display, &service.name, &service.pid.to_string()])
            })
            .map(|service| {
                let pid = if service.pid == 0 {
                    String::new()
                } else {
                    service.pid.to_string()
                };
                Line {
                    id: service.name.clone(),
                    pid: None,
                    service: Some(service.name.clone()),
                    indent: 0.0,
                    expandable: false,
                    expanded: false,
                    selected: selected.as_deref() == Some(service.name.as_str()),
                    cells: vec![
                        cell(&service.name),
                        cell(&pid),
                        cell(&service.display),
                        cell(&service.state),
                    ],
                }
            })
            .collect();
        let heads = vec![
            Head::fixed("Nombre", None, 160.0, false),
            Head::fixed("PID", None, 72.0, false),
            Head::flex("Descripción", None),
            Head::fixed("Estado", None, 130.0, false),
        ];
        self.show_lines(ui, heads, lines, false);
    }

    fn history_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Uso de CPU desde que se abrió esta ventana.")
                .color(MUTED)
                .size(12.0),
        );
        ui.add_space(6.0);
        let mut groups: HashMap<String, (String, f32)> = HashMap::new();
        for process in &self.snapshot.processes {
            if !matches_filter(&self.filter, &[&process.name, &process.pid.to_string()]) {
                continue;
            }
            let key = group_key(&process.name);
            let seconds = self.cpu_seconds.get(&process.pid).copied().unwrap_or(0.0);
            let entry = groups.entry(key).or_insert_with(|| (pretty_name(&process.name), 0.0));
            entry.1 += seconds;
        }
        let mut rows: Vec<_> = groups.into_values().collect();
        rows.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
        let lines = rows
            .into_iter()
            .map(|(name, seconds)| Line {
                id: name.clone(),
                pid: None,
                service: None,
                indent: 0.0,
                expandable: false,
                expanded: false,
                selected: false,
                cells: vec![cell(&name), cell(&clock(seconds)), cell("—"), cell("—")],
            })
            .collect();
        let heads = vec![
            Head::flex("Nombre", None),
            Head::fixed("Tiempo de CPU", None, 140.0, true),
            Head::fixed("Red", None, 100.0, true),
            Head::fixed("Notificaciones", None, 130.0, true),
        ];
        self.show_lines(ui, heads, lines, false);
    }

    fn startup_page(&mut self, ui: &mut egui::Ui) {
        let lines = self
            .startup
            .iter()
            .filter(|row| matches_filter(&self.filter, &[&row.name, &row.source]))
            .map(|row| Line {
                id: row.name.clone(),
                pid: None,
                service: None,
                indent: 0.0,
                expandable: false,
                expanded: false,
                selected: false,
                cells: vec![
                    cell(&row.name),
                    cell(&row.source),
                    cell(if row.enabled { "Habilitado" } else { "Deshabilitado" }),
                    cell("—"),
                ],
            })
            .collect();
        let heads = vec![
            Head::flex("Nombre", None),
            Head::fixed("Editor", None, 180.0, false),
            Head::fixed("Estado", None, 130.0, false),
            Head::fixed("Impacto de inicio", None, 150.0, false),
        ];
        self.show_lines(ui, heads, lines, false);
    }

    fn settings_page(&self, ui: &mut egui::Ui) {
        ui.label("La ventana sigue el Administrador de tareas de Windows 11.");
        ui.add_space(8.0);
        ui.label(RichText::new("La lista se actualiza cada un segundo.").color(MUTED));
        ui.label(
            RichText::new("En la terminal, taskmgr --tui abre la misma información.")
                .color(MUTED),
        );
        ui.label(
            RichText::new("El historial de red de Windows y el impacto de arranque no están en este corte.")
                .color(MUTED),
        );
    }

    fn show_lines(&mut self, ui: &mut egui::Ui, heads: Vec<Head>, lines: Vec<Line>, sortable: bool) {
        let width = ui.available_width();
        let event = paint_table(ui, width, &heads, &lines, self.sort, sortable);
        match event {
            Some(Event::Sort(sort)) => self.sort = sort,
            Some(Event::Toggle(id)) => {
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
            }
            Some(Event::Pick { pid, service }) => {
                if pid.is_some() {
                    self.selected_pid = pid;
                }
                if service.is_some() {
                    self.selected_service = service;
                }
                self.pending = None;
            }
            None => {}
        }
    }

    fn performance_page(&mut self, ui: &mut egui::Ui) {
        let height = ui.available_height();
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.set_height(height);
            ui.vertical(|ui| {
                ui.set_width(300.0);
                ui.set_height(height);
                egui::ScrollArea::vertical()
                    .id_salt("meters")
                    .show(ui, |ui| {
                        chosen = self.meter_list(ui);
                    });
            });
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                self.meter_detail(ui);
            });
        });
        if let Some(meter) = chosen {
            self.meter = meter;
        }
    }

    fn meter_list(&self, ui: &mut egui::Ui) -> Option<Meter> {
        let mut chosen = None;
        let summary = &self.snapshot.summary;
        if meter_card(
            ui,
            self.meter == Meter::Cpu,
            "CPU",
            &format!(
                "{}   {}",
                percent_text(summary.cpu_percent),
                ghz(summary.cpu_mhz)
            ),
            &self.history.cpu,
            100.0,
        ) {
            chosen = Some(Meter::Cpu);
        }
        let memory = if summary.memory_total == 0 {
            "0 B".to_string()
        } else {
            format!(
                "{}/{} ({})",
                format_size(summary.memory_used),
                format_size(summary.memory_total),
                memory_percent(&self.snapshot)
            )
        };
        if meter_card(
            ui,
            self.meter == Meter::Memory,
            "Memoria",
            &memory,
            &self.history.memory,
            100.0,
        ) {
            chosen = Some(Meter::Memory);
        }
        for (index, disk) in self.snapshot.disks.iter().enumerate() {
            let meter = Meter::Disk(disk.mount.clone());
            let rate = disk.read_per_sec.saturating_add(disk.write_per_sec);
            let values = series(&self.history.disk, &disk.mount);
            let max = series_max(&values);
            if meter_card(
                ui,
                self.meter == meter,
                &format!("Disco {index} ({})", disk.mount.trim_end_matches('\\')),
                &format!("{}\n{}", disk.kind, rate_text(rate)),
                &values,
                max,
            ) {
                chosen = Some(Meter::Disk(disk.mount.clone()));
            }
        }
        for network in &self.snapshot.networks {
            let meter = Meter::Net(network.name.clone());
            let values = series(&self.history.net, &network.name);
            let max = series_max(&values);
            if meter_card(
                ui,
                self.meter == meter,
                &clip(&network.name, 28),
                &format!(
                    "E: {}   R: {}",
                    rate_text(network.transmitted_per_sec),
                    rate_text(network.received_per_sec)
                ),
                &values,
                max,
            ) {
                chosen = Some(Meter::Net(network.name.clone()));
            }
        }
        for (index, gpu) in self.snapshot.gpus.iter().enumerate() {
            let meter = Meter::Gpu(gpu.name.clone());
            let values = series(&self.history.gpu, &gpu.name);
            let used = gpu
                .memory_used
                .map(format_size)
                .unwrap_or_else(|| "—".to_string());
            if meter_card(
                ui,
                self.meter == meter,
                &format!("GPU {index}"),
                &format!(
                    "{}\n{}   {}",
                    clip(&gpu.name, 26),
                    gpu.utilization
                        .map(percent_text)
                        .unwrap_or_else(|| "—".to_string()),
                    used
                ),
                &values,
                100.0,
            ) {
                chosen = Some(Meter::Gpu(gpu.name.clone()));
            }
        }
        chosen
    }

    fn meter_detail(&self, ui: &mut egui::Ui) {
        let summary = &self.snapshot.summary;
        match &self.meter {
            Meter::Cpu => {
                chart(ui, "CPU", &summary.cpu_brand, &self.history.cpu, 100.0, true);
                stat_grid(
                    ui,
                    &[
                        ("Uso", percent_text(summary.cpu_percent)),
                        ("Velocidad", ghz(summary.cpu_mhz)),
                        ("Procesos", summary.process_count.to_string()),
                        (
                            "Núcleos físicos",
                            summary
                                .physical_cpus
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "—".to_string()),
                        ),
                        ("Procesadores lógicos", summary.logical_cpus.to_string()),
                        ("Tiempo activo", uptime(summary.uptime_secs)),
                    ],
                );
            }
            Meter::Memory => {
                chart(ui, "Memoria", "uso de RAM", &self.history.memory, 100.0, true);
                stat_grid(
                    ui,
                    &[
                        ("En uso", format_size(summary.memory_used)),
                        ("Total", format_size(summary.memory_total)),
                        ("Uso", memory_percent(&self.snapshot)),
                    ],
                );
            }
            Meter::Disk(mount) => {
                let values = series(&self.history.disk, mount);
                let max = series_max(&values);
                let disk = self.snapshot.disks.iter().find(|disk| &disk.mount == mount);
                chart(ui, "Disco", mount, &values, max, false);
                if let Some(disk) = disk {
                    stat_grid(
                        ui,
                        &[
                            ("Lectura", rate_text(disk.read_per_sec)),
                            ("Escritura", rate_text(disk.write_per_sec)),
                            ("Usado", format_size(disk.used)),
                            ("Total", format_size(disk.total)),
                            ("Tipo", disk.kind.clone()),
                            ("Sistema de archivos", disk.file_system.clone()),
                        ],
                    );
                }
            }
            Meter::Net(name) => {
                let values = series(&self.history.net, name);
                let max = series_max(&values);
                let network = self.snapshot.networks.iter().find(|network| &network.name == name);
                chart(ui, "Ethernet", name, &values, max, false);
                if let Some(network) = network {
                    stat_grid(
                        ui,
                        &[
                            ("Recepción", rate_text(network.received_per_sec)),
                            ("Envío", rate_text(network.transmitted_per_sec)),
                            ("Total recibido", format_size(network.total_received)),
                            ("Total enviado", format_size(network.total_transmitted)),
                        ],
                    );
                }
            }
            Meter::Gpu(name) => {
                let values = series(&self.history.gpu, name);
                let gpu = self.snapshot.gpus.iter().find(|gpu| &gpu.name == name);
                chart(ui, "GPU", name, &values, 100.0, true);
                if let Some(gpu) = gpu {
                    stat_grid(
                        ui,
                        &[
                            (
                                "Uso",
                                gpu.utilization
                                    .map(percent_text)
                                    .unwrap_or_else(|| "—".to_string()),
                            ),
                            (
                                "Memoria",
                                gpu.memory_used
                                    .map(format_size)
                                    .unwrap_or_else(|| "—".to_string()),
                            ),
                            (
                                "Total",
                                gpu.memory_total
                                    .map(format_size)
                                    .unwrap_or_else(|| "—".to_string()),
                            ),
                        ],
                    );
                }
            }
        }
    }
}

impl eframe::App for TaskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_if_due();
        ctx.request_repaint_after(Duration::from_secs(1));
        egui::TopBottomPanel::top("search")
            .exact_height(48.0)
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(12, 8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let width = 460.0_f32.min(ui.available_width());
                    let spare = (ui.available_width() - width).max(0.0) / 2.0;
                    ui.add_space(spare);
                    ui.visuals_mut().widgets.inactive.corner_radius = CornerRadius::same(16);
                    ui.visuals_mut().widgets.hovered.corner_radius = CornerRadius::same(16);
                    ui.visuals_mut().widgets.active.corner_radius = CornerRadius::same(16);
                    ui.visuals_mut().widgets.inactive.bg_fill = Color32::from_rgb(48, 48, 48);
                    ui.add(
                        TextEdit::singleline(&mut self.filter)
                            .hint_text("Busca por nombre, editor o PID")
                            .desired_width(width),
                    );
                });
            });
        egui::SidePanel::left("nav")
            .exact_width(248.0)
            .resizable(false)
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::same(8)))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                menu_glyph(ui);
                ui.add_space(8.0);
                for page in Page::all() {
                    nav(ui, &mut self.page, *page);
                }
                let spare = ui.available_height() - 40.0;
                if spare > 0.0 {
                    ui.add_space(spare);
                }
                nav(ui, &mut self.page, Page::Settings);
            });
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::same(16)))
            .show(ctx, |ui| self.content(ui));
    }
}

fn nav(ui: &mut egui::Ui, current: &mut Page, page: Page) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let painter = ui.painter();
    if *current == page {
        painter.rect_filled(rect.shrink(1.0), CornerRadius::same(4), SELECTED);
    } else if response.hovered() {
        painter.rect_filled(rect.shrink(1.0), CornerRadius::same(4), HOVER);
    }
    let icon = Rect::from_min_size(rect.min + vec2(12.0, 10.0), vec2(16.0, 16.0));
    paint_icon(painter, icon, page);
    painter.text(
        pos2(rect.left() + 40.0, rect.center().y),
        Align2::LEFT_CENTER,
        page.label(),
        FontId::proportional(14.0),
        TEXT,
    );
    if response.clicked() {
        *current = page;
    }
}

fn menu_glyph(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    let x = rect.left() + 14.0;
    let color = MUTED;
    for index in 0..3 {
        let y = rect.top() + 4.0 + index as f32 * 5.0;
        painter.line_segment(
            [pos2(x, y), pos2(x + 14.0, y)],
            Stroke::new(1.4, color),
        );
    }
}

struct Head {
    title: &'static str,
    value: Option<String>,
    width: Option<f32>,
    sort: Option<SortCol>,
    heat: bool,
}

impl Head {
    fn flex(title: &'static str, value: Option<String>) -> Self {
        Self {
            title,
            value,
            width: None,
            sort: Some(SortCol::Name),
            heat: false,
        }
    }

    fn fixed(title: &'static str, value: Option<String>, width: f32, heat: bool) -> Self {
        let sort = match title {
            "CPU" => Some(SortCol::Cpu),
            "Memoria" => Some(SortCol::Memory),
            "Disco" => Some(SortCol::Disk),
            _ => None,
        };
        Self {
            title,
            value,
            width: Some(width),
            sort,
            heat,
        }
    }
}

struct Cell {
    text: String,
    heat: Option<f32>,
}

fn cell(text: &str) -> Cell {
    Cell {
        text: text.to_string(),
        heat: None,
    }
}

fn heat_cell(text: String, fraction: f32) -> Cell {
    Cell {
        text,
        heat: Some(fraction.clamp(0.0, 1.0)),
    }
}

struct Line {
    id: String,
    pid: Option<u32>,
    service: Option<String>,
    indent: f32,
    expandable: bool,
    expanded: bool,
    selected: bool,
    cells: Vec<Cell>,
}

enum Event {
    Sort(SortCol),
    Toggle(String),
    Pick { pid: Option<u32>, service: Option<String> },
}

fn paint_table(
    ui: &mut egui::Ui,
    width: f32,
    heads: &[Head],
    lines: &[Line],
    sort: SortCol,
    sortable: bool,
) -> Option<Event> {
    let (header, response) = ui.allocate_exact_size(vec2(width, 48.0), Sense::click());
    let painter = ui.painter();
    let rects = layout(header, heads);
    painter.line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, Color32::from_rgb(58, 58, 58)),
    );
    for (head, rect) in heads.iter().zip(&rects) {
        if head.heat {
            painter.rect_filled(*rect, 0.0, HEAT_BG);
        }
        if let Some(value) = &head.value {
            painter.text(
                rect.right_top() + vec2(-8.0, 2.0),
                Align2::RIGHT_TOP,
                value,
                FontId::proportional(18.0),
                TEXT,
            );
        }
        let mark = head.sort == Some(sort);
        let title = if mark {
            format!("{}  ▾", head.title)
        } else {
            head.title.to_string()
        };
        if head.width.is_none() {
            painter.text(
                rect.left_bottom() + vec2(12.0, -4.0),
                Align2::LEFT_BOTTOM,
                title,
                FontId::proportional(12.0),
                MUTED,
            );
        } else {
            painter.text(
                rect.right_bottom() + vec2(-8.0, -4.0),
                Align2::RIGHT_BOTTOM,
                title,
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }
    let mut event = None;
    if sortable && response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            for (head, rect) in heads.iter().zip(&rects) {
                if rect.contains(pos) {
                    if let Some(sort) = head.sort {
                        event = Some(Event::Sort(sort));
                    }
                }
            }
        }
    }

    egui::ScrollArea::vertical()
        .id_salt("rows")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(width);
            for line in lines {
                if let Some(next) = paint_line(ui, width, heads, line) {
                    event = Some(next);
                }
            }
        });
    event
}

fn paint_line(ui: &mut egui::Ui, width: f32, heads: &[Head], line: &Line) -> Option<Event> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let painter = ui.painter();
    let rects = layout(rect, heads);
    if line.selected {
        painter.rect_filled(rect, 0.0, Color32::from_white_alpha(14));
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, Color32::from_white_alpha(8));
    }
    for (index, cell) in line.cells.iter().enumerate() {
        let Some(cell_rect) = rects.get(index).copied() else {
            continue;
        };
        if cell.heat.is_some() || heads.get(index).is_some_and(|head| head.heat) {
            painter.rect_filled(cell_rect, 0.0, HEAT_BG);
        }
        if let Some(fraction) = cell.heat {
            if fraction > 0.004 {
                let bar = Rect::from_min_max(
                    pos2(cell_rect.left() + 1.0, cell_rect.top() + 5.0),
                    pos2(
                        cell_rect.left() + (cell_rect.width() - 2.0) * fraction,
                        cell_rect.bottom() - 5.0,
                    ),
                );
                painter.rect_filled(bar, CornerRadius::same(2), heat_color(fraction));
            }
        }
        let text_pos = if cell.heat.is_some() {
            cell_rect.right_center() + vec2(-8.0, 0.0)
        } else if index == 0 {
            let extra = if line.expandable { 18.0 } else { 4.0 };
            pos2(
                cell_rect.left() + 8.0 + line.indent + extra,
                cell_rect.center().y,
            )
        } else {
            cell_rect.left_center() + vec2(8.0, 0.0)
        };
        let align = if cell.heat.is_some() {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        if index == 0 && line.expandable {
            chevron(
                painter,
                pos2(cell_rect.left() + 10.0 + line.indent, cell_rect.center().y),
                line.expanded,
            );
        }
        let clipped = painter.with_clip_rect(cell_rect.shrink(1.0));
        clipped.text(
            text_pos,
            align,
            &cell.text,
            FontId::proportional(13.0),
            TEXT,
        );
    }
    if response.clicked() {
        let on_chevron = response.interact_pointer_pos().is_some_and(|pos| {
            let name = rects.first().copied().unwrap_or(rect);
            let zone = Rect::from_min_size(
                pos2(name.left() + line.indent, name.top()),
                vec2(22.0, name.height()),
            );
            zone.contains(pos)
        });
        if line.expandable && on_chevron {
            return Some(Event::Toggle(line.id.clone()));
        }
        return Some(Event::Pick {
            pid: line.pid,
            service: line.service.clone(),
        });
    }
    None
}

fn layout(full: Rect, heads: &[Head]) -> Vec<Rect> {
    let base_fixed: f32 = heads.iter().filter_map(|head| head.width).sum();
    let extra = (full.width() - base_fixed).max(0.0);
    let name_cap = full.width() * 0.34;
    let name_width = extra.min(name_cap).max(180.0).min(extra);
    let surplus = (extra - name_width).max(0.0);
    let heat_count = heads.iter().filter(|head| head.heat).count() as f32;
    let bonus = if heat_count > 0.0 { surplus / heat_count } else { 0.0 };
    let mut rects = Vec::with_capacity(heads.len());
    let mut x = full.left();
    for head in heads {
        let width = if head.width.is_none() {
            name_width
        } else if head.heat {
            head.width.unwrap_or(0.0) + bonus
        } else {
            head.width.unwrap_or(0.0)
        };
        rects.push(Rect::from_min_max(
            pos2(x, full.top()),
            pos2(x + width, full.bottom()),
        ));
        x += width;
    }
    if let Some(last) = rects.last_mut() {
        *last = Rect::from_min_max(last.min, pos2(full.right(), full.bottom()));
    }
    rects
}

fn metric_heads(snapshot: &Snapshot, with_status: bool) -> Vec<Head> {
    let summary = &snapshot.summary;
    let mut heads = vec![Head::flex("Nombre", None)];
    if with_status {
        heads.push(Head::fixed("Estado", None, 92.0, false));
    }
    heads.push(Head::fixed(
        "CPU",
        Some(percent_text(summary.cpu_percent)),
        108.0,
        true,
    ));
    heads.push(Head::fixed(
        "Memoria",
        Some(memory_percent(snapshot)),
        140.0,
        true,
    ));
    heads.push(Head::fixed(
        "Disco",
        Some(rate_text(
            summary.disk_read_per_sec.saturating_add(summary.disk_write_per_sec),
        )),
        108.0,
        true,
    ));
    heads.push(Head::fixed(
        "Red",
        Some(mbps(
            summary
                .net_received_per_sec
                .saturating_add(summary.net_transmitted_per_sec),
        )),
        96.0,
        true,
    ));
    heads.push(Head::fixed(
        "GPU",
        Some(
            summary
                .gpu_percent
                .map(percent_text)
                .unwrap_or_else(|| "0%".to_string()),
        ),
        72.0,
        true,
    ));
    heads
}

struct Group {
    key: String,
    label: String,
    members: Vec<ProcessRow>,
}

fn process_lines(
    processes: &[ProcessRow],
    filter: &str,
    sort: SortCol,
    expanded: &HashSet<String>,
    selected: Option<u32>,
) -> Vec<Line> {
    let mut groups: HashMap<String, Group> = HashMap::new();
    for process in processes {
        let key = group_key(&process.name);
        let group = groups.entry(key.clone()).or_insert_with(|| Group {
            key: key.clone(),
            label: pretty_name(&process.name),
            members: Vec::new(),
        });
        group.members.push(process.clone());
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    for group in &mut groups {
        sort_members(&mut group.members, sort);
    }
    sort_groups(&mut groups, sort);
    let force = !filter.trim().is_empty();
    let mut lines = Vec::new();
    let maxes = maxima(processes);
    for group in groups {
        let matching: Vec<_> = group
            .members
            .iter()
            .filter(|member| {
                force && matches_filter(filter, &[&member.name, &member.pid.to_string(), &group.label])
                    || !force
            })
            .cloned()
            .collect();
        let group_match = matches_filter(filter, &[&group.label, &group.key]);
        if force && matching.is_empty() && !group_match {
            continue;
        }
        let members = if force && !group_match { matching } else { group.members };
        if members.is_empty() {
            continue;
        }
        let open = members.len() > 1 && (expanded.contains(&group.key) || (force && !group_match));
        let cpu: f32 = members.iter().map(|member| member.cpu).sum();
        let memory: u64 = members.iter().map(|member| member.memory).sum();
        let disk: u64 = members.iter().map(|member| member.disk_per_sec).sum();
        let label = if members.len() > 1 {
            format!("{} ({})", group.label, members.len())
        } else {
            group.label.clone()
        };
        let pid = members.first().map(|member| member.pid);
        let selected_group = members.iter().any(|member| Some(member.pid) == selected) && !open;
        lines.push(metric_line(
            &group.key,
            pid,
            0.0,
            members.len() > 1,
            open,
            selected_group,
            &label,
            "",
            cpu,
            memory,
            disk,
            &maxes,
        ));
        if open {
            for member in &members {
                if force && !group_match && !matches_filter(filter, &[&member.name, &member.pid.to_string()])
                {
                    continue;
                }
                lines.push(metric_line(
                    &format!("pid:{}", member.pid),
                    Some(member.pid),
                    18.0,
                    false,
                    false,
                    Some(member.pid) == selected,
                    &pretty_name(&member.name),
                    &status_text(member.status),
                    member.cpu,
                    member.memory,
                    member.disk_per_sec,
                    &maxes,
                ));
            }
        }
    }
    lines
}

fn user_lines(
    processes: &[ProcessRow],
    filter: &str,
    sort: SortCol,
    expanded: &HashSet<String>,
    selected: Option<u32>,
) -> Vec<Line> {
    let mut groups: HashMap<String, Group> = HashMap::new();
    for process in processes {
        let label = if process.user.is_empty() || process.user == "-" {
            "Sistema".to_string()
        } else {
            process.user.clone()
        };
        let key = format!("user:{}", label.to_ascii_lowercase());
        let group = groups.entry(key.clone()).or_insert_with(|| Group {
            key,
            label,
            members: Vec::new(),
        });
        group.members.push(process.clone());
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    for group in &mut groups {
        sort_members(&mut group.members, sort);
    }
    sort_groups(&mut groups, sort);
    let force = !filter.trim().is_empty();
    let maxes = maxima(processes);
    let mut lines = Vec::new();
    for group in groups {
        let visible: Vec<_> = group
            .members
            .iter()
            .filter(|member| {
                !force
                    || matches_filter(
                        filter,
                        &[&member.name, &member.pid.to_string(), &group.label],
                    )
            })
            .cloned()
            .collect();
        if visible.is_empty() {
            continue;
        }
        let open = expanded.contains(&group.key) || force;
        let cpu: f32 = visible.iter().map(|member| member.cpu).sum();
        let memory: u64 = visible.iter().map(|member| member.memory).sum();
        let disk: u64 = visible.iter().map(|member| member.disk_per_sec).sum();
        lines.push(metric_line(
            &group.key,
            visible.first().map(|member| member.pid),
            0.0,
            true,
            open,
            visible.iter().any(|member| Some(member.pid) == selected) && !open,
            &format!("{} ({})", group.label, visible.len()),
            "",
            cpu,
            memory,
            disk,
            &maxes,
        ));
        if open {
            for member in &visible {
                lines.push(metric_line(
                    &format!("pid:{}", member.pid),
                    Some(member.pid),
                    18.0,
                    false,
                    false,
                    Some(member.pid) == selected,
                    &pretty_name(&member.name),
                    &status_text(member.status),
                    member.cpu,
                    member.memory,
                    member.disk_per_sec,
                    &maxes,
                ));
            }
        }
    }
    lines
}

fn detail_lines(
    processes: &[ProcessRow],
    filter: &str,
    sort: SortCol,
    selected: Option<u32>,
) -> Vec<Line> {
    let mut rows: Vec<_> = processes
        .iter()
        .filter(|process| matches_filter(filter, &[&process.name, &process.pid.to_string(), &process.user]))
        .cloned()
        .collect();
    sort_members(&mut rows, sort);
    rows.into_iter()
        .map(|process| Line {
            id: process.pid.to_string(),
            pid: Some(process.pid),
            service: None,
            indent: 0.0,
            expandable: false,
            expanded: false,
            selected: Some(process.pid) == selected,
            cells: vec![
                cell(&process.name),
                cell(&process.pid.to_string()),
                cell(match process.status {
                    "en ejecución" => "En ejecución",
                    "suspendido" | "detenido" => "Suspendido",
                    other => other,
                }),
                cell(&process.user),
                cell(&percent_text(process.cpu)),
                cell(&format_size(process.memory)),
            ],
        })
        .collect()
}

struct Maxes {
    cpu: f32,
    memory: u64,
    disk: u64,
}

fn maxima(processes: &[ProcessRow]) -> Maxes {
    Maxes {
        cpu: processes.iter().map(|row| row.cpu).fold(0.0, f32::max).max(1.0),
        memory: processes.iter().map(|row| row.memory).max().unwrap_or(1).max(1),
        disk: processes
            .iter()
            .map(|row| row.disk_per_sec)
            .max()
            .unwrap_or(1)
            .max(1),
    }
}

fn metric_line(
    id: &str,
    pid: Option<u32>,
    indent: f32,
    expandable: bool,
    expanded: bool,
    selected: bool,
    name: &str,
    status: &str,
    cpu: f32,
    memory: u64,
    disk: u64,
    maxes: &Maxes,
) -> Line {
    Line {
        id: id.to_string(),
        pid,
        service: None,
        indent,
        expandable,
        expanded,
        selected,
        cells: vec![
            cell(name),
            cell(status),
            heat_cell(percent_text(cpu), cpu / maxes.cpu),
            heat_cell(format_size(memory), memory as f32 / maxes.memory as f32),
            heat_cell(rate_text(disk), disk as f32 / maxes.disk as f32),
            heat_cell("—".to_string(), 0.0),
            heat_cell("—".to_string(), 0.0),
        ],
    }
}

fn sort_members(rows: &mut [ProcessRow], sort: SortCol) {
    rows.sort_by(|left, right| match sort {
        SortCol::Name => left
            .name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then(left.pid.cmp(&right.pid)),
        SortCol::Cpu => right.cpu.total_cmp(&left.cpu).then(left.pid.cmp(&right.pid)),
        SortCol::Memory => right.memory.cmp(&left.memory).then(left.pid.cmp(&right.pid)),
        SortCol::Disk => right
            .disk_per_sec
            .cmp(&left.disk_per_sec)
            .then(left.pid.cmp(&right.pid)),
    });
}

fn sort_groups(groups: &mut [Group], sort: SortCol) {
    groups.sort_by(|left, right| {
        let order = match sort {
            SortCol::Name => left
                .label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase()),
            SortCol::Cpu => sum_cpu(right)
                .total_cmp(&sum_cpu(left))
                .then(left.label.cmp(&right.label)),
            SortCol::Memory => sum_memory(right).cmp(&sum_memory(left)),
            SortCol::Disk => sum_disk(right).cmp(&sum_disk(left)),
        };
        order.then(left.label.to_ascii_lowercase().cmp(&right.label.to_ascii_lowercase()))
    });
}

fn sum_cpu(group: &Group) -> f32 {
    group.members.iter().map(|member| member.cpu).sum()
}

fn sum_memory(group: &Group) -> u64 {
    group.members.iter().map(|member| member.memory).sum()
}

fn sum_disk(group: &Group) -> u64 {
    group.members.iter().map(|member| member.disk_per_sec).sum()
}

fn meter_card(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    subtitle: &str,
    values: &VecDeque<f32>,
    max: f32,
) -> bool {
    let samples: Vec<f32> = values.iter().copied().collect();
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 68.0), Sense::click());
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(
            rect.shrink(2.0),
            CornerRadius::same(4),
            if selected { SELECTED } else { HOVER },
        );
    }
    let graph = Rect::from_min_size(rect.min + vec2(8.0, 16.0), vec2(78.0, 36.0));
    sparkline(painter, graph, &samples, max, TEAL);
    painter.text(
        pos2(rect.left() + 98.0, rect.top() + 12.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(14.0),
        TEXT,
    );
    painter.text(
        pos2(rect.left() + 98.0, rect.top() + 32.0),
        Align2::LEFT_TOP,
        subtitle,
        FontId::proportional(12.0),
        MUTED,
    );
    ui.add_space(4.0);
    response.clicked()
}

fn chart(ui: &mut egui::Ui, title: &str, subtitle: &str, values: &VecDeque<f32>, max: f32, percent: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(28.0).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(subtitle).size(16.0).color(MUTED));
        });
    });
    ui.label(RichText::new(if percent { "% de uso" } else { "actividad" }).color(MUTED).size(12.0));
    let height = (ui.available_height() - 120.0).clamp(180.0, 460.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    for index in 0..5 {
        let y = rect.top() + rect.height() * index as f32 / 4.0;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right() - 48.0, y)],
            Stroke::new(1.0, Color32::from_rgb(48, 48, 48)),
        );
        let label = if percent {
            format!("{}%", 100 - index * 25)
        } else if index == 0 {
            rate_text(max as u64)
        } else {
            String::new()
        };
        painter.text(
            pos2(rect.right() - 4.0, y),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(11.0),
            MUTED,
        );
    }
    painter.text(
        pos2(rect.left(), rect.bottom() + 4.0),
        Align2::LEFT_TOP,
        "60 segundos",
        FontId::proportional(11.0),
        MUTED,
    );
    let plot = Rect::from_min_max(rect.min, pos2(rect.right() - 52.0, rect.bottom()));
    let samples: Vec<f32> = values.iter().copied().collect();
    sparkline(painter, plot, &samples, max, TEAL);
    ui.add_space(18.0);
}

fn stat_grid(ui: &mut egui::Ui, rows: &[(&str, String)]) {
    ui.add_space(8.0);
    let columns = 2;
    egui::Grid::new("stats").num_columns(columns * 2).spacing([24.0, 8.0]).show(ui, |ui| {
        for (index, (label, value)) in rows.iter().enumerate() {
            ui.label(RichText::new(*label).color(MUTED));
            ui.label(RichText::new(value).strong());
            if index % columns == columns - 1 {
                ui.end_row();
            }
        }
    });
}

fn sparkline(painter: &egui::Painter, rect: Rect, values: &[f32], max: f32, color: Color32) {
    if values.len() < 2 || rect.width() < 2.0 {
        return;
    }
    let max = max.max(0.001);
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = rect.left() + rect.width() * index as f32 / (values.len() - 1) as f32;
            let y = rect.bottom() - rect.height() * (value / max).clamp(0.0, 1.0);
            pos2(x, y)
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.4, color)));
}

fn series(map: &HashMap<String, VecDeque<f32>>, key: &str) -> VecDeque<f32> {
    map.get(key).cloned().unwrap_or_default()
}

fn series_max(values: &VecDeque<f32>) -> f32 {
    values.iter().copied().fold(1.0, f32::max).max(1.0)
}

fn tool(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(13.0).color(TEXT))
            .frame(false)
            .min_size(vec2(0.0, 28.0)),
    )
    .clicked()
}

fn chevron(painter: &egui::Painter, center: Pos2, expanded: bool) {
    let points = if expanded {
        vec![
            center + vec2(-4.0, -2.0),
            center + vec2(4.0, -2.0),
            center + vec2(0.0, 3.0),
        ]
    } else {
        vec![
            center + vec2(-2.0, -4.5),
            center + vec2(-2.0, 4.5),
            center + vec2(4.0, 0.0),
        ]
    };
    painter.add(Shape::convex_polygon(
        points,
        MUTED,
        Stroke::NONE,
    ));
}

fn paint_icon(painter: &egui::Painter, rect: Rect, page: Page) {
    let color = Color32::from_rgb(210, 210, 210);
    let stroke = Stroke::new(1.3, color);
    match page {
        Page::Processes => {
            for index in 0..3 {
                let y = rect.top() + 2.0 + index as f32 * 5.0;
                painter.circle_filled(pos2(rect.left() + 2.0, y), 1.3, color);
                painter.line_segment([pos2(rect.left() + 6.0, y), pos2(rect.right(), y)], stroke);
            }
        }
        Page::Performance => {
            let mid = rect.center().y;
            painter.add(Shape::line(
                vec![
                    pos2(rect.left(), mid),
                    pos2(rect.left() + 3.0, mid),
                    pos2(rect.left() + 5.0, rect.top() + 2.0),
                    pos2(rect.left() + 9.0, rect.bottom() - 2.0),
                    pos2(rect.left() + 12.0, mid),
                    pos2(rect.right(), mid),
                ],
                stroke,
            ));
        }
        Page::History => {
            painter.circle_stroke(rect.center(), 7.0, stroke);
            painter.line_segment([rect.center(), rect.center() + vec2(0.0, -4.0)], stroke);
            painter.line_segment([rect.center(), rect.center() + vec2(3.0, 2.0)], stroke);
        }
        Page::Startup => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(rect.center().x, rect.top()),
                    pos2(rect.right() - 2.0, rect.bottom() - 4.0),
                    pos2(rect.left() + 2.0, rect.bottom() - 4.0),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
        }
        Page::Users => {
            painter.circle_stroke(pos2(rect.center().x, rect.top() + 4.5), 3.0, stroke);
            painter.add(Shape::line(
                vec![
                    pos2(rect.left() + 1.0, rect.bottom()),
                    pos2(rect.left() + 3.0, rect.center().y + 1.0),
                    pos2(rect.right() - 3.0, rect.center().y + 1.0),
                    pos2(rect.right() - 1.0, rect.bottom()),
                ],
                stroke,
            ));
        }
        Page::Details => {
            for index in 0..3 {
                let y = rect.top() + 2.0 + index as f32 * 5.0;
                painter.rect_stroke(
                    Rect::from_min_size(pos2(rect.left(), y - 1.5), vec2(3.0, 3.0)),
                    0.0,
                    stroke,
                    egui::StrokeKind::Outside,
                );
                painter.line_segment([pos2(rect.left() + 6.0, y), pos2(rect.right(), y)], stroke);
            }
        }
        Page::Services => {
            painter.circle_stroke(rect.center(), 3.0, stroke);
            for index in 0..4 {
                let angle = index as f32 * std::f32::consts::FRAC_PI_2;
                let dir = vec2(angle.cos(), angle.sin());
                painter.line_segment(
                    [rect.center() + dir * 4.0, rect.center() + dir * 7.0],
                    stroke,
                );
            }
        }
        Page::Settings => {
            painter.circle_stroke(rect.center(), 3.2, stroke);
            painter.circle_stroke(rect.center(), 6.5, stroke);
        }
    }
}

fn heat_color(fraction: f32) -> Color32 {
    let t = fraction.clamp(0.0, 1.0);
    Color32::from_rgb(
        (36.0 + 40.0 * t) as u8,
        (78.0 + 70.0 * t) as u8,
        (140.0 + 70.0 * t) as u8,
    )
}

fn matches_filter(filter: &str, fields: &[&str]) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    filter.is_empty()
        || fields
            .iter()
            .any(|field| field.to_ascii_lowercase().contains(&filter))
}

fn group_key(name: &str) -> String {
    pretty_name(name).to_ascii_lowercase()
}

fn pretty_name(name: &str) -> String {
    if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        name[..name.len() - 4].to_string()
    } else {
        name.to_string()
    }
}

fn status_text(status: &str) -> String {
    match status {
        "en ejecución" => String::new(),
        "suspendido" | "detenido" => "Suspendido".to_string(),
        other => other.to_string(),
    }
}

fn percent_text(value: f32) -> String {
    if value < 0.05 {
        "0%".to_string()
    } else if value < 10.0 {
        format!("{}%", comma(value as f64, 1))
    } else {
        format!("{value:.0}%")
    }
}

fn memory_percent(snapshot: &Snapshot) -> String {
    if snapshot.summary.memory_total == 0 {
        "0%".to_string()
    } else {
        let percent = snapshot.summary.memory_used as f32 / snapshot.summary.memory_total as f32 * 100.0;
        percent_text(percent)
    }
}

fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1024.0 / 1024.0;
    if mb >= 1024.0 {
        format!("{} GB", comma(mb / 1024.0, 1))
    } else if mb >= 100.0 {
        format!("{} MB", comma(mb, 1))
    } else if mb >= 0.1 {
        format!("{} MB", comma(mb, 1))
    } else if bytes >= 1024 {
        format!("{} KB", comma(bytes as f64 / 1024.0, 0))
    } else {
        format!("{bytes} B")
    }
}

fn rate_text(bytes_per_sec: u64) -> String {
    if bytes_per_sec == 0 {
        "0 MB/s".to_string()
    } else {
        format!("{}/s", format_size(bytes_per_sec))
    }
}

fn mbps(bytes_per_sec: u64) -> String {
    let value = bytes_per_sec as f64 * 8.0 / 1_000_000.0;
    if value < 0.05 {
        "0 Mbps".to_string()
    } else {
        format!("{} Mbps", comma(value, 1))
    }
}

fn ghz(mhz: u64) -> String {
    if mhz == 0 {
        "—".to_string()
    } else {
        format!("{} GHz", comma(mhz as f64 / 1000.0, 2))
    }
}

fn comma(value: f64, digits: usize) -> String {
    let negative = value.is_sign_negative();
    let text = format!("{:.*}", digits, value.abs());
    let (int, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut grouped = String::new();
    for (index, ch) in int.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    let sign = if negative { "-" } else { "" };
    if digits == 0 || frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped},{frac}")
    }
}

fn clock(seconds: f32) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
}

fn uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let mins = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    if days > 0 {
        format!("{days}:{hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{hours}:{mins:02}:{secs:02}")
    }
}

fn clip(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    let mut out: String = value.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn launch(command: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("escribí un programa o un comando".to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", command])
            .creation_flags(0x0800_0000)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .args(["-c", command])
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
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

fn install_font(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Segoe UI".to_owned(),
        Arc::new(FontData::from_owned(bytes)),
    );
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "Segoe UI".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = BG;
    visuals.panel_fill = BG;
    visuals.extreme_bg_color = Color32::from_rgb(24, 24, 24);
    visuals.faint_bg_color = Color32::from_rgb(42, 42, 42);
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    visuals.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(58, 58, 58);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(48, 48, 48);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(62, 62, 62);
    visuals.widgets.active.bg_fill = Color32::from_rgb(72, 72, 72);
    visuals.selection.bg_fill = Color32::from_rgb(46, 86, 140);
    visuals.override_text_color = Some(TEXT);
    ctx.set_visuals(visuals);
}
