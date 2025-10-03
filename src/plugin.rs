use anyhow::Result;
use lazy_static::lazy_static;
use openaction::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::gfx::{ColorScheme, GraphConfig};
use crate::sensors;
use crate::websocket::{WebSocketClient, WebSocketConfig};

const MAX_DATA_POINTS: usize = 10;
const UPDATE_INTERVAL_SECS: u64 = 1;

/// Data source type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DataSource {
    #[serde(rename = "lmsensors")]
    LmSensors,
    #[serde(rename = "websocket")]
    WebSocket,
}

impl Default for DataSource {
    fn default() -> Self {
        DataSource::LmSensors
    }
}

/// Sensor metric type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MetricType {
    CpuTemp,
    CpuPackageTemp,
    CpuLoad,
    GpuTemp,
    GpuLoad,
    MotherboardTemp,
    NvmeTemp,
    CpuFan,
    SystemFan,
    CpuVoltage,
    DiskWrite,
    DiskRead,
    RamUsage,
    NetDownload,
    NetUpload,
}

impl Default for MetricType {
    fn default() -> Self {
        MetricType::CpuTemp
    }
}

impl MetricType {
    pub fn default_max(&self) -> f32 {
        match self {
            MetricType::CpuTemp
            | MetricType::CpuPackageTemp
            | MetricType::GpuTemp
            | MetricType::MotherboardTemp
            | MetricType::NvmeTemp => 120.0,
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::RamUsage => 100.0,
            MetricType::CpuFan | MetricType::SystemFan => 3000.0,
            MetricType::CpuVoltage => 2.0,
            MetricType::DiskWrite | MetricType::DiskRead => 500.0, // MB/s
            MetricType::NetDownload | MetricType::NetUpload => 125.0, // MB/s (1 Gbps)
        }
    }

    pub fn default_threshold(&self) -> Option<f32> {
        match self {
            MetricType::CpuTemp | MetricType::CpuPackageTemp => Some(80.0),
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::RamUsage => Some(80.0),
            MetricType::GpuTemp => Some(85.0),
            MetricType::MotherboardTemp => Some(60.0),
            MetricType::NvmeTemp => Some(70.0),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            MetricType::CpuTemp => "CPU Temp",
            MetricType::CpuPackageTemp => "CPU Package",
            MetricType::CpuLoad => "CPU Load",
            MetricType::GpuTemp => "GPU Temp",
            MetricType::GpuLoad => "GPU Load",
            MetricType::MotherboardTemp => "Motherboard",
            MetricType::NvmeTemp => "NVMe Temp",
            MetricType::CpuFan => "CPU Fan",
            MetricType::SystemFan => "System Fan",
            MetricType::CpuVoltage => "CPU Voltage",
            MetricType::DiskWrite => "Disk Write",
            MetricType::DiskRead => "Disk Read",
            MetricType::RamUsage => "RAM Usage",
            MetricType::NetDownload => "Net Down",
            MetricType::NetUpload => "Net Up",
        }
    }

    pub fn value_suffix(&self) -> &str {
        match self {
            MetricType::CpuTemp
            | MetricType::CpuPackageTemp
            | MetricType::GpuTemp
            | MetricType::MotherboardTemp
            | MetricType::NvmeTemp => "°C",
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::RamUsage => "%",
            MetricType::CpuFan | MetricType::SystemFan => " RPM",
            MetricType::CpuVoltage => "V",
            MetricType::DiskWrite | MetricType::DiskRead => " MB/s",
            MetricType::NetDownload | MetricType::NetUpload => " MB/s",
        }
    }
}

/// Settings for the graph action
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct GraphSettings {
    // Data source
    pub data_source: DataSource,

    // LM Sensors settings
    pub metric_type: MetricType,

    // WebSocket settings
    pub websocket_url: Option<String>,
    pub websocket_api_key: Option<String>,
    pub websocket_init_messages: Vec<String>,
    pub websocket_send_pings: bool,

    // Display settings
    pub show_value_text: bool,
    pub threshold: Option<f32>,
    pub normal_color: String,
    pub warning_color: String,
    pub max_value: Option<f32>,
    pub min_value: Option<f32>,
}

/// Data for a single graph instance
struct GraphData {
    data_points: VecDeque<f32>,
    settings: GraphSettings,
    ws_client: Option<Arc<WebSocketClient>>,
}

impl GraphData {
    fn new(settings: GraphSettings) -> Self {
        Self {
            data_points: VecDeque::with_capacity(MAX_DATA_POINTS),
            settings,
            ws_client: None,
        }
    }

    fn add_data_point(&mut self, value: f32) {
        if self.data_points.len() >= MAX_DATA_POINTS {
            self.data_points.pop_front();
        }
        self.data_points.push_back(value);
    }

    fn get_graph_config(&self) -> GraphConfig {

        let normal_color = parse_hex_color(&self.settings.normal_color)
            .unwrap_or(ColorScheme::default().normal_color);
        let warning_color = parse_hex_color(&self.settings.warning_color)
            .unwrap_or(ColorScheme::default().warning_color);

        // Title is always the metric name only (displayed on the graph image)
        let title = match self.settings.data_source {
            DataSource::LmSensors => self.settings.metric_type.display_name().to_string(),
            DataSource::WebSocket => "WebSocket".to_string(),
        };

        GraphConfig {
            data_points: self.data_points.iter().copied().collect(),
            max_value: self.settings.max_value.unwrap_or_else(|| {
                match self.settings.data_source {
                    DataSource::LmSensors => self.settings.metric_type.default_max(),
                    DataSource::WebSocket => 100.0,
                }
            }),
            min_value: self.settings.min_value.unwrap_or(0.0),
            threshold: self.settings.threshold.or_else(|| {
                match self.settings.data_source {
                    DataSource::LmSensors => self.settings.metric_type.default_threshold(),
                    DataSource::WebSocket => None,
                }
            }),
            color_scheme: ColorScheme {
                normal_color,
                warning_color,
            },
            title,
        }
    }

    async fn initialize_websocket(&mut self) -> Result<()> {
        if self.settings.data_source == DataSource::WebSocket {
            if let Some(url) = &self.settings.websocket_url {
                let config = WebSocketConfig {
                    url: url.clone(),
                    api_key: self.settings.websocket_api_key.clone(),
                    init_messages: self.settings.websocket_init_messages.clone(),
                    send_pings: self.settings.websocket_send_pings,
                };

                let client = Arc::new(WebSocketClient::new(config));
                client.start().await?;
                self.ws_client = Some(client);
            }
        }
        Ok(())
    }
}

lazy_static! {
    static ref GRAPH_INSTANCES: Arc<Mutex<HashMap<String, GraphData>>> =
        Arc::new(Mutex::new(HashMap::new()));
}

fn parse_hex_color(hex: &str) -> Option<image::Rgba<u8>> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(image::Rgba([r, g, b, 255]))
}

async fn read_sensor_value(settings: &GraphSettings, ws_client: Option<&Arc<WebSocketClient>>) -> Result<f32> {
    match settings.data_source {
        DataSource::LmSensors => read_lm_sensors_value(settings).await,
        DataSource::WebSocket => {
            if let Some(client) = ws_client {
                Ok(client.get_value().await)
            } else {
                Ok(0.0)
            }
        }
    }
}

async fn read_lm_sensors_value(settings: &GraphSettings) -> Result<f32> {
    match settings.metric_type {
        MetricType::CpuTemp | MetricType::CpuPackageTemp => {
            sensors::find_cpu_temperature(None, None).await
        }
        MetricType::CpuLoad => sensors::find_cpu_load().await,
        MetricType::GpuTemp => sensors::find_gpu_temperature(None, None).await,
        MetricType::GpuLoad => sensors::find_gpu_load(None, None).await,
        MetricType::MotherboardTemp => {
            sensors::find_motherboard_temperature(None, None).await
        }
        MetricType::NvmeTemp => sensors::find_nvme_temperature(None, None).await,
        MetricType::CpuFan => sensors::find_cpu_fan_speed(None, None).await,
        MetricType::SystemFan => sensors::find_system_fan_speed(None, None).await,
        MetricType::CpuVoltage => sensors::find_cpu_voltage(None, None).await,
        MetricType::DiskWrite => sensors::find_disk_write().await,
        MetricType::DiskRead => sensors::find_disk_read().await,
        MetricType::RamUsage => sensors::find_ram_usage().await,
        MetricType::NetDownload => sensors::find_net_download().await,
        MetricType::NetUpload => sensors::find_net_upload().await,
    }
}

pub struct GraphAction;

#[async_trait]
impl Action for GraphAction {
    const UUID: ActionUuid = "com.victormarin.graphs.action";
    type Settings = GraphSettings;

    async fn will_appear(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
        let instance_id = instance.instance_id.clone();
        let mut instances = GRAPH_INSTANCES.lock().await;

        let mut graph_data = GraphData::new(settings.clone());

        if let Err(e) = graph_data.initialize_websocket().await {
            log::error!("Failed to initialize WebSocket: {}", e);
        }

        instances.insert(instance_id, graph_data);

        Ok(())
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &Self::Settings) -> OpenActionResult<()> {
        let instance_id = instance.instance_id.clone();
        let mut instances = GRAPH_INSTANCES.lock().await;
        instances.remove(&instance_id);

        Ok(())
    }

    async fn did_receive_settings(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        let instance_id = instance.instance_id.clone();
        let mut instances = GRAPH_INSTANCES.lock().await;

        if let Some(graph_data) = instances.get_mut(&instance_id) {
            let old_source = graph_data.settings.data_source.clone();
            graph_data.settings = settings.clone();

            // Reinitialize WebSocket if source changed to WebSocket
            if settings.data_source == DataSource::WebSocket && old_source != DataSource::WebSocket {
                if let Err(e) = graph_data.initialize_websocket().await {
                    log::error!("Failed to initialize WebSocket: {}", e);
                }
            }
        }

        Ok(())
    }
}

pub async fn start_sensor_monitoring() {
    tokio::spawn(async {
        let mut interval = interval(Duration::from_secs(UPDATE_INTERVAL_SECS));

        loop {
            interval.tick().await;

            let visible = visible_instances(GraphAction::UUID).await;

            for instance in visible {
                let instance_id = instance.instance_id.clone();

                let mut instances = GRAPH_INSTANCES.lock().await;

                if let Some(graph_data) = instances.get_mut(&instance_id) {
                    let ws_client = graph_data.ws_client.as_ref();

                    if let Ok(value) = read_sensor_value(&graph_data.settings, ws_client).await {
                        graph_data.add_data_point(value);

                        let config = graph_data.get_graph_config();

                        // Prepare title text before dropping instances
                        let title_option = if graph_data.settings.show_value_text {
                            let suffix = match graph_data.settings.data_source {
                                DataSource::LmSensors => graph_data.settings.metric_type.value_suffix(),
                                DataSource::WebSocket => "",
                            };
                            Some(format!("{:.1}{}", value, suffix))
                        } else {
                            None
                        };

                        if let Ok(data_uri) = crate::gfx::generate_graph_data_uri(&config) {
                            drop(instances);
                            let _ = instance.set_image(Some(data_uri), None).await;

                            // Set title with current value if enabled
                            if let Some(title_text) = title_option {
                                let _ = instance.set_title(Some(title_text), None).await;
                            } else {
                                let _ = instance.set_title(None::<String>, None).await;
                            }
                        }
                    }
                }
            }
        }
    });
}

pub async fn init() -> OpenActionResult<()> {
    log::info!("Initializing Graphs plugin");

    start_sensor_monitoring().await;
    register_action(GraphAction).await;

    run(std::env::args().collect()).await
}
