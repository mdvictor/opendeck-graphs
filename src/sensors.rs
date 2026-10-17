use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, MemoryRefreshKind, Networks, System};

static PREV_CPU_STATS: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static PREV_DISK_WRITE: Mutex<Option<u64>> = Mutex::new(None);
static PREV_DISK_READ: Mutex<Option<u64>> = Mutex::new(None);
static PREV_NET_RX: LazyLock<Mutex<HashMap<String, NetSample>>> = LazyLock::new(Default::default);
static PREV_NET_TX: LazyLock<Mutex<HashMap<String, NetSample>>> = LazyLock::new(Default::default);
/// A previous network counter reading, remembered per button instance
struct NetSample {
    selection: String,
    taken: Instant,
    total: u64,
}

/// Readings older than this are discarded so a long gap does not show up as one huge spike
const NET_SAMPLE_MAX_AGE: Duration = Duration::from_secs(10);

static NVML: OnceLock<Option<nvml_wrapper::Nvml>> = OnceLock::new();
static SYSINFO: Mutex<Option<System>> = Mutex::new(None);

/// Get the shared NVML instance, initializing it once on first use.
fn nvml() -> Option<&'static nvml_wrapper::Nvml> {
    NVML.get_or_init(|| nvml_wrapper::Nvml::init().ok())
        .as_ref()
}

/// Get RAM info (used, total) using a shared System instance.
fn ram_info() -> (u64, u64) {
    let mut guard = SYSINFO.lock().unwrap();
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    (sys.used_memory(), sys.total_memory())
}

#[derive(Debug, Clone, Copy)]
enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

fn detect_gpu_vendor_for_card(card: &str) -> GpuVendor {
    // Check AMD
    if Path::new(&format!("/sys/class/drm/{}/device/gpu_busy_percent", card)).exists() {
        return GpuVendor::Amd;
    }

    // Check Intel via sysfs frequency file
    if Path::new(&format!("/sys/class/drm/{}/gt_cur_freq_mhz", card)).exists() {
        return GpuVendor::Intel;
    }

    // Check via PCI vendor ID
    let vendor_path = format!("/sys/class/drm/{}/device/vendor", card);
    if let Ok(vendor) = fs::read_to_string(&vendor_path) {
        match vendor.trim() {
            "0x8086" => return GpuVendor::Intel,
            "0x10de" => return GpuVendor::Nvidia,
            "0x1002" => return GpuVendor::Amd,
            _ => {}
        }
    }

    GpuVendor::Unknown
}

/// Represents a detected GPU with its card name and display name
#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub card: String,
    pub name: String,
}

/// Enumerate all available GPUs from /sys/class/drm
pub fn list_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    // Check for NVIDIA GPUs via NVML
    if Path::new("/proc/driver/nvidia/version").exists() {
        if let Some(nv) = nvml() {
            if let Ok(count) = nv.device_count() {
                for i in 0..count {
                    if let Ok(device) = nv.device_by_index(i) {
                        let name = device
                            .name()
                            .unwrap_or_else(|_| format!("NVIDIA GPU {}", i));
                        gpus.push(GpuInfo {
                            card: format!("nvidia:{}", i),
                            name: format!("NVIDIA {} (GPU {})", name, i),
                        });
                    }
                }
            }
        }
    }

    // Enumerate DRM cards
    let drm_path = Path::new("/sys/class/drm");
    if let Ok(entries) = fs::read_dir(drm_path) {
        let mut cards: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Only top-level cardN entries (not cardN-HDMI-A-1 etc.)
                if name.starts_with("card") && name.chars().skip(4).all(|c| c.is_ascii_digit()) {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        cards.sort();

        for card in cards {
            // Try to read GPU name from device/product_name or uevent
            let name = read_gpu_name(&card);
            let vendor = detect_gpu_vendor_for_card(&card);
            let vendor_prefix = match vendor {
                GpuVendor::Amd => "AMD",
                GpuVendor::Intel => "Intel",
                _ => "GPU",
            };
            let display_name = if name.is_empty() {
                format!("{} {} ({})", vendor_prefix, card, card)
            } else {
                format!("{} - {}", card, name)
            };
            gpus.push(GpuInfo {
                card: card.clone(),
                name: display_name,
            });
        }
    }

    gpus
}

fn read_gpu_name(card: &str) -> String {
    // Try PCI device name from uevent
    let uevent_path = format!("/sys/class/drm/{}/device/uevent", card);
    if let Ok(content) = fs::read_to_string(&uevent_path) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("PCI_ID=") {
                // val is something like "1002:744C" — look up in pci.ids or just return the ID
                return val.to_string();
            }
        }
    }

    // Try hwmon name as fallback
    let hwmon_base = format!("/sys/class/drm/{}/device/hwmon", card);
    if let Ok(entries) = fs::read_dir(&hwmon_base) {
        for entry in entries.flatten() {
            let name_path = entry.path().join("name");
            if let Ok(name) = fs::read_to_string(name_path) {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }

    String::new()
}

/// Find CPU load percentage by reading /proc/stat
pub async fn find_cpu_load() -> Result<f32> {
    let stat_content = tokio::fs::read_to_string("/proc/stat").await?;

    if let Some(cpu_line) = stat_content.lines().next() {
        if cpu_line.starts_with("cpu ") {
            let parts: Vec<&str> = cpu_line.split_whitespace().collect();
            if parts.len() >= 8 {
                let user: u64 = parts[1].parse().unwrap_or(0);
                let nice: u64 = parts[2].parse().unwrap_or(0);
                let system: u64 = parts[3].parse().unwrap_or(0);
                let idle: u64 = parts[4].parse().unwrap_or(0);
                let iowait: u64 = parts[5].parse().unwrap_or(0);
                let irq: u64 = parts[6].parse().unwrap_or(0);
                let softirq: u64 = parts[7].parse().unwrap_or(0);

                let total = user + nice + system + idle + iowait + irq + softirq;
                let active = user + nice + system + irq + softirq;

                // Calculate CPU usage based on delta from previous reading
                let mut prev_stats = PREV_CPU_STATS.lock().unwrap();
                let cpu_usage = if let Some((prev_total, prev_active)) = *prev_stats {
                    let total_delta = total.saturating_sub(prev_total);
                    let active_delta = active.saturating_sub(prev_active);
                    if total_delta > 0 {
                        (active_delta as f32 / total_delta as f32) * 100.0
                    } else {
                        0.0
                    }
                } else {
                    // First reading, return 0
                    0.0
                };

                *prev_stats = Some((total, active));
                return Ok(cpu_usage);
            }
        }
    }

    Ok(0.0)
}

/// Find RAM usage percentage
pub async fn find_ram_usage() -> Result<f32> {
    let (used, total) = ram_info();
    if total > 0 {
        Ok((used as f32 / total as f32) * 100.0)
    } else {
        Ok(0.0)
    }
}

/// Find RAM usage in gigabytes
pub async fn find_ram_usage_gb() -> Result<f32> {
    let (used, _) = ram_info();
    Ok(used as f32 / 1_073_741_824.0)
}

/// Find total RAM in gigabytes
pub async fn find_ram_total_gb() -> Result<f32> {
    let (_, total) = ram_info();
    Ok(total as f32 / 1_073_741_824.0)
}

/// Find RAM temperature from sysinfo
pub async fn find_ram_temperature() -> Result<f32> {
    let components = Components::new_with_refreshed_list();

    for component in &components {
        let label = component.label();
        // Look for SPD5118 or other RAM temperature sensors
        if label.contains("spd5118") || label.contains("SPD5118") {
            if let Some(temp) = component.temperature() {
                return Ok(temp);
            }
        }
    }

    log::warn!("RAM temperature sensor not found");
    Ok(0.0)
}

/// Find disk write speed in MB/s
pub async fn find_disk_write() -> Result<f32> {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(true);

    let mut total_written = 0u64;
    for disk in disks.list() {
        total_written += disk.usage().total_written_bytes;
    }

    let mut prev = PREV_DISK_WRITE.lock().unwrap();
    let write_speed = if let Some(prev_written) = *prev {
        let delta = total_written.saturating_sub(prev_written);
        (delta as f32) / 1_048_576.0 // Convert to MB/s
    } else {
        0.0
    };

    *prev = Some(total_written);
    Ok(write_speed)
}

/// Find disk read speed in MB/s
pub async fn find_disk_read() -> Result<f32> {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(true);

    let mut total_read = 0u64;
    for disk in disks.list() {
        total_read += disk.usage().total_read_bytes;
    }

    let mut prev = PREV_DISK_READ.lock().unwrap();
    let read_speed = if let Some(prev_read) = *prev {
        let delta = total_read.saturating_sub(prev_read);
        (delta as f32) / 1_048_576.0 // Convert to MB/s
    } else {
        0.0
    };

    *prev = Some(total_read);
    Ok(read_speed)
}

/// Network interface selector value meaning "every interface except loopback"
pub const NET_ALL: &str = "all";
/// Network interface selector value meaning "physical interfaces"
pub const NET_PHYSICAL: &str = "physical";

pub struct NetInterfaceInfo {
    pub name: String,
    pub physical: bool,
}

/// An interface is physical when it is backed by a device (virtual ones like
/// lo, bridges, veth, tun/wg or ifb have no `device` link in sysfs).
fn is_physical_interface(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).join("device").exists()
}

/// List all network interfaces except loopback
pub fn list_net_interfaces() -> Vec<NetInterfaceInfo> {
    let mut interfaces: Vec<NetInterfaceInfo> = Networks::new_with_refreshed_list()
        .iter()
        .filter(|(name, _)| name.as_str() != "lo")
        .map(|(name, _)| NetInterfaceInfo {
            name: name.clone(),
            physical: is_physical_interface(name),
        })
        .collect();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

/// Sum a counter over the interfaces matching `selection` (None/"physical", "all" or an interface name)
fn sum_net_counter(selection: Option<&str>, counter: impl Fn(&sysinfo::NetworkData) -> u64) -> u64 {
    let networks = Networks::new_with_refreshed_list();
    let selection = selection.unwrap_or(NET_PHYSICAL);

    networks
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            if name == "lo" {
                return false;
            }
            match selection {
                NET_ALL => true,
                NET_PHYSICAL => is_physical_interface(name),
                selected => name == selected,
            }
        })
        .map(|(_, data)| counter(data))
        .sum()
}

/// Turn a counter total into a MB/s delta, tracking the previous reading per button instance.
/// A changed selection or a stale reading starts over instead of reporting a bogus delta.
fn net_speed(
    prev: &Mutex<HashMap<String, NetSample>>,
    instance_id: &str,
    selection: Option<&str>,
    total: u64,
) -> f32 {
    let selection = selection.unwrap_or(NET_PHYSICAL);
    let now = Instant::now();
    let mut prev = prev.lock().unwrap();

    let speed = match prev.get(instance_id) {
        Some(sample)
            if sample.selection == selection
                && now.duration_since(sample.taken) <= NET_SAMPLE_MAX_AGE =>
        {
            (total.saturating_sub(sample.total) as f32) / 1_048_576.0 // Convert to MB/s
        }
        _ => 0.0,
    };

    prev.insert(
        instance_id.to_string(),
        NetSample {
            selection: selection.to_string(),
            taken: now,
            total,
        },
    );
    speed
}

/// Find network download speed in MB/s
pub async fn find_net_download(instance_id: &str, selection: Option<&str>) -> Result<f32> {
    let total_rx = sum_net_counter(selection, |n| n.total_received());
    Ok(net_speed(&PREV_NET_RX, instance_id, selection, total_rx))
}

/// Find network upload speed in MB/s
pub async fn find_net_upload(instance_id: &str, selection: Option<&str>) -> Result<f32> {
    let total_tx = sum_net_counter(selection, |n| n.total_transmitted());
    Ok(net_speed(&PREV_NET_TX, instance_id, selection, total_tx))
}

/// Read a hwmon sysfs value as f32, dividing by 1000 (millidegrees → degrees, millivolts → volts)
fn read_hwmon_millis(path: &Path) -> Option<f32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| v / 1000.0)
}

fn read_hwmon_raw(path: &Path) -> Option<f32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
}

/// Iterate /sys/class/hwmon entries, yielding (hwmon_path, chip_name)
fn iter_hwmon() -> impl Iterator<Item = (std::path::PathBuf, String)> {
    let base = Path::new("/sys/class/hwmon");
    let entries = fs::read_dir(base)
        .map(|e| e.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    entries.into_iter().filter_map(|entry| {
        let hwmon_path = entry.path();
        let name = fs::read_to_string(hwmon_path.join("name"))
            .or_else(|_| fs::read_to_string(hwmon_path.join("device/name")))
            .unwrap_or_default();
        Some((hwmon_path, name.trim().to_string()))
    })
}

pub async fn find_cpu_temperature() -> Result<f32> {
    let priority_labels = ["Package id 0", "Tdie", "Tctl", "Tccd1", "Core 0"];

    for (hwmon_path, chip_name) in iter_hwmon() {
        if !chip_name.contains("coretemp") &&
            !chip_name.contains("k10temp") &&
            !chip_name.contains("zenpower") {
            continue;
        }
        for n in 1..=32u32 {
            let label_path = hwmon_path.join(format!("temp{}_label", n));
            let input_path = hwmon_path.join(format!("temp{}_input", n));
            if !input_path.exists() {
                break;
            }
            if let Ok(label) = fs::read_to_string(&label_path) {
                if priority_labels.iter().any(|&p| label.trim().contains(p)) {
                    if let Some(temp) = read_hwmon_millis(&input_path) {
                        return Ok(temp);
                    }
                }
            }
        }
        if let Some(temp) = read_hwmon_millis(&hwmon_path.join("temp1_input")) {
            return Ok(temp);
        }
    }
    Ok(0.0)
}

/// Find GPU load percentage for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_load(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(utilization) = device.utilization_rates() {
                    return Ok(utilization.gpu as f32);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd => {
                let path = format!("/sys/class/drm/{}/device/gpu_busy_percent", card);
                if let Ok(load_str) = fs::read_to_string(&path) {
                    if let Ok(load) = load_str.trim().parse::<f32>() {
                        return Ok(load);
                    }
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(utilization) = device.utilization_rates() {
                            return Ok(utilization.gpu as f32);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU load sensor not found for card: {}", card);
    Ok(0.0)
}

/// Find GPU temperature for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_temperature(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(temp) =
                    device.temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                {
                    return Ok(temp as f32);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd | GpuVendor::Intel => {
                let hwmon_path = format!("/sys/class/drm/{}/device/hwmon", card);
                if let Ok(entries) = fs::read_dir(&hwmon_path) {
                    for entry in entries.flatten() {
                        let temp_path = entry.path().join("temp1_input");
                        if let Ok(temp_str) = fs::read_to_string(temp_path) {
                            if let Ok(temp_millis) = temp_str.trim().parse::<f32>() {
                                return Ok(temp_millis / 1000.0);
                            }
                        }
                    }
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(temp) = device.temperature(
                            nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu,
                        ) {
                            return Ok(temp as f32);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU temperature sensor not found for card: {}", card);
    Ok(0.0)
}

/// Find GPU VRAM usage percentage for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_vram(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(mem_info) = device.memory_info() {
                    let pct = (mem_info.used as f64 / mem_info.total as f64) * 100.0;
                    return Ok(pct as f32);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd => {
                let base = format!("/sys/class/drm/{}/device", card);
                let used = read_sysfs_f32(&format!("{}/mem_info_vram_used", base));
                let total = read_sysfs_f32(&format!("{}/mem_info_vram_total", base));
                if let (Ok(u), Ok(t)) = (used, total) {
                    if t > 0.0 {
                        return Ok((u / t) * 100.0);
                    }
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(mem_info) = device.memory_info() {
                            let pct = (mem_info.used as f64 / mem_info.total as f64) * 100.0;
                            return Ok(pct as f32);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU VRAM sensor not found for card: {}", card);
    Ok(0.0)
}

/// Find GPU VRAM usage in gigabytes for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_vram_gb(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(mem_info) = device.memory_info() {
                    return Ok(mem_info.used as f32 / 1_073_741_824.0);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd => {
                let base = format!("/sys/class/drm/{}/device", card);
                if let Ok(used) = read_sysfs_f32(&format!("{}/mem_info_vram_used", base)) {
                    return Ok(used / 1_073_741_824.0);
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(mem_info) = device.memory_info() {
                            return Ok(mem_info.used as f32 / 1_073_741_824.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU VRAM sensor not found for card: {}", card);
    Ok(0.0)
}

/// Find total GPU VRAM in gigabytes for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_vram_total_gb(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(mem_info) = device.memory_info() {
                    return Ok(mem_info.total as f32 / 1_073_741_824.0);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd => {
                let base = format!("/sys/class/drm/{}/device", card);
                if let Ok(total) = read_sysfs_f32(&format!("{}/mem_info_vram_total", base)) {
                    return Ok(total / 1_073_741_824.0);
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(mem_info) = device.memory_info() {
                            return Ok(mem_info.total as f32 / 1_073_741_824.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU VRAM total not found for card: {}", card);
    Ok(0.0)
}

/// Find GPU power draw in watts for a specific card (e.g. "card0" or "nvidia:0")
pub async fn find_gpu_power(card: &str) -> Result<f32> {
    if let Some(idx_str) = card.strip_prefix("nvidia:") {
        let idx: u32 = idx_str.parse().unwrap_or(0);
        if let Some(nv) = nvml() {
            if let Ok(device) = nv.device_by_index(idx) {
                if let Ok(power_mw) = device.power_usage() {
                    return Ok(power_mw as f32 / 1000.0);
                }
            }
        }
    } else {
        match detect_gpu_vendor_for_card(card) {
            GpuVendor::Amd => {
                let hwmon_path = format!("/sys/class/drm/{}/device/hwmon", card);
                if let Some(path) = first_hwmon_dir(&hwmon_path) {
                    let power_path = format!("{}/power1_average", path);
                    if let Ok(microwatts) = read_sysfs_f32(&power_path) {
                        return Ok(microwatts / 1_000_000.0);
                    }
                }
            }
            GpuVendor::Nvidia => {
                if let Some(nv) = nvml() {
                    if let Ok(device) = nv.device_by_index(0) {
                        if let Ok(power_mw) = device.power_usage() {
                            return Ok(power_mw as f32 / 1000.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    log::warn!("GPU power sensor not found for card: {}", card);
    Ok(0.0)
}

/// Read a sysfs file as f32
fn read_sysfs_f32(path: &str) -> Result<f32> {
    let content = fs::read_to_string(path)?;
    Ok(content.trim().parse()?)
}

/// Return the first hwmon directory path inside the given base directory.
fn first_hwmon_dir(base: &str) -> Option<String> {
    let entries = fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("hwmon") {
            return Some(entry.path().to_string_lossy().into_owned());
        }
    }
    None
}

/// Find motherboard temperature via hwmon sysfs
pub async fn find_motherboard_temperature() -> Result<f32> {
    for (hwmon_path, chip_name) in iter_hwmon() {
        if !chip_name.contains("nct") && !chip_name.contains("it87") {
            continue;
        }
        for n in 1..=32u32 {
            let label_path = hwmon_path.join(format!("temp{}_label", n));
            let input_path = hwmon_path.join(format!("temp{}_input", n));
            if !input_path.exists() {
                break;
            }
            if let Ok(label) = fs::read_to_string(&label_path) {
                let label = label.trim();
                if label.contains("SYSTIN") || label.contains("MB") {
                    if let Some(temp) = read_hwmon_millis(&input_path) {
                        return Ok(temp);
                    }
                }
            }
        }
    }
    Ok(0.0)
}

pub async fn find_nvme_temperature() -> Result<f32> {
    for (hwmon_path, chip_name) in iter_hwmon() {
        if chip_name.contains("nvme") {
            if let Some(temp) = read_hwmon_millis(&hwmon_path.join("temp1_input")) {
                return Ok(temp);
            }
        }
    }
    Ok(0.0)
}

pub async fn find_system_fan_speed(fan_number: u32) -> Result<f32> {
    for (hwmon_path, _chip_name) in iter_hwmon() {
        let fan_path = hwmon_path.join(format!("fan{}_input", fan_number));
        if let Some(rpm) = read_hwmon_raw(&fan_path) {
            return Ok(rpm);
        }
    }
    log::warn!("System fan {} sensor not found", fan_number);
    Ok(0.0)
}

pub async fn find_cpu_voltage() -> Result<f32> {
    for (hwmon_path, _chip_name) in iter_hwmon() {
        for n in 0..=32u32 {
            let label_path = hwmon_path.join(format!("in{}_label", n));
            let input_path = hwmon_path.join(format!("in{}_input", n));
            if !input_path.exists() {
                break;
            }
            if let Ok(label) = fs::read_to_string(&label_path) {
                let label = label.trim();
                if label.contains("CPU") && (label.contains("Vcore") || label.contains("in")) {
                    if let Some(v) = read_hwmon_millis(&input_path) {
                        return Ok(v);
                    }
                }
            }
        }
    }
    Ok(0.0)
}
