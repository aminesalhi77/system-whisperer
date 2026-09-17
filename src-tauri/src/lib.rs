use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Disks, System};
use once_cell::sync::Lazy;

// Global cached System + last refresh time
static SYS: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new_all()));
static LAST_REFRESH: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));

#[derive(Serialize)]
struct Snapshot {
    cpu_usage: f32,
    total_ram_gb: f64,
    used_ram_gb: f64,
    ram_percent: f64,
    disk_total_gb: f64,
    disk_used_gb: f64,
    disk_used_percent: f64,
    whisper: String,
}

#[derive(Serialize)]
struct ProcessInfo {
    pid: u32,
    name: String,
    ram_mb: f64,
    cpu_usage: f32,
    icon_path: Option<String>,
}

#[derive(Serialize)]
struct DiskEntry {
    name: String,
    path: String,
    size_mb: f64,
    is_dir: bool,
}

// ---------- Refresh helper (throttled) ----------
fn refresh_system() {
    let mut last = LAST_REFRESH.lock().unwrap();
    let should_refresh = match *last {
        None => true,
        Some(t) => t.elapsed() >= Duration::from_millis(900),
    };
    if should_refresh {
        if let Ok(mut sys) = SYS.lock() {
            sys.refresh_all();
        }
        *last = Some(Instant::now());
    }
}

// ---------- Snapshot ----------
#[tauri::command]
async fn get_system_snapshot() -> Snapshot {
    refresh_system();

    let (cpu_usage, total_ram, used_ram) = {
        let sys = SYS.lock().unwrap();
        let total = sys.total_memory() as f64 / 1_073_741_824.0;
        let used  = sys.used_memory()  as f64 / 1_073_741_824.0;
        (sys.global_cpu_usage(), total, used)
    };

    let ram_percent = if total_ram > 0.0 { (used_ram / total_ram) * 100.0 } else { 0.0 };

    let disks = Disks::new_with_refreshed_list();
    let (disk_total, disk_used) = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .or_else(|| disks.iter().next())
        .map(|d| {
            let total = d.total_space() as f64 / 1_073_741_824.0;
            let avail = d.available_space() as f64 / 1_073_741_824.0;
            (total, total - avail)
        })
        .unwrap_or((0.0, 0.0));

    let disk_used_percent = if disk_total > 0.0 { (disk_used / disk_total) * 100.0 } else { 0.0 };

    let whisper = if ram_percent > 85.0 {
        format!("RAM is {:.0}% full ({:.1} / {:.1} GB). Click RAM to see the hog.", ram_percent, used_ram, total_ram)
    } else if disk_used_percent > 90.0 {
        format!("Disk is {:.0}% full ({:.0} / {:.0} GB). Click Disk to see what's big.", disk_used_percent, disk_used, disk_total)
    } else if cpu_usage > 70.0 {
        format!("CPU at {:.0}%. Something's working hard — click CPU to see what.", cpu_usage)
    } else {
        format!("All healthy — CPU {:.0}%, RAM {:.0}%, Disk {:.0}%.", cpu_usage, ram_percent, disk_used_percent)
    };

    Snapshot {
        cpu_usage,
        total_ram_gb: total_ram,
        used_ram_gb: used_ram,
        ram_percent,
        disk_total_gb: disk_total,
        disk_used_gb: disk_used,
        disk_used_percent,
        whisper,
    }
}

// ---------- Processes ----------
#[tauri::command]
async fn get_top_processes(sort_by: String) -> Vec<ProcessInfo> {
    refresh_system();

    let mut procs: Vec<ProcessInfo> = {
        let sys = SYS.lock().unwrap();
        sys.processes()
            .iter()
            .map(|(pid, p)| {
                let name = p.name().to_string_lossy().to_string();
                ProcessInfo {
                    pid: pid.as_u32(),
                    icon_path: find_icon_path(&name),
                    name,
                    ram_mb: p.memory() as f64 / 1_048_576.0,
                    cpu_usage: p.cpu_usage(),
                }
            })
            .collect()
    };

    if sort_by == "cpu" {
        procs.sort_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        procs.sort_by(|a, b| b.ram_mb.partial_cmp(&a.ram_mb).unwrap_or(std::cmp::Ordering::Equal));
    }

    procs.into_iter().take(8).collect()
}

// ---------- Disk ----------
#[tauri::command]
async fn get_disk_usage(path: Option<String>) -> Vec<DiskEntry> {
    let target = path.unwrap_or_else(|| {
        std::env::var("HOME").unwrap_or_else(|_| "/".into())
    });
    let root = PathBuf::from(&target);
    if !root.is_dir() { return vec![]; }

    let Ok(entries) = std::fs::read_dir(&root) else { return vec![] };
    let mut items: Vec<DiskEntry> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') { continue; }

        let meta = match std::fs::symlink_metadata(&path) { Ok(m) => m, Err(_) => continue };
        let is_dir = meta.is_dir();
        let size_bytes = if is_dir { dir_size(&path, 0) } else { meta.len() };

        if is_dir && size_bytes < 100 * 1024 * 1024 { continue; }
        if !is_dir && size_bytes < 50 * 1024 * 1024 { continue; }

        items.push(DiskEntry {
            name,
            path: path.to_string_lossy().to_string(),
            size_mb: size_bytes as f64 / 1_048_576.0,
            is_dir,
        });
    }

    items.sort_by(|a, b| b.size_mb.partial_cmp(&a.size_mb).unwrap_or(std::cmp::Ordering::Equal));
    items.truncate(15);
    items
}

fn dir_size(path: &PathBuf, depth: u32) -> u64 {
    if depth > 4 { return 0; }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.is_dir() { total += dir_size(&p, depth + 1); } else { total += m.len(); }
    }
    total
}

// ---------- Icons (Linux only) ----------
#[cfg(target_os = "linux")]
fn find_icon_path(process_name: &str) -> Option<String> {
    let lower = process_name.to_lowercase();
    let skip = ["bash", "sh", "zsh", "fish", "systemd", "kworker", "kthread", "init", "dbus"];
    if skip.iter().any(|s| lower.starts_with(s)) { return None; }

    let dirs = [
        "/usr/share/applications",
        "/usr/local/share/applications",
        "/var/lib/flatpak/exports/share/applications",
        "/home/cheester/.local/share/applications",
    ];

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") { continue; }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };

            let file_match = path.file_stem().and_then(|s| s.to_str())
                .map(|s| s.to_lowercase().contains(&lower)).unwrap_or(false);

            let exec_match = content.lines()
                .find(|l| l.starts_with("Exec="))
                .map(|l| l.to_lowercase().contains(&lower))
                .unwrap_or(false);

            if !file_match && !exec_match { continue; }

            if let Some(icon_line) = content.lines().find(|l| l.starts_with("Icon=")) {
                let icon_name = icon_line.trim_start_matches("Icon=").trim();
                if icon_name.starts_with('/') { return Some(icon_name.to_string()); }
                if let Some(found) = resolve_icon_name(icon_name) { return Some(found); }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn resolve_icon_name(icon_name: &str) -> Option<String> {
    let base_dirs = [
        "/usr/share/icons/hicolor",
        "/usr/share/icons/Adwaita",
        "/usr/share/icons/Papirus",
        "/usr/share/icons/Papirus-Dark",
        "/usr/share/icons/breeze",
        "/usr/share/icons/breeze-dark",
        "/usr/share/pixmaps",
    ];
    let sizes = ["256x256", "128x128", "64x64", "48x48", "32x32"];
    let exts = ["png", "svg"];

    for base in base_dirs {
        for size in sizes {
            for ext in exts {
                let p = PathBuf::from(base).join(size).join("apps").join(format!("{}.{}", icon_name, ext));
                if p.exists() { return Some(p.to_string_lossy().to_string()); }
            }
        }
        for ext in exts {
            let p = PathBuf::from(base).join(format!("{}.{}", icon_name, ext));
            if p.exists() { return Some(p.to_string_lossy().to_string()); }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn find_icon_path(_process_name: &str) -> Option<String> { None }

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_system_snapshot,
            get_top_processes,
            get_disk_usage
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}