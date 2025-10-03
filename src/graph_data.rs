use crate::gfx::{ColorScheme, GraphConfig};
use crate::websocket::{WebSocketClient, WebSocketConfig};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

const MAX_DATA_POINTS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DataSource {
    #[default]
    LmSensors,
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VisualizationType {
    #[default]
    Graph,
    Gauge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricType {
    CpuTemp,
    CpuPackageTemp,
    CpuLoad,
    GpuTemp,
    GpuLoad,
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
            | MetricType::NvmeTemp
            | MetricType::RamTemp => 120.0,
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::RamUsage => 100.0,
            MetricType::SystemFan => 3000.0,
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
            MetricType::RamTemp => Some(85.0),
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
            MetricType::SystemFan => "System Fan",
            MetricType::CpuVoltage => "CPU Voltage",
            MetricType::DiskWrite => "Disk Write",
            MetricType::DiskRead => "Disk Read",
            MetricType::RamUsage => "RAM Usage",
            MetricType::RamTemp => "RAM Temp",
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
            | MetricType::NvmeTemp
            | MetricType::RamTemp => "°C",
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::RamUsage => "%",
            MetricType::SystemFan => " RPM",
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

    // Display settings
    pub visualization_type: VisualizationType,
    pub show_value_text: bool,
    pub threshold: Option<f32>,
    pub normal_color: String,
    pub warning_color: String,
    pub max_value: Option<f32>,
    pub min_value: Option<f32>,

    // Fan settings
    pub fan_number: Option<u32>,
}

/// Data for a single graph instance
pub struct GraphData {
    data_points: VecDeque<f32>,
    pub settings: GraphSettings,
    ws_client: Option<Arc<WebSocketClient>>,
}

impl GraphData {
    pub fn new(settings: GraphSettings) -> Self {
        Self {
            data_points: VecDeque::with_capacity(MAX_DATA_POINTS),
            settings,
            ws_client: None,
        }
    }

    pub fn add_data_point(&mut self, value: f32) {
        if self.data_points.len() >= MAX_DATA_POINTS {
            self.data_points.pop_front();
        }
        self.data_points.push_back(value);
    }

    pub fn get_graph_config(&self) -> GraphConfig {
        let normal_color = parse_hex_color(&self.settings.normal_color)
            .unwrap_or(ColorScheme::default().normal_color);
        let warning_color = parse_hex_color(&self.settings.warning_color)
            .unwrap_or(ColorScheme::default().warning_color);

        // Title is always the metric name only (displayed on the graph image)
        let title = match self.settings.data_source {
            DataSource::LmSensors => {
                // For system fan, show the fan number
                if matches!(self.settings.metric_type, MetricType::SystemFan) {
                    if let Some(fan_num) = self.settings.fan_number {
                        format!("Fan {}", fan_num)
                    } else {
                        "Fan 1".to_string()
                    }
                } else {
                    self.settings.metric_type.display_name().to_string()
                }
            }
            DataSource::WebSocket => "WebSocket".to_string(),
        };

        GraphConfig {
            data_points: self.data_points.iter().copied().collect(),
            max_value: self
                .settings
                .max_value
                .unwrap_or_else(|| match self.settings.data_source {
                    DataSource::LmSensors => self.settings.metric_type.default_max(),
                    DataSource::WebSocket => 100.0,
                }),
            min_value: self.settings.min_value.unwrap_or(0.0),
            threshold: self
                .settings
                .threshold
                .or_else(|| match self.settings.data_source {
                    DataSource::LmSensors => self.settings.metric_type.default_threshold(),
                    DataSource::WebSocket => None,
                }),
            color_scheme: ColorScheme {
                normal_color,
                warning_color,
            },
            title,
        }
    }

    pub async fn initialize_websocket(&mut self) -> Result<()> {
        if self.settings.data_source == DataSource::WebSocket {
            if let Some(url) = &self.settings.websocket_url {
                let config = WebSocketConfig {
                    url: url.clone(),
                    api_key: self.settings.websocket_api_key.clone(),
                    init_messages: self.settings.websocket_init_messages.clone(),
                };

                let client = Arc::new(WebSocketClient::new(config));
                client.start().await?;
                self.ws_client = Some(client);
            }
        }

        Ok(())
    }

    pub fn get_ws_client(&self) -> Option<&Arc<WebSocketClient>> {
        self.ws_client.as_ref()
    }
}

/// Parse hex color string to RGBA
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
