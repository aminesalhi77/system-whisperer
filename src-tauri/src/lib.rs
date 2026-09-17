use serde::Serialize;
use sysinfo::{Disks, System};

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
}

#[tauri::command]
fn get_system_snapshot() -> Snapshot {
    let mut sys = System::new_all();
    sys.refresh_all();
    std::thread::sleep(std::time::Duration::from_millis(300));
    sys.refresh_all();

    let cpu_usage = sys.global_cpu_usage();

    let total_ram = sys.total_memory() as f64 / 1_073_741_824.0;
    let used_ram  = sys.used_memory()  as f64 / 1_073_741_824.0;
    let ram_percent = if total_ram > 0.0 { (used_ram / total_ram) * 100.0 } else { 0.0 };

    // Root disk info
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

    let disk_used_percent = if disk_total > 0.0 {
        (disk_used / disk_total) * 100.0
    } else {
        0.0
    };

    let whisper = if ram_percent > 85.0 {
        format!("RAM is {:.0}% full ({:.1} / {:.1} GB). Click RAM to see the hog.", ram_percent, used_ram, total_ram)
    } else if disk_used_percent > 90.0 {
        format!("Disk is {:.0}% full ({:.0} / {:.0} GB). Time to clean up.", disk_used_percent, disk_used, disk_total)
    } else if cpu_usage > 70.0 {
        format!("CPU at {:.0}%. Something's working hard.", cpu_usage)
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
        whisper,
    }
}

#[tauri::command]
fn get_top_processes(sort_by: String) -> Vec<ProcessInfo> {
    let mut sys = System::new_all();
    sys.refresh_all();
    std::thread::sleep(std::time::Duration::from_millis(300));
    sys.refresh_all();

    let mut procs: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().to_string(),
            ram_mb: p.memory() as f64 / 1_048_576.0,
            cpu_usage: p.cpu_usage(),
        })
        .collect();

    if sort_by == "cpu" {
        procs.sort_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        procs.sort_by(|a, b| b.ram_mb.partial_cmp(&a.ram_mb).unwrap_or(std::cmp::Ordering::Equal));
    }

    procs.into_iter().take(8).collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_system_snapshot,
            get_top_processes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}