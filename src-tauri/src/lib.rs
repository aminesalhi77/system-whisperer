use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Disks, Networks, System};

// ---------- Global caches ----------
static SYS: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new_all()));
static LAST_REFRESH: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));
static ICON_CACHE: Lazy<Mutex<HashMap<String, Option<String>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static DESKTOP_INDEX: Lazy<Mutex<Option<HashMap<String, String>>>> =
    Lazy::new(|| Mutex::new(None));

// Network state — tracks delta between calls
struct NetState {
    networks: Networks,
    last_refresh: Instant,
    last_rx: u64,
    last_tx: u64,
}
static NET: Lazy<Mutex<Option<NetState>>> = Lazy::new(|| Mutex::new(None));

// ---------- Structs ----------
#[derive(Serialize)]
struct Snapshot {
    cpu_usage: f32,
    total_ram_gb: f64,
    used_ram_gb: f64,
    ram_percent: f64,
    disk_total_gb: f64,
    disk_used_gb: f64,
    disk_used_percent: f64,
    // Network
    net_down_mbps: f64,
    net_up_mbps: f64,
    net_connections: usize,
    // Whisper
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

#[derive(Serialize)]
struct NetInterface {
    name: String,
    down_mbps: f64,
    up_mbps: f64,
    total_rx_mb: f64,
    total_tx_mb: f64,
}

// ---------- Refresh ----------
fn refresh_system() {
    let mut last = LAST_REFRESH.lock().unwrap();
    let should_refresh = match *last {
        None => true,
        Some(t) => t.elapsed() >= Duration::from_millis(1500),
    };
    if should_refresh {
        if let Ok(mut sys) = SYS.lock() {
            sys.refresh_all();
        }
        *last = Some(Instant::now());
    }
}

// ---------- Network helper ----------
/// Returns (down_mbps, up_mbps, connections, per-interface list)
/// Returns (down_mbps, up_mbps, connections, per-interface list)
fn sample_network() -> (f64, f64, usize, Vec<NetInterface>) {
    let mut guard = NET.lock().unwrap();

    // First call — initialize
    if guard.is_none() {
        let mut nets = Networks::new_with_refreshed_list();
        nets.refresh(true);
        let total_rx: u64 = nets.iter().map(|(_, d)| d.total_received()).sum();
        let total_tx: u64 = nets.iter().map(|(_, d)| d.total_transmitted()).sum();
        *guard = Some(NetState {
            networks: nets,
            last_refresh: Instant::now(),
            last_rx: total_rx,
            last_tx: total_tx,
        });
        return (0.0, 0.0, 0, vec![]);
    }

    let state = guard.as_mut().unwrap();

    // Always refresh the underlying counters first
    state.networks.refresh(true);

    let now = Instant::now();
    let elapsed = now.duration_since(state.last_refresh).as_secs_f64();

    let total_rx: u64 = state.networks.iter().map(|(_, d)| d.total_received()).sum();
    let total_tx: u64 = state.networks.iter().map(|(_, d)| d.total_transmitted()).sum();

    // Per-interface live rates (each interface has its own delta)
    let interfaces: Vec<NetInterface> = state
        .networks
        .iter()
        .map(|(name, data)| {
            let recv = data.received() as f64 / 1_048_576.0;   // MB since last refresh
            let sent = data.transmitted() as f64 / 1_048_576.0;
            let factor = if elapsed > 0.1 { 1.0 / elapsed } else { 0.0 };
            NetInterface {
                name: name.clone(),
                down_mbps: recv * factor,
                up_mbps: sent * factor,
                total_rx_mb: (data.total_received() as f64) / 1_048_576.0,
                total_tx_mb: (data.total_transmitted() as f64) / 1_048_576.0,
            }
        })
        .collect();

    // Only update the baseline if at least 500ms passed
    // so rapid calls don't reset the delta to near-zero
    let (down_mbps, up_mbps) = if elapsed >= 0.5 {
        let rx_delta = total_rx.saturating_sub(state.last_rx) as f64 / 1_048_576.0;
        let tx_delta = total_tx.saturating_sub(state.last_tx) as f64 / 1_048_576.0;
        let down = rx_delta / elapsed;
        let up = tx_delta / elapsed;
        state.last_rx = total_rx;
        state.last_tx = total_tx;
        state.last_refresh = now;
        (down, up)
    } else {
        // Too soon — report last known rate (approximate by reusing last delta)
        // Compute based on last known state without updating baseline
        let rx_delta = total_rx.saturating_sub(state.last_rx) as f64 / 1_048_576.0;
        let tx_delta = total_tx.saturating_sub(state.last_tx) as f64 / 1_048_576.0;
        let since = now.duration_since(state.last_refresh).as_secs_f64().max(0.1);
        (rx_delta / since, tx_delta / since)
    };

    let connections = state
        .networks
        .iter()
        .filter(|(_, d)| d.received() > 0 || d.transmitted() > 0)
        .count();

    (down_mbps, up_mbps, connections, interfaces)
}

// ---------- Snapshot ----------
#[tauri::command]
async fn get_system_snapshot() -> Snapshot {
    refresh_system();

    let (cpu_usage, total_ram, used_ram) = {
        let sys = SYS.lock().unwrap();
        let total = sys.total_memory() as f64 / 1_073_741_824.0;
        let used = sys.used_memory() as f64 / 1_073_741_824.0;
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

    let (net_down, net_up, net_conns, _) = sample_network();

    // ---------- Whisper logic ----------
    let whisper = if net_down > 20.0 {
        format!(
            "Heavy download: {:.1} MB/s coming in. Something big is updating or streaming.",
            net_down
        )
    } else if net_up > 10.0 {
        format!(
            "Heavy upload: {:.1} MB/s going out. Backing up or syncing?",
            net_up
        )
    } else if ram_percent > 85.0 {
        format!("RAM is {:.0}% full ({:.1} / {:.1} GB). Click RAM to see the hog.", ram_percent, used_ram, total_ram)
    } else if disk_used_percent > 90.0 {
        format!("Disk is {:.0}% full ({:.0} / {:.0} GB). Click Disk to see what's big.", disk_used_percent, disk_used, disk_total)
    } else if cpu_usage > 70.0 {
        format!("CPU at {:.0}%. Something's working hard — click CPU to see what.", cpu_usage)
    } else if net_down > 0.5 || net_up > 0.5 {
        format!(
            "Everything calm — light network activity ({:.1} ↓ / {:.1} ↑ MB/s).",
            net_down, net_up
        )
    } else {
        format!(
            "All healthy — CPU {:.0}%, RAM {:.0}%, Disk {:.0}%.",
            cpu_usage, ram_percent, disk_used_percent
        )
    };

    Snapshot {
        cpu_usage,
        total_ram_gb: total_ram,
        used_ram_gb: used_ram,
        ram_percent,
        disk_total_gb: disk_total,
        disk_used_gb: disk_used,
        disk_used_percent,
        net_down_mbps: net_down,
        net_up_mbps: net_up,
        net_connections: net_conns,
        whisper,
    }
}

// ---------- Processes ----------
#[tauri::command]
async fn get_top_processes(sort_by: String) -> Vec<ProcessInfo> {
    refresh_system();

    let mut raw: Vec<(u32, String, f64, f32)> = {
        let sys = SYS.lock().unwrap();
        sys.processes()
            .iter()
            .map(|(pid, p)| {
                (
                    pid.as_u32(),
                    p.name().to_string_lossy().to_string(),
                    p.memory() as f64 / 1_048_576.0,
                    p.cpu_usage(),
                )
            })
            .collect()
    };

    if sort_by == "cpu" {
        raw.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        raw.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    }

    raw.into_iter()
        .take(8)
        .map(|(pid, name, ram_mb, cpu_usage)| {
            let icon_path = cached_icon_path(&name);
            ProcessInfo { pid, name, ram_mb, cpu_usage, icon_path }
        })
        .collect()
}

// ---------- Network command ----------
#[tauri::command]
async fn get_network_interfaces() -> Vec<NetInterface> {
    let (_, _, _, ifaces) = sample_network();
    let mut list = ifaces;
    list.sort_by(|a, b| {
        let at = a.down_mbps + a.up_mbps;
        let bt = b.down_mbps + b.up_mbps;
        bt.partial_cmp(&at).unwrap_or(std::cmp::Ordering::Equal)
    });
    list
}

// ---------- Icons ----------
fn cached_icon_path(name: &str) -> Option<String> {
    {
        let cache = ICON_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(name) {
            return cached.clone();
        }
    }
    let result = find_icon_path(name);
    ICON_CACHE.lock().unwrap().insert(name.to_string(), result.clone());
    result
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

// ---------- Desktop index (Linux icons) ----------
#[cfg(target_os = "linux")]
fn build_desktop_index() -> HashMap<String, String> {
    let mut map = HashMap::new();
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

            let icon_name = content.lines()
                .find(|l| l.starts_with("Icon="))
                .map(|l| l.trim_start_matches("Icon=").trim().to_string());
            let Some(icon_name) = icon_name else { continue };
            if icon_name.is_empty() { continue; }

            let resolved = if icon_name.starts_with('/') {
                Some(icon_name.clone())
            } else {
                resolve_icon_name(&icon_name)
            };
            let Some(real_path) = resolved else { continue };

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                map.entry(stem.to_lowercase()).or_insert_with(|| real_path.clone());
            }
            if let Some(exec_line) = content.lines().find(|l| l.starts_with("Exec=")) {
                let exec_val = exec_line.trim_start_matches("Exec=").trim();
                let first = exec_val.split_whitespace().next().unwrap_or("");
                if !first.is_empty() {
                    let bin = std::path::Path::new(first)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(first)
                        .to_lowercase();
                    map.entry(bin).or_insert_with(|| real_path.clone());
                }
            }
        }
    }
    map
}

#[cfg(target_os = "linux")]
fn find_icon_path(process_name: &str) -> Option<String> {
    let lower = process_name.to_lowercase();
    let skip = ["bash", "sh", "zsh", "fish", "systemd", "kworker", "kthread", "init", "dbus", "sshd"];
    if skip.iter().any(|s| lower.starts_with(s)) { return None; }

    {
        let mut idx = DESKTOP_INDEX.lock().unwrap();
        if idx.is_none() { *idx = Some(build_desktop_index()); }
    }
    let idx = DESKTOP_INDEX.lock().unwrap();
    let map = idx.as_ref().unwrap();

    if let Some(p) = map.get(&lower) { return Some(p.clone()); }
    for (key, path) in map.iter() {
        if lower.contains(key) || key.contains(&lower) { return Some(path.clone()); }
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

// ---------- Entry ----------
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_system_snapshot,
            get_top_processes,
            get_disk_usage,
            get_network_interfaces
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}