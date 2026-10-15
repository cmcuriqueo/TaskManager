use std::collections::{HashMap, HashSet};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

const ICON_PX: usize = 32;
const LOADS_PER_FRAME: u8 = 12;

pub struct IconCache {
    ready: HashMap<String, TextureHandle>,
    failed: HashSet<String>,
    service_paths: HashMap<String, String>,
    budget: u8,
    pub starved: bool,
}

impl IconCache {
    pub fn new() -> Self {
        Self {
            ready: HashMap::new(),
            failed: HashSet::new(),
            service_paths: HashMap::new(),
            budget: LOADS_PER_FRAME,
            starved: false,
        }
    }

    pub fn begin_frame(&mut self) {
        self.budget = LOADS_PER_FRAME;
        self.starved = false;
    }

    pub fn texture(&mut self, ctx: &egui::Context, key: &str) -> Option<egui::TextureId> {
        if key.is_empty() {
            return None;
        }
        let cache_key = self.resolve(key);
        if cache_key.is_empty() {
            return None;
        }
        if let Some(texture) = self.ready.get(&cache_key) {
            return Some(texture.id());
        }
        if self.failed.contains(&cache_key) {
            return None;
        }
        if self.budget == 0 {
            self.starved = true;
            return None;
        }
        self.budget -= 1;
        match load_pixels(&cache_key) {
            Some(pixels) => {
                let image = ColorImage::from_rgba_unmultiplied([ICON_PX, ICON_PX], &pixels);
                let handle = ctx.load_texture(cache_key.clone(), image, TextureOptions::LINEAR);
                let id = handle.id();
                self.ready.insert(cache_key, handle);
                Some(id)
            }
            None => {
                self.failed.insert(cache_key);
                None
            }
        }
    }

    fn resolve(&mut self, key: &str) -> String {
        let Some(name) = key.strip_prefix("service:") else {
            return key.to_string();
        };
        if let Some(path) = self.service_paths.get(name) {
            return path.clone();
        }
        if self.budget == 0 {
            self.starved = true;
            return String::new();
        }
        self.budget -= 1;
        let path = service_binary(name).unwrap_or_else(|| "stock:app".to_string());
        self.service_paths.insert(name.to_string(), path.clone());
        path
    }
}

fn load_pixels(key: &str) -> Option<Vec<u8>> {
    if key == "stock:app" {
        return stock_icon(Stock::App);
    }
    if key == "stock:user" {
        return stock_icon(Stock::User);
    }
    file_icon(key).or_else(|| stock_icon(Stock::App))
}

enum Stock {
    App,
    User,
}

#[cfg(windows)]
fn file_icon(path: &str) -> Option<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};

    let wide: Vec<u16> = std::ffi::OsStr::new(path).encode_wide().chain([0]).collect();
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(std::ptr::addr_of_mut!(info)),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if found == 0 {
        return None;
    }
    raster(info.hIcon)
}

#[cfg(not(windows))]
fn file_icon(path: &str) -> Option<Vec<u8>> {
    // Use gdk-pixbuf to load and rasterize icons. This delegates icon theme resolution
    // and SVG rendering to the system-installed gdk-pixbuf loaders (e.g. librsvg).
    use gdk_pixbuf::Pixbuf;
    use std::path::Path;

    fn try_load_file(p: &Path) -> Option<Vec<u8>> {
        // Load at requested size (keeps aspect ratio and scales)
        let pb = Pixbuf::from_file_at_scale(p, ICON_PX as i32, ICON_PX as i32, true).ok()?;
        pixbuf_to_rgba(&pb)
    }

    let p = Path::new(path);
    // Try absolute/relative path first
    if (p.is_absolute() || p.components().count() > 1) && p.exists() {
        if let Some(px) = try_load_file(p) {
            return Some(px);
        }
    }

    // Treat as icon name: search XDG data dirs and common locations
    let name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);

    let mut candidates = Vec::new();
    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        candidates.push(format!("{}/icons", xdg_data_home));
        candidates.push(format!("{}/pixmaps", xdg_data_home));
    } else if let Ok(home) = std::env::var("HOME") {
        candidates.push(format!("{}/.local/share/icons", home));
        candidates.push(format!("{}/.local/share/pixmaps", home));
    }

    if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for dir in xdg_data_dirs.split(':') {
            candidates.push(format!("{}/icons", dir));
            candidates.push(format!("{}/pixmaps", dir));
        }
    } else {
        candidates.push("/usr/share/icons".to_string());
        candidates.push("/usr/share/pixmaps".to_string());
    }

    let exts = ["png", "xpm", "ico", "svg", "jpg", "jpeg"];

    for base in candidates {
        // try base/<name>.<ext>
        for ext in &exts {
            let cand = Path::new(&base).join(format!("{}.{}", name, ext));
            if cand.exists() {
                if let Some(px) = try_load_file(&cand) {
                    return Some(px);
                }
            }
        }

        // try theme subdirs: base/*/apps/<name>.<ext>
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    let apps = path.join("apps");
                    if apps.exists() {
                        for ext in &exts {
                            let cand = apps.join(format!("{}.{}", name, ext));
                            if cand.exists() {
                                if let Some(px) = try_load_file(&cand) {
                                    return Some(px);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(not(windows))]
fn stock_icon(kind: Stock) -> Option<Vec<u8>> {
    // Map built-in stock icons to common freedesktop icon names and reuse file_icon.
    let name = match kind {
        Stock::App => "application-x-executable",
        Stock::User => "user",
    };
    file_icon(name)
}

#[cfg(windows)]
fn stock_icon(kind: Stock) -> Option<Vec<u8>> {
    use windows::Win32::UI::Shell::{
        SHGetStockIconInfo, SHSTOCKICONINFO, SHGSI_ICON, SIID_APPLICATION, SIID_USERS,
    };

    let id = match kind {
        Stock::App => SIID_APPLICATION,
        Stock::User => SIID_USERS,
    };
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..SHSTOCKICONINFO::default()
    };
    if unsafe { SHGetStockIconInfo(id, SHGSI_ICON, std::ptr::addr_of_mut!(info)) }.is_err() {
        return None;
    }
    raster(info.hIcon)
}

#[cfg(windows)]
fn service_binary(name: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_CONFIG,
    };

    unsafe {
        let manager = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
        let wide: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().chain([0]).collect();
        let service = OpenServiceW(manager, PCWSTR(wide.as_ptr()), SERVICE_QUERY_CONFIG).ok();
        let path = service.and_then(|service| {
            let path = query_binary(service);
            let _ = CloseServiceHandle(service);
            path
        });
        let _ = CloseServiceHandle(manager);
        path
    }
}

#[cfg(windows)]
fn query_binary(service: windows::Win32::System::Services::SC_HANDLE) -> Option<String> {
    use windows::Win32::System::Services::{QueryServiceConfigW, QUERY_SERVICE_CONFIGW};

    let mut needed = 0u32;
    unsafe { let _ = QueryServiceConfigW(service, None, 0, &mut needed); };
    if needed < std::mem::size_of::<QUERY_SERVICE_CONFIGW>() as u32 {
        return None;
    }
    let mut buffer = vec![0u8; needed as usize];
    let queried = unsafe {
        QueryServiceConfigW(
            service,
            Some(buffer.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
            needed,
            &mut needed,
        )
    };
    if queried.is_err() {
        return None;
    }
    let config = unsafe { &*(buffer.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    let raw = if config.lpBinaryPathName.is_null() {
        String::new()
    } else {
        unsafe { config.lpBinaryPathName.to_string() }.unwrap_or_default()
    };
    let prepared = crate::startup::prepare_icon_path(&raw);
    if prepared.is_empty() { None } else { Some(prepared) }
}

#[cfg(not(windows))]
fn service_binary(_name: &str) -> Option<String> {
    None
}

#[cfg(windows)]
fn raster(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL};

    if icon.is_invalid() {
        return None;
    }
    unsafe {
        let mem = CreateCompatibleDC(None);
        if mem.is_invalid() {
            let _ = DestroyIcon(icon);
            return None;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_PX as i32,
            biHeight: -(ICON_PX as i32),
            biPlanes: 1,
            biBitCount: 32,
            ..BITMAPINFOHEADER::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(_) => {
                let _ = DeleteDC(mem);
                let _ = DestroyIcon(icon);
                return None;
            }
        };
        let previous = SelectObject(mem, bitmap.into());
        let drawn = DrawIconEx(
            mem,
            0,
            0,
            icon,
            ICON_PX as i32,
            ICON_PX as i32,
            0,
            None,
            DI_NORMAL,
        )
        .is_ok();
        let pixels = if drawn && !bits.is_null() {
            Some(bgra_to_rgba(std::slice::from_raw_parts(
                bits as *const u8,
                ICON_PX * ICON_PX * 4,
            )))
        } else {
            None
        };
        if !previous.is_invalid() {
            SelectObject(mem, previous);
        }
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        let _ = DestroyIcon(icon);
        pixels
    }
}

#[cfg(windows)]
fn bgra_to_rgba(pixels: &[u8]) -> Vec<u8> {
    let opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == 0)
        && pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0);
    let mut rgba = Vec::with_capacity(pixels.len());
    for pixel in pixels.chunks_exact(4) {
        let blue = pixel[0] as u32;
        let green = pixel[1] as u32;
        let red = pixel[2] as u32;
        let alpha = if opaque { 255 } else { pixel[3] as u32 };
        if alpha == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else if opaque {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        } else {
            rgba.push(((red * 255) / alpha).min(255) as u8);
            rgba.push(((green * 255) / alpha).min(255) as u8);
            rgba.push(((blue * 255) / alpha).min(255) as u8);
            rgba.push(alpha as u8);
        }
    }
    rgba
}

fn pixbuf_to_rgba(pb: &gdk_pixbuf::Pixbuf) -> Option<Vec<u8>> {
    // Convert Pixbuf pixels (RGB or RGBA) into RGBA Vec<u8> row-major expected by egui
    let width = pb.get_width();
    let height = pb.get_height();
    let n_channels = pb.get_n_channels();
    let has_alpha = pb.get_has_alpha();
    let rowstride = pb.get_rowstride() as usize;
    let pixels = pb.get_pixels();
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let start = (y as usize) * rowstride;
        for x in 0..width as usize {
            let i = start + x * n_channels as usize;
            let r = pixels[i];
            let g = pixels[i + 1];
            let b = pixels[i + 2];
            let a = if has_alpha {
                pixels[i + 3]
            } else {
                255
            };
            out.push(r);
            out.push(g);
            out.push(b);
            out.push(a);
        }
    }
    Some(out)
}
