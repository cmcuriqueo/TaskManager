use std::process::Command;

#[cfg(target_os = "linux")]
use std::path::Path;

/// Una GPU observada. La utilización y la memoria usada faltan cuando la
/// fuente solo conoce el adaptador.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuRow {
    pub name: String,
    pub utilization: Option<f32>,
    pub memory_used: Option<u64>,
    pub memory_total: Option<u64>,
}

pub fn sample_gpus() -> Vec<GpuRow> {
    merge_gpus(nvidia_smi_rows(), platform_gpus())
}

pub fn merge_gpus(primary: Vec<GpuRow>, extra: Vec<GpuRow>) -> Vec<GpuRow> {
    let mut rows = primary;
    let have_nvidia = rows.iter().any(|row| is_nvidia(&row.name));
    for row in extra {
        if have_nvidia && is_nvidia(&row.name) {
            continue;
        }
        if rows
            .iter()
            .any(|existing| same_name(&existing.name, &row.name))
        {
            continue;
        }
        rows.push(row);
    }
    rows.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    rows
}

pub fn parse_nvidia_smi(text: &str) -> Vec<GpuRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts: Vec<&str> = line.rsplitn(4, ',').map(str::trim).collect();
            if parts.len() != 4 {
                return None;
            }
            parts.reverse();
            let name = parts[0];
            if name.is_empty() {
                return None;
            }
            Some(GpuRow {
                name: name.to_string(),
                utilization: parse_percent(parts[1]),
                memory_used: parse_mib(parts[2]),
                memory_total: parse_mib(parts[3]),
            })
        })
        .collect()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn gpu_from_sysfs(
    vendor: Option<&str>,
    device: Option<&str>,
    product: Option<&str>,
    busy: Option<&str>,
    vram_used: Option<&str>,
    vram_total: Option<&str>,
) -> Option<GpuRow> {
    let vendor_id = vendor.and_then(parse_hex_u32);
    let device_id = device.and_then(parse_hex_u32);
    let product = product.map(str::trim).filter(|value| !value.is_empty());
    let name = match (product, vendor_id, device_id) {
        (Some(product), _, _) => product.to_string(),
        (None, Some(vendor_id), Some(device_id)) => {
            format!("{} {device_id:04x}", vendor_label(vendor_id))
        }
        (None, Some(vendor_id), None) => vendor_label(vendor_id).to_string(),
        (None, None, _) => return None,
    };
    Some(GpuRow {
        name,
        utilization: busy.and_then(parse_percent),
        memory_used: vram_used.and_then(parse_bytes),
        memory_total: vram_total.and_then(parse_bytes),
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn is_drm_card(name: &str) -> bool {
    let Some(index) = name.strip_prefix("card") else {
        return false;
    };
    !index.is_empty() && index.chars().all(|ch| ch.is_ascii_digit())
}

fn nvidia_smi_rows() -> Vec<GpuRow> {
    let mut command = Command::new("nvidia-smi");
    command.args([
        "--query-gpu=name,utilization.gpu,memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = match command.output() {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    parse_nvidia_smi(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "linux")]
fn platform_gpus() -> Vec<GpuRow> {
    let entries = match std::fs::read_dir("/sys/class/drm") {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !is_drm_card(&file_name) {
            continue;
        }
        if let Some(row) = read_drm_card(&entry.path().join("device")) {
            rows.push(row);
        }
    }
    rows
}

#[cfg(target_os = "linux")]
fn read_drm_card(device: &Path) -> Option<GpuRow> {
    gpu_from_sysfs(
        read_trimmed(&device.join("vendor")).as_deref(),
        read_trimmed(&device.join("device")).as_deref(),
        read_trimmed(&device.join("product_name")).as_deref(),
        read_trimmed(&device.join("gpu_busy_percent")).as_deref(),
        read_trimmed(&device.join("mem_info_vram_used")).as_deref(),
        read_trimmed(&device.join("mem_info_vram_total")).as_deref(),
    )
}

#[cfg(target_os = "linux")]
fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(windows)]
fn platform_gpus() -> Vec<GpuRow> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };

    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    let mut index = 0u32;
    loop {
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        index += 1;
        let desc = match unsafe { adapter.GetDesc1() } {
            Ok(desc) => desc,
            Err(_) => continue,
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let name = wide_to_string(&desc.Description);
        if name.is_empty() {
            continue;
        }
        let memory_total = if desc.DedicatedVideoMemory > 0 {
            Some(desc.DedicatedVideoMemory as u64)
        } else if desc.SharedSystemMemory > 0 {
            Some(desc.SharedSystemMemory as u64)
        } else {
            None
        };
        rows.push(GpuRow {
            name,
            utilization: None,
            memory_used: None,
            memory_total,
        });
    }
    rows
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_gpus() -> Vec<GpuRow> {
    Vec::new()
}

#[cfg(windows)]
fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end]).trim().to_string()
}

fn is_nvidia(name: &str) -> bool {
    name.to_ascii_lowercase().contains("nvidia")
}

fn same_name(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn vendor_label(vendor: u32) -> &'static str {
    match vendor {
        0x10de => "NVIDIA",
        0x1002 => "AMD",
        0x8086 => "Intel",
        _ => "GPU",
    }
}

fn parse_percent(text: &str) -> Option<f32> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("n/a") || text == "[N/A]" {
        return None;
    }
    text.parse::<f32>().ok().filter(|value| value.is_finite())
}

fn parse_mib(text: &str) -> Option<u64> {
    parse_bytes(text).map(|value| value.saturating_mul(1024 * 1024))
}

fn parse_bytes(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("n/a") || text == "[N/A]" {
        return None;
    }
    text.parse().ok()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_hex_u32(text: &str) -> Option<u32> {
    let text = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    if text.is_empty() {
        None
    } else {
        u32::from_str_radix(text, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuRow, gpu_from_sysfs, is_drm_card, merge_gpus, parse_nvidia_smi};

    #[test]
    fn parses_nvidia_smi_with_commas_in_the_name() {
        let rows = parse_nvidia_smi(
            "NVIDIA GeForce RTX 4070, Laptop, 12, 1024, 8188\nNVIDIA GeForce RTX 3060, [N/A], 0, 6144\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "NVIDIA GeForce RTX 4070, Laptop");
        assert_eq!(rows[0].utilization, Some(12.0));
        assert_eq!(rows[0].memory_used, Some(1024 * 1024 * 1024));
        assert_eq!(rows[1].name, "NVIDIA GeForce RTX 3060");
        assert_eq!(rows[1].utilization, None);
    }

    #[test]
    fn reads_amd_sysfs_fields() {
        let row = gpu_from_sysfs(
            Some("0x1002"),
            Some("0x73bf"),
            None,
            Some("37"),
            Some("1048576"),
            Some("8589934592"),
        )
        .expect("gpu");
        assert_eq!(row.name, "AMD 73bf");
        assert_eq!(row.utilization, Some(37.0));
        assert_eq!(row.memory_used, Some(1_048_576));
        assert_eq!(row.memory_total, Some(8_589_934_592));
    }

    #[test]
    fn prefers_nvidia_smi_over_the_same_vendor_adapter() {
        let primary = vec![GpuRow {
            name: "NVIDIA GeForce RTX 4070".to_string(),
            utilization: Some(4.0),
            memory_used: Some(1),
            memory_total: Some(2),
        }];
        let extra = vec![GpuRow {
            name: "NVIDIA GeForce RTX 4070".to_string(),
            utilization: None,
            memory_used: None,
            memory_total: Some(2),
        }];
        let rows = merge_gpus(primary, extra);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].utilization, Some(4.0));
    }

    #[test]
    fn recognizes_drm_card_directories() {
        assert!(is_drm_card("card0"));
        assert!(is_drm_card("card12"));
        assert!(!is_drm_card("card0-DP-1"));
        assert!(!is_drm_card("renderD128"));
    }
}
