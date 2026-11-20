//! One read per requested counter group and tick, shared by every visible key.
use crate::graph_data::{GraphSettings, MemoryDisplay, MetricType};
use crate::pdh_rates::{Groups, Sampler};
use anyhow::{anyhow, bail, Result};
use nvml_wrapper::{enum_wrappers::device::TemperatureSensor, Nvml};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use sysinfo::{MemoryRefreshKind, System};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use wmi::WMIConnection;

const MIB: f64 = 1_048_576.0;
const GIB: f64 = 1_073_741_824.0;

/// A missing provider reading is represented by null.
pub fn probe() -> serde_json::Value {
    use MetricType::*;
    let metrics = [
        CpuTemp,
        CpuPackageTemp,
        CpuLoad,
        GpuTemp,
        GpuLoad,
        GpuVram,
        GpuPower,
        MotherboardTemp,
        NvmeTemp,
        SystemFan,
        CpuVoltage,
        DiskWrite,
        DiskRead,
        RamUsage,
        RamTemp,
        NetDownload,
        NetUpload,
    ];
    let snap = Snapshot::sample(
        metrics.iter().copied(),
        &mut Sampler::default(),
        |_, _, _| true,
    );
    let values = |card: Option<String>, gpu: bool| {
        let mut values = serde_json::Map::new();
        for m in metrics {
            if matches!(m, GpuTemp | GpuLoad | GpuVram | GpuPower) != gpu {
                continue;
            }
            let settings = GraphSettings {
                metric_type: m,
                gpu_card: card.clone(),
                ..Default::default()
            };
            values.insert(
                m.display_name().into(),
                serde_json::json!(snap.read(&settings).ok()),
            );
        }
        values
    };
    let adapters: Vec<_> = snap.gpus.iter().map(|g| serde_json::json!({"name":g.name,"id":g.card,"values":values(Some(g.card.clone()),true)})).collect();
    serde_json::json!({"system":values(None,false),"adapters":adapters,"typeface":crate::gfx::typeface::name()})
}

#[derive(Clone)]
pub struct GpuInfo {
    pub card: String,
    pub name: String,
    luid: String,
    capacity: u64,
    nvidia_uuid: Option<String>,
    native: NativeGpu,
}

#[derive(Clone, Default)]
struct NativeGpu {
    load: Option<f32>,
    memory: Option<(u64, u64)>,
    temp: Option<f32>,
    power: Option<f32>,
}

const GPU_METRICS: [MetricType; 4] = [
    MetricType::GpuLoad,
    MetricType::GpuVram,
    MetricType::GpuTemp,
    MetricType::GpuPower,
];

pub struct Request {
    metric: MetricType,
    card: Option<String>,
}

impl From<&GraphSettings> for Request {
    fn from(settings: &GraphSettings) -> Self {
        Self {
            metric: settings.metric_type,
            card: settings.gpu_card.clone(),
        }
    }
}

fn matches_adapter(selection: Option<&str>, id: &str, primary: bool) -> bool {
    match selection {
        None | Some("" | "card0") => primary,
        Some(selected) => selected == id,
    }
}

impl NativeGpu {
    /// Fetch a requested counter once. A valid zero is a successful reading.
    fn fetch(&mut self, metric: MetricType, dev: Option<&nvml_wrapper::Device<'_>>) -> bool {
        let Some(dev) = dev else { return false };
        match metric {
            MetricType::GpuLoad => {
                self.load = dev.utilization_rates().ok().map(|v| v.gpu as f32);
                self.load.is_some()
            }
            MetricType::GpuVram => {
                self.memory = dev.memory_info().ok().map(|v| (v.used, v.total));
                self.memory.is_some()
            }
            MetricType::GpuTemp => {
                self.temp = dev
                    .temperature(TemperatureSensor::Gpu)
                    .ok()
                    .map(|v| v as f32);
                self.temp.is_some()
            }
            MetricType::GpuPower => {
                self.power = dev.power_usage().ok().map(|v| v as f32 / 1000.0);
                self.power.is_some()
            }
            _ => false,
        }
    }
}

fn nvml() -> Option<&'static Nvml> {
    static LIBRARY: OnceLock<Option<Nvml>> = OnceLock::new();
    LIBRARY.get_or_init(|| Nvml::init().ok()).as_ref()
}

fn nvidia_devices() -> Vec<(u32, String)> {
    let Some(lib) = nvml() else { return vec![] };
    (0..lib.device_count().unwrap_or(0))
        .filter_map(|i| {
            let dev = lib.device_by_index(i).ok()?;
            Some((dev.pci_info().ok()?.pci_device_id, dev.uuid().ok()?))
        })
        .collect()
}

pub fn list_gpus() -> Vec<GpuInfo> {
    // The factory owns adapter references. GetDesc1 returns a copied descriptor.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return vec![];
    };
    let nvidia = nvidia_devices();
    let mut cards = vec![];
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        let Ok(d) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let name = String::from_utf16_lossy(
            &d.Description[..d
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(d.Description.len())],
        );
        let luid = format!(
            "luid_0x{:08x}_0x{:08x}",
            d.AdapterLuid.HighPart, d.AdapterLuid.LowPart
        );
        let matching: Vec<_> = nvidia
            .iter()
            .filter(|(pci, _)| *pci == (d.DeviceId << 16 | d.VendorId))
            .collect();
        let nvidia_uuid = match matching.as_slice() {
            [(_, uuid)] => Some(uuid.clone()),
            _ => None,
        };
        let card = nvidia_uuid.clone().unwrap_or_else(|| {
            format!(
                "pci:{:04x}:{:04x}:{:08x}",
                d.VendorId, d.DeviceId, d.SubSysId
            )
        });
        cards.push(GpuInfo {
            card,
            name,
            luid,
            capacity: d.DedicatedVideoMemory as u64,
            nvidia_uuid,
            native: NativeGpu::default(),
        });
    }
    // Identical PCI models without a unique vendor UUID need session-local IDs.
    // A stale selection then becomes unavailable instead of silently switching cards.
    let mut counts = HashMap::new();
    for gpu in &cards {
        *counts.entry(gpu.card.clone()).or_insert(0) += 1;
    }
    for gpu in &mut cards {
        if counts[&gpu.card] > 1 {
            gpu.card = format!("{}:{}", gpu.card, gpu.luid);
            gpu.name = format!("{} ({})", gpu.name, gpu.luid);
            gpu.nvidia_uuid = None;
        }
    }
    cards.sort_by(|a, b| {
        b.capacity
            .cmp(&a.capacity)
            .then_with(|| a.card.cmp(&b.card))
    });
    cards
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Processor {
    percent_processor_time: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Disk {
    disk_read_bytes_persec: u64,
    disk_write_bytes_persec: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Network {
    bytes_received_persec: u64,
    bytes_sent_persec: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AdapterMemory {
    name: String,
    dedicated_usage: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Engine {
    name: String,
    utilization_percentage: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Hardware {
    name: String,
    identifier: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Sensor {
    name: String,
    parent: String,
    identifier: String,
    sensor_type: String,
    value: Option<f32>,
}

// Keep the numeric model and leave the full device name in the adapter picker.
fn compact_adapter_name(full: &str) -> String {
    let name = full
        .trim_start_matches("NVIDIA GeForce ")
        .trim_start_matches("NVIDIA ")
        .trim_start_matches("AMD ")
        .trim_end_matches(" Graphics")
        .replace("(TM)", "")
        .replace("(R)", "");
    if name.chars().count() <= 12 {
        return name;
    }
    let mut family = "";
    for word in name.split_whitespace() {
        if word.bytes().any(|b| b.is_ascii_digit()) {
            return format!("{family} {word}").trim().chars().take(12).collect();
        }
        family = word;
    }
    name.split_whitespace()
        .next()
        .unwrap_or("GPU")
        .chars()
        .take(12)
        .collect()
}

#[cfg(test)]
mod adapter_label_tests {
    use super::*;
    fn adapter(id: &str, load: Option<f32>) -> GpuInfo {
        GpuInfo {
            name: id.into(),
            card: id.into(),
            luid: id.into(),
            capacity: 100,
            nvidia_uuid: None,
            native: NativeGpu {
                load,
                ..Default::default()
            },
        }
    }

    #[test]
    fn native_zero_and_mixed_adapter_fallback_stay_separate() {
        let snapshot = Snapshot {
            gpus: vec![adapter("primary", Some(0.0)), adapter("secondary", None)],
            engines: Some(vec![
                Engine {
                    name: "secondary_3d".into(),
                    utilization_percentage: 17,
                },
                Engine {
                    name: "secondary_3d".into(),
                    utilization_percentage: 18,
                },
                Engine {
                    name: "primary_3d".into(),
                    utilization_percentage: 91,
                },
            ]),
            ..Default::default()
        };
        for (card, expected) in [
            (None, Some(0.0)),
            (Some(""), Some(0.0)),
            (Some("card0"), Some(0.0)),
            (Some("secondary"), Some(35.0)),
            (Some("gone"), None),
        ] {
            let settings = GraphSettings {
                gpu_card: card.map(String::from),
                metric_type: MetricType::GpuLoad,
                ..Default::default()
            };
            assert_eq!(snapshot.gpu_read(&settings), expected);
        }
    }

    #[test]
    fn fallback_vram_units_and_ambiguous_hardware_names() {
        let mut gpu = adapter("gpu", None);
        gpu.capacity = 2 << 30;
        let mut snap = Snapshot {
            gpus: vec![gpu],
            gpu_memory: vec![AdapterMemory {
                name: "GPU_PHYS_0".into(),
                dedicated_usage: 1 << 30,
            }],
            hardware: vec![Hardware {
                name: "GPU".into(),
                identifier: "chip0".into(),
            }],
            sensors: vec![Sensor {
                name: "GPU Board Power".into(),
                parent: "chip0".into(),
                identifier: "chip0/power/0".into(),
                sensor_type: "Power".into(),
                value: Some(42.0),
            }],
            ..Default::default()
        };
        let mut settings = GraphSettings {
            metric_type: MetricType::GpuVram,
            ..Default::default()
        };
        assert_eq!(snap.gpu_read(&settings), Some(50.0));
        settings.vram_display = MemoryDisplay::Gigabytes;
        assert_eq!(snap.gpu_read(&settings), Some(1.0));
        settings.metric_type = MetricType::GpuPower;
        assert_eq!(snap.gpu_read(&settings), Some(42.0));
        snap.hardware.push(Hardware {
            name: "gpu".into(),
            identifier: "chip1".into(),
        });
        assert_eq!(snap.gpu_read(&settings), None);
    }

    #[test]
    fn long_adapter_names_keep_the_model_and_leave_room_for_the_metric() {
        assert_eq!(
            compact_adapter_name("NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition"),
            "PRO 6000"
        );
        assert_eq!(compact_adapter_name("NVIDIA GeForce RTX 5090"), "RTX 5090");
        assert_eq!(compact_adapter_name("AMD Radeon(TM) Graphics"), "Radeon");
    }
}

fn contains_any(value: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| value.contains(pattern))
}

impl Sensor {
    fn matches_board_metric(&self, metric: MetricType) -> bool {
        use MetricType::*;
        let id = self.parent.to_ascii_lowercase();
        let name = self.name.to_ascii_lowercase();
        let board = contains_any(&id, &["lpc/", "superio", "motherboard"]);
        match (metric, self.sensor_type.as_str()) {
            (CpuTemp | CpuPackageTemp, "Temperature") => {
                id.contains("cpu/")
                    && (contains_any(&name, &["package", "tctl"])
                        || (metric == CpuTemp && name.contains("core")))
            }
            (CpuVoltage, "Voltage") => {
                contains_any(&name, &["vcore", "cpu core"])
                    || (id.contains("cpu/") && name.contains("core"))
            }
            (SystemFan, "Fan") => board || name.contains("system"),
            (MotherboardTemp, "Temperature") => board && !name.contains("cpu"),
            (NvmeTemp, "Temperature") => id.contains("nvme"),
            (RamTemp, "Temperature") => {
                contains_any(&id, &["dimm", "memory", "ram/"]) || name.contains("dimm")
            }
            _ => false,
        }
    }
}

#[derive(Default)]
pub struct Snapshot {
    pub acquired_at: u64,
    cpu: Option<f32>,
    cpu_temperature: Option<f32>,
    ram: Option<(u64, u64)>,
    disk: Option<Disk>,
    network: Option<(u64, u64)>,
    gpus: Vec<GpuInfo>,
    engines: Option<Vec<Engine>>,
    gpu_memory: Vec<AdapterMemory>,
    hardware: Vec<Hardware>,
    sensors: Vec<Sensor>,
}

fn rows<T: serde::de::DeserializeOwned>(db: Option<&WMIConnection>, query: &str) -> Option<Vec<T>> {
    db?.raw_query(query)
        .map_err(|e| log::debug!("Counter query failed: {e}"))
        .ok()
}

impl Snapshot {
    pub fn collect(requests: &[Request], sampler: &mut Sampler) -> Self {
        Self::sample(
            requests.iter().map(|r| r.metric),
            sampler,
            |id, primary, metric| {
                requests
                    .iter()
                    .any(|r| r.metric == metric && matches_adapter(r.card.as_deref(), id, primary))
            },
        )
    }

    fn sample(
        metrics: impl Iterator<Item = MetricType> + Clone,
        sampler: &mut Sampler,
        wants_gpu: impl Fn(&str, bool, MetricType) -> bool,
    ) -> Self {
        use MetricType::*;
        let needs = |candidates: &[MetricType]| metrics.clone().any(|m| candidates.contains(&m));
        // Time of acquisition, conservatively including any slower fallback work.
        let acquired_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let groups = Groups {
            cpu: needs(&[CpuLoad]),
            network: needs(&[NetUpload, NetDownload]),
        };
        let rates = sampler.read(groups);
        let mut s = Self {
            acquired_at,
            cpu: rates.cpu,
            network: rates.network,
            ..Self::default()
        };
        let db: std::cell::LazyCell<Option<WMIConnection>> =
            std::cell::LazyCell::new(|| WMIConnection::new().ok());
        s.sample_system(&db, groups, needs);
        let mut needs_board = (s.cpu_temperature.is_none() && needs(&[CpuTemp, CpuPackageTemp]))
            || needs(&[MotherboardTemp, NvmeTemp, SystemFan, CpuVoltage, RamTemp]);
        if needs(&GPU_METRICS) {
            needs_board |= s.sample_adapters(wants_gpu, &db);
        }
        if needs_board {
            s.sample_hardware_monitor();
        }
        s
    }

    fn sample_system(
        &mut self,
        db: &std::cell::LazyCell<Option<WMIConnection>>,
        groups: Groups,
        needs: impl Fn(&[MetricType]) -> bool,
    ) {
        use MetricType::*;
        if needs(&[CpuTemp, CpuPackageTemp]) {
            self.cpu_temperature = crate::cpu_temperature::current();
        }
        if groups.cpu && self.cpu.is_none() {
            self.cpu = rows::<Processor>(db.as_ref(), "SELECT PercentProcessorTime FROM Win32_PerfFormattedData_PerfOS_Processor WHERE Name='_Total'").and_then(|r| r.first().map(|r| r.percent_processor_time as f32));
        }
        if needs(&[RamUsage]) {
            let mut sys = System::new();
            sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
            self.ram = (sys.total_memory() > 0).then(|| (sys.used_memory(), sys.total_memory()));
        }
        if needs(&[DiskRead, DiskWrite]) {
            self.disk = rows::<Disk>(db.as_ref(),"SELECT DiskReadBytesPersec,DiskWriteBytesPersec FROM Win32_PerfFormattedData_PerfDisk_PhysicalDisk WHERE Name='_Total'").and_then(|mut r| r.pop());
        }
        if groups.network && self.network.is_none() {
            self.network = rows::<Network>(db.as_ref(),"SELECT BytesReceivedPersec,BytesSentPersec FROM Win32_PerfFormattedData_Tcpip_NetworkInterface").filter(|r| !r.is_empty()).map(|r| r.iter().fold((0,0), |(rx,tx), n| (rx+n.bytes_received_persec,tx+n.bytes_sent_persec)));
        }
    }

    /// Returns true when a requested temperature or power reading still needs LHM.
    fn sample_adapters(
        &mut self,
        wants_gpu: impl Fn(&str, bool, MetricType) -> bool,
        db: &std::cell::LazyCell<Option<WMIConnection>>,
    ) -> bool {
        use MetricType::*;
        self.gpus = list_gpus();
        let mut missing = Vec::new();
        for (i, gpu) in self.gpus.iter_mut().enumerate() {
            let mut requested = GPU_METRICS
                .into_iter()
                .filter(|&m| wants_gpu(&gpu.card, i == 0, m))
                .peekable();
            if requested.peek().is_none() {
                continue;
            }
            let dev = gpu
                .nvidia_uuid
                .as_deref()
                .and_then(|id| nvml()?.device_by_uuid(id).ok());
            missing.extend(requested.filter(|&m| !gpu.native.fetch(m, dev.as_ref())));
        }
        if missing.contains(&GpuLoad) {
            self.engines = rows(db.as_ref(), "SELECT Name,UtilizationPercentage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine");
            for row in self.engines.iter_mut().flatten() {
                row.name.make_ascii_lowercase();
            }
        }
        if missing.contains(&GpuVram) {
            self.gpu_memory = rows(db.as_ref(), "SELECT Name,DedicatedUsage FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUAdapterMemory").unwrap_or_default();
        }
        missing.contains(&GpuTemp) || missing.contains(&GpuPower)
    }

    fn sample_hardware_monitor(&mut self) {
        let db = WMIConnection::with_namespace_path(r"ROOT\LibreHardwareMonitor").ok();
        self.hardware =
            rows(db.as_ref(), "SELECT Identifier,Name FROM Hardware").unwrap_or_default();
        self.sensors = rows(
            db.as_ref(),
            "SELECT Parent,Identifier,Name,SensorType,Value FROM Sensor",
        )
        .unwrap_or_default();
        self.sensors.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    }

    fn gpu(&self, settings: &GraphSettings) -> Result<&GpuInfo> {
        self.gpus
            .iter()
            .enumerate()
            .find(|(i, g)| matches_adapter(settings.gpu_card.as_deref(), &g.card, *i == 0))
            .map(|(_, g)| g)
            .ok_or_else(|| anyhow!("Selected adapter unavailable"))
    }

    pub fn instrument_title(&self, settings: &GraphSettings) -> Option<String> {
        use MetricType::*;
        let metric = match settings.metric_type {
            GpuLoad => "Load",
            GpuVram => "VRAM",
            GpuPower => "Power",
            GpuTemp => "Temp",
            _ => return None,
        };
        let gpu = self.gpu(settings).ok()?;
        let name = compact_adapter_name(&gpu.name);
        Some(format!("{name} {metric}"))
    }

    pub fn total_gb(&self, settings: &GraphSettings) -> Option<f32> {
        let bytes = match settings.metric_type {
            MetricType::RamUsage => self.ram?.1,
            MetricType::GpuVram => self.gpu(settings).ok()?.capacity,
            _ => return None,
        };
        (bytes > 0).then(|| (bytes as f64 / GIB) as f32)
    }

    /// WMI reports one row per process/engine; combine rows before choosing the busiest engine.
    fn busiest_engine(&self, luid: &str) -> Option<f32> {
        let mut sums = HashMap::new();
        for row in self.engines.as_ref()? {
            if let Some((_, engine)) = row.name.split_once(luid) {
                *sums.entry(engine).or_insert(0u64) += row.utilization_percentage;
            }
        }
        sums.values().max().map(|&v| v.min(100) as f32)
    }

    fn gpu_memory_usage(&self, gpu: &GpuInfo, display: MemoryDisplay) -> Option<f32> {
        let (used, total) = gpu.native.memory.or_else(|| {
            let physical = format!("{}_phys_0", gpu.luid);
            let row = self
                .gpu_memory
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(&physical))?;
            Some((row.dedicated_usage, gpu.capacity))
        })?;
        match display {
            MemoryDisplay::Percentage => {
                (total > 0).then(|| (100.0 * used as f64 / total as f64) as f32)
            }
            MemoryDisplay::Gigabytes => Some((used as f64 / GIB) as f32),
        }
    }

    fn gpu_sensor(&self, gpu: &GpuInfo, sensor_type: &str, names: &[&str]) -> Option<f32> {
        let mut hardware = self
            .hardware
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(&gpu.name));
        let id = &hardware.next()?.identifier;
        if hardware.next().is_some() {
            return None;
        }
        self.sensors
            .iter()
            .filter(|s| {
                &s.parent == id && s.sensor_type == sensor_type && names.contains(&s.name.as_str())
            })
            .find_map(|s| s.value)
    }

    fn gpu_read(&self, settings: &GraphSettings) -> Option<f32> {
        use MetricType::*;
        let gpu = self.gpu(settings).ok()?;
        match settings.metric_type {
            GpuLoad => gpu.native.load.or_else(|| self.busiest_engine(&gpu.luid)),
            GpuVram => self.gpu_memory_usage(gpu, settings.vram_display),
            GpuTemp => gpu
                .native
                .temp
                .or_else(|| self.gpu_sensor(gpu, "Temperature", &["GPU Core"])),
            GpuPower => gpu.native.power.or_else(|| {
                self.gpu_sensor(
                    gpu,
                    "Power",
                    &["GPU Package", "GPU Board Power", "GPU Power", "GPU PPT"],
                )
            }),
            _ => None,
        }
    }

    fn board_read(&self, settings: &GraphSettings) -> Option<f32> {
        use MetricType::*;
        let metric = settings.metric_type;
        let mut found = self
            .sensors
            .iter()
            .filter(|sensor| sensor.matches_board_metric(metric))
            .filter_map(|sensor| sensor.value);
        let index = if metric == SystemFan {
            settings.fan_number.unwrap_or(1).checked_sub(1)? as usize
        } else {
            0
        };
        found.nth(index)
    }

    pub fn read(&self, settings: &GraphSettings) -> Result<f32> {
        use MetricType::*;
        let v = match settings.metric_type {
            CpuLoad => self.cpu,
            CpuTemp | CpuPackageTemp => self.cpu_temperature.or_else(|| self.board_read(settings)),
            RamUsage => self.ram.map(|(used, total)| match settings.ram_display {
                MemoryDisplay::Percentage => (100.0 * used as f64 / total as f64) as f32,
                MemoryDisplay::Gigabytes => (used as f64 / GIB) as f32,
            }),
            DiskRead | DiskWrite => self.disk.as_ref().map(|d| {
                let v = if settings.metric_type == DiskRead {
                    d.disk_read_bytes_persec
                } else {
                    d.disk_write_bytes_persec
                };
                (v as f64 / MIB) as f32
            }),
            NetDownload | NetUpload => self.network.map(|(rx, tx)| {
                (if settings.metric_type == NetDownload {
                    rx
                } else {
                    tx
                } as f64
                    / MIB) as f32
            }),
            GpuTemp | GpuLoad | GpuVram | GpuPower => self.gpu_read(settings),
            _ => self.board_read(settings),
        };
        match v {
            Some(v) if v.is_finite() => Ok(v),
            _ => bail!("Sensor unavailable"),
        }
    }
}
