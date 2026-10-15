use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{
    CpuRefreshKind, DiskKind, Disks, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, Networks,
    ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, RefreshKind, System, Uid, UpdateKind,
    Users,
};

/// Columna activa de la lista de procesos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Cpu,
    Memory,
    Name,
    Pid,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "memoria",
            Self::Name => "nombre",
            Self::Pid => "PID",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    pub disk_per_sec: u64,
    pub status: &'static str,
    pub user: String,
    pub exe: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskRow {
    pub mount: String,
    pub kind: String,
    pub file_system: String,
    pub used: u64,
    pub total: u64,
    pub read_per_sec: u64,
    pub write_per_sec: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRow {
    pub name: String,
    pub received_per_sec: u64,
    pub transmitted_per_sec: u64,
    pub total_received: u64,
    pub total_transmitted: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub cpu_percent: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub process_count: usize,
    pub disk_read_per_sec: u64,
    pub disk_write_per_sec: u64,
    pub net_received_per_sec: u64,
    pub net_transmitted_per_sec: u64,
    pub gpu_percent: Option<f32>,
    pub cpu_brand: String,
    pub cpu_mhz: u64,
    pub logical_cpus: usize,
    pub physical_cpus: Option<usize>,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub summary: Summary,
    pub processes: Vec<ProcessRow>,
    pub disks: Vec<DiskRow>,
    pub networks: Vec<NetworkRow>,
    pub gpus: Vec<crate::gpu::GpuRow>,
    pub services: Vec<crate::services::ServiceRow>,
    pub services_note: Option<String>,
    pub sessions: Vec<crate::sessions::SessionRow>,
    pub sessions_note: Option<String>,
}

/// Lectura de CPU, memoria, procesos, discos y redes.
///
/// En Linux `sysinfo` obtiene estos datos de `/proc` y `/proc/net/dev`.
/// En Windows usa las API nativas, para poder compilar el mismo monitor
/// durante el desarrollo.
pub struct SystemMonitor {
    system: System,
    users: HashMap<Uid, String>,
    disks: Disks,
    networks: Networks,
    last_pull: Option<Instant>,
    latest: Snapshot,
}

impl SystemMonitor {
    pub fn open() -> Self {
        let mut monitor = Self::new();
        monitor.pull();
        thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        monitor.pull();
        monitor
    }

    fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage().with_frequency())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let users = Users::new_with_refreshed_list();
        let users = users
            .list()
            .iter()
            .map(|user| (user.id().clone(), user.name().to_string()))
            .collect();
        Self {
            system,
            users,
            disks: Disks::new(),
            networks: Networks::new(),
            last_pull: None,
            latest: Snapshot::empty(),
        }
    }

    pub fn latest(&self) -> &Snapshot {
        &self.latest
    }

    pub fn pull(&mut self) -> &Snapshot {
        let now = Instant::now();
        let elapsed = self
            .last_pull
            .map(|previous| now.saturating_duration_since(previous))
            .unwrap_or(Duration::ZERO);

        self.system
            .refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage().with_frequency());
        self.system.refresh_memory();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_disk_usage()
                .with_memory()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_user(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );
        self.disks.refresh(true);
        self.networks.refresh(true);
        self.last_pull = Some(now);

        let cores = self.system.cpus().len().max(1) as f32;
        let processes = self
            .system
            .processes()
            .values()
            .map(|process| {
                let usage = process.disk_usage();
                ProcessRow {
                    pid: process.pid().as_u32(),
                    name: process_name(process.name()),
                    cpu: process.cpu_usage() / cores,
                    memory: process.memory(),
                    disk_per_sec: bytes_per_second(
                        usage.read_bytes.saturating_add(usage.written_bytes),
                        elapsed,
                    ),
                    status: status_label(process.status()),
                    user: process
                        .user_id()
                        .and_then(|id| self.users.get(id).cloned())
                        .unwrap_or_else(|| "-".to_string()),
                    exe: process
                        .exe()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                }
            })
            .collect();

        let mut disks: Vec<DiskRow> = self
            .disks
            .list()
            .iter()
            .filter(|disk| disk.total_space() > 0)
            .map(|disk| {
                let usage = disk.usage();
                let total = disk.total_space();
                let available = disk.available_space();
                DiskRow {
                    mount: disk.mount_point().display().to_string(),
                    kind: disk_kind_label(disk.kind(), disk.is_removable()),
                    file_system: os_string(disk.file_system()),
                    used: total.saturating_sub(available),
                    total,
                    read_per_sec: bytes_per_second(usage.read_bytes, elapsed),
                    write_per_sec: bytes_per_second(usage.written_bytes, elapsed),
                }
            })
            .collect();
        disks.sort_by(|left, right| {
            let left_io = left.read_per_sec.saturating_add(left.write_per_sec);
            let right_io = right.read_per_sec.saturating_add(right.write_per_sec);
            right_io.cmp(&left_io).then(left.mount.cmp(&right.mount))
        });

        let mut networks: Vec<NetworkRow> = self
            .networks
            .iter()
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, data)| NetworkRow {
                name: name.clone(),
                received_per_sec: bytes_per_second(data.received(), elapsed),
                transmitted_per_sec: bytes_per_second(data.transmitted(), elapsed),
                total_received: data.total_received(),
                total_transmitted: data.total_transmitted(),
            })
            .collect();
        networks.sort_by(|left, right| {
            let left_io = left
                .received_per_sec
                .saturating_add(left.transmitted_per_sec);
            let right_io = right
                .received_per_sec
                .saturating_add(right.transmitted_per_sec);
            right_io.cmp(&left_io).then(left.name.cmp(&right.name))
        });

        self.latest = Snapshot {
            summary: Summary {
                cpu_percent: self.system.global_cpu_usage(),
                memory_used: self.system.used_memory(),
                memory_total: self.system.total_memory(),
                process_count: self.system.processes().len(),
                disk_read_per_sec: disks.iter().map(|disk| disk.read_per_sec).sum(),
                disk_write_per_sec: disks.iter().map(|disk| disk.write_per_sec).sum(),
                net_received_per_sec: networks
                    .iter()
                    .map(|network| network.received_per_sec)
                    .sum(),
                net_transmitted_per_sec: networks
                    .iter()
                    .map(|network| network.transmitted_per_sec)
                    .sum(),
                gpu_percent: None,
                cpu_brand: cpu_brand(&self.system),
                cpu_mhz: self
                    .system
                    .cpus()
                    .iter()
                    .map(|cpu| cpu.frequency())
                    .max()
                    .unwrap_or(0),
                logical_cpus: self.system.cpus().len(),
                physical_cpus: System::physical_core_count(),
                uptime_secs: System::uptime(),
            },
            processes,
            disks,
            networks,
            gpus: Vec::new(),
            services: Vec::new(),
            services_note: None,
            sessions: Vec::new(),
            sessions_note: None,
        };
        let gpus = crate::gpu::sample_gpus();
        self.latest.gpus = gpus;
        self.latest.summary.gpu_percent = self
            .latest
            .gpus
            .iter()
            .filter_map(|gpu| gpu.utilization)
            .max_by(|left, right| left.total_cmp(right));
        let (services, services_note) = crate::services::sample_services();
        self.latest.services = services;
        self.latest.services_note = services_note;
        let (sessions, sessions_note) = crate::sessions::sample_sessions();
        self.latest.sessions = sessions;
        self.latest.sessions_note = sessions_note;
        &self.latest
    }
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            summary: Summary {
                cpu_percent: 0.0,
                memory_used: 0,
                memory_total: 0,
                process_count: 0,
                disk_read_per_sec: 0,
                disk_write_per_sec: 0,
                net_received_per_sec: 0,
                net_transmitted_per_sec: 0,
                gpu_percent: None,
                cpu_brand: String::new(),
                cpu_mhz: 0,
                logical_cpus: 0,
                physical_cpus: None,
                uptime_secs: 0,
            },
            processes: Vec::new(),
            disks: Vec::new(),
            networks: Vec::new(),
            gpus: Vec::new(),
            services: Vec::new(),
            services_note: None,
            sessions: Vec::new(),
            sessions_note: None,
        }
    }
}

pub fn bytes_per_second(bytes: u64, elapsed: Duration) -> u64 {
    let seconds = elapsed.as_secs_f64();
    if seconds < 0.05 {
        0
    } else {
        (bytes as f64 / seconds).round() as u64
    }
}

pub fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn disk_kind_label(kind: DiskKind, removable: bool) -> String {
    let base = match kind {
        DiskKind::HDD => "HDD",
        DiskKind::SSD => "SSD",
        DiskKind::Unknown(_) => "desconocido",
    };
    if removable {
        format!("{base} extraíble")
    } else {
        base.to_string()
    }
}

fn os_string(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
    if value.is_empty() {
        "-".to_string()
    } else {
        value.into_owned()
    }
}

fn cpu_brand(system: &System) -> String {
    system
        .cpus()
        .iter()
        .map(|cpu| cpu.brand().trim())
        .find(|brand| !brand.is_empty())
        .unwrap_or("CPU")
        .to_string()
}

fn process_name(name: &std::ffi::OsStr) -> String {
    let name = name.to_string_lossy();
    if name.is_empty() {
        "-".to_string()
    } else {
        name.into_owned()
    }
}

pub fn sort_processes(rows: &mut [ProcessRow], key: SortKey) {
    rows.sort_by(|left, right| match key {
        SortKey::Cpu => right
            .cpu
            .total_cmp(&left.cpu)
            .then(left.pid.cmp(&right.pid)),
        SortKey::Memory => right
            .memory
            .cmp(&left.memory)
            .then(left.pid.cmp(&right.pid)),
        SortKey::Name => left
            .name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then(left.pid.cmp(&right.pid)),
        SortKey::Pid => left.pid.cmp(&right.pid),
    });
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn status_label(status: ProcessStatus) -> &'static str {
    match status {
        ProcessStatus::Idle => "inactivo",
        ProcessStatus::Run => "en ejecución",
        ProcessStatus::Sleep => "suspendido",
        ProcessStatus::Stop => "detenido",
        ProcessStatus::Zombie => "zombie",
        ProcessStatus::Tracing => "depurando",
        ProcessStatus::Dead => "muerto",
        ProcessStatus::Wakekill => "wakekill",
        ProcessStatus::Waking => "despertando",
        ProcessStatus::Parked => "parked",
        ProcessStatus::LockBlocked => "bloqueado",
        ProcessStatus::UninterruptibleDiskSleep => "disco",
        ProcessStatus::Unknown(_) => "desconocido",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ProcessRow, SortKey, bytes_per_second, format_bytes, sort_processes};

    fn row(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.to_string(),
            cpu,
            memory,
            disk_per_sec: 0,
            status: "en ejecución",
            user: "user".to_string(),
            exe: String::new(),
        }
    }

    #[test]
    fn rates_use_the_sample_interval() {
        assert_eq!(bytes_per_second(1024, Duration::from_secs(1)), 1024);
        assert_eq!(bytes_per_second(2048, Duration::from_millis(500)), 4096);
        assert_eq!(bytes_per_second(u64::MAX, Duration::ZERO), 0);
    }

    #[test]
    fn formats_binary_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
    }

    #[test]
    fn sorts_cpu_then_pid() {
        let mut rows = vec![
            row(3, "b", 1.0, 10),
            row(1, "a", 9.0, 1),
            row(2, "c", 9.0, 5),
        ];
        sort_processes(&mut rows, SortKey::Cpu);
        let order: Vec<_> = rows.iter().map(|row| row.pid).collect();
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn sorts_name_case_insensitive() {
        let mut rows = vec![row(2, "zeta", 0.0, 0), row(1, "alfa", 0.0, 0)];
        sort_processes(&mut rows, SortKey::Name);
        let order: Vec<_> = rows.iter().map(|row| row.name.clone()).collect();
        assert_eq!(order, vec!["alfa", "zeta"]);
    }
}
