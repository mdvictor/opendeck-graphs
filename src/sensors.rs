use anyhow::Result;
use lm_sensors as sensors;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use sysinfo::{Components, Disks, Networks, System};

static PREV_CPU_STATS: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static PREV_DISK_WRITE: Mutex<Option<u64>> = Mutex::new(None);
static PREV_DISK_READ: Mutex<Option<u64>> = Mutex::new(None);
static PREV_NET_RX: Mutex<Option<u64>> = Mutex::new(None);
static PREV_NET_TX: Mutex<Option<u64>> = Mutex::new(None);

#[derive(Debug, Clone, Copy)]
enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

fn detect_gpu_vendor() -> GpuVendor {
    // Check NVIDIA
    if Path::new("/proc/driver/nvidia/version").exists() {
        return GpuVendor::Nvidia;
    }

    // Check AMD
    if Path::new("/sys/class/drm/card0/device/gpu_busy_percent").exists() {
        return GpuVendor::Amd;
    }

    // Check Intel
    if Path::new("/sys/class/drm/card0/gt_cur_freq_mhz").exists() {
        return GpuVendor::Intel;
    }

    GpuVendor::Unknown
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
    let mut sys = System::new_all();
    sys.refresh_memory();

    let total = sys.total_memory();
    let used = sys.used_memory();

    if total > 0 {
        Ok((used as f32 / total as f32) * 100.0)
    } else {
        Ok(0.0)
    }
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

/// Find network download speed in MB/s
pub async fn find_net_download() -> Result<f32> {
    let networks = Networks::new_with_refreshed_list();

    let mut total_rx = 0u64;
    for (_interface_name, network) in &networks {
        total_rx += network.total_received();
    }

    let mut prev = PREV_NET_RX.lock().unwrap();
    let download_speed = if let Some(prev_rx) = *prev {
        let delta = total_rx.saturating_sub(prev_rx);
        (delta as f32) / 1_048_576.0 // Convert to MB/s
    } else {
        0.0
    };

    *prev = Some(total_rx);
    Ok(download_speed)
}

/// Find network upload speed in MB/s
pub async fn find_net_upload() -> Result<f32> {
    let networks = Networks::new_with_refreshed_list();

    let mut total_tx = 0u64;
    for (_interface_name, network) in &networks {
        total_tx += network.total_transmitted();
    }

    let mut prev = PREV_NET_TX.lock().unwrap();
    let upload_speed = if let Some(prev_tx) = *prev {
        let delta = total_tx.saturating_sub(prev_tx);
        (delta as f32) / 1_048_576.0 // Convert to MB/s
    } else {
        0.0
    };

    *prev = Some(total_tx);
    Ok(upload_speed)
}
/// Find CPU temperature from lm-sensors
pub async fn find_cpu_temperature() -> Result<f32> {
    let sensors_lib = sensors::Initializer::default().initialize()?;

    for chip in sensors_lib.chip_iter(None) {
        let chip_name = format!("{}", chip);
        if chip_name.contains("coretemp") || chip_name.contains("k10temp") {
            for feature in chip.feature_iter() {
                if let Ok(label) = feature.label() {
                    if label.contains("Package")
                        || label.contains("Tdie")
                        || label.contains("Core 0")
                    {
                        for sub_feature in feature.sub_feature_iter() {
                            if let Ok(value) = sub_feature.value() {
                                return Ok(value.raw_value() as f32);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(0.0)
}

/// Find GPU load percentage
pub async fn find_gpu_load() -> Result<f32> {
    match detect_gpu_vendor() {
        GpuVendor::Amd => {
            // AMD GPU load from sysfs
            if let Ok(load_str) = fs::read_to_string("/sys/class/drm/card0/device/gpu_busy_percent")
            {
                if let Ok(load) = load_str.trim().parse::<f32>() {
                    return Ok(load);
                }
            }
        }
        GpuVendor::Nvidia => {
            // NVIDIA GPU load using NVML
            if let Ok(nvml) = nvml_wrapper::Nvml::init() {
                if let Ok(device) = nvml.device_by_index(0) {
                    if let Ok(utilization) = device.utilization_rates() {
                        return Ok(utilization.gpu as f32);
                    }
                }
            }
        }
        _ => {}
    }

    log::warn!("GPU load sensor not found");
    Ok(0.0)
}

/// Find GPU temperature from lm-sensors
pub async fn find_gpu_temperature() -> Result<f32> {
    match detect_gpu_vendor() {
        GpuVendor::Amd => {
            // AMD GPU temperature from sysfs hwmon
            let hwmon_path = "/sys/class/drm/card0/device/hwmon";
            if let Ok(entries) = fs::read_dir(hwmon_path) {
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
            // NVIDIA GPU temperature using NVML
            if let Ok(nvml) = nvml_wrapper::Nvml::init() {
                if let Ok(device) = nvml.device_by_index(0) {
                    if let Ok(temp) = device
                        .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                    {
                        return Ok(temp as f32);
                    }
                }
            }
        }
        _ => {}
    }

    log::warn!("GPU temperature sensor not found");
    Ok(0.0)
}

/// Find motherboard temperature from lm-sensors
pub async fn find_motherboard_temperature() -> Result<f32> {
    let sensors_lib = sensors::Initializer::default().initialize()?;

    for chip in sensors_lib.chip_iter(None) {
        let chip_name = format!("{}", chip);
        if chip_name.contains("nct") || chip_name.contains("it87") {
            for feature in chip.feature_iter() {
                if let Ok(label) = feature.label() {
                    if label.contains("SYSTIN") || label.contains("MB") {
                        for sub_feature in feature.sub_feature_iter() {
                            if let Ok(value) = sub_feature.value() {
                                return Ok(value.raw_value() as f32);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(0.0)
}

/// Find NVMe temperature from lm-sensors
pub async fn find_nvme_temperature() -> Result<f32> {
    let sensors_lib = sensors::Initializer::default().initialize()?;

    for chip in sensors_lib.chip_iter(None) {
        let chip_name = format!("{}", chip);
        if chip_name.contains("nvme") {
            for feature in chip.feature_iter() {
                for sub_feature in feature.sub_feature_iter() {
                    if let Ok(value) = sub_feature.value() {
                        return Ok(value.raw_value() as f32);
                    }
                }
            }
        }
    }
    Ok(0.0)
}

/// Find system fan speed from lm-sensors by fan number
pub async fn find_system_fan_speed(fan_number: u32) -> Result<f32> {
    let sensors_lib = sensors::Initializer::default().initialize()?;

    let target_fan = format!("fan{}", fan_number);

    for chip in sensors_lib.chip_iter(None) {
        for feature in chip.feature_iter() {
            // Check if this is a fan feature
            if let Some(kind) = feature.kind() {
                if matches!(kind, sensors::feature::Kind::Fan) {
                    // Check the feature name (like "fan1", "fan2", etc.)
                    if let Some(Ok(feature_name)) = feature.name() {
                        if feature_name == target_fan {
                            for sub_feature in feature.sub_feature_iter() {
                                if let Some(Ok(name)) = sub_feature.name() {
                                    if name.contains("input") {
                                        if let Ok(value) = sub_feature.value() {
                                            return Ok(value.raw_value() as f32);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    log::warn!("System fan {} sensor not found", fan_number);
    Ok(0.0)
}

/// Find CPU voltage from lm-sensors
pub async fn find_cpu_voltage() -> Result<f32> {
    let sensors_lib = sensors::Initializer::default().initialize()?;

    for chip in sensors_lib.chip_iter(None) {
        for feature in chip.feature_iter() {
            if let Ok(label) = feature.label() {
                if label.contains("CPU") && (label.contains("Vcore") || label.contains("in")) {
                    for sub_feature in feature.sub_feature_iter() {
                        if let Ok(value) = sub_feature.value() {
                            return Ok(value.raw_value() as f32);
                        }
                    }
                }
            }
        }
    }
    Ok(0.0)
}
