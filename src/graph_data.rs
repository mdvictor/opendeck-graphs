use crate::gfx::{
    BackgroundConfig, ColorScheme, GradientType, GraphConfig, ValuePos, DEFAULT_GAUGE_OUTER_RADIUS,
    DEFAULT_GAUGE_THICKNESS,
};
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
    #[serde(alias = "lmsensors")]
    Local,
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VisualizationType {
    #[default]
    Graph,
    Gauge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MemoryDisplay {
    #[default]
    Percentage,
    Gigabytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BgGradient {
    #[default]
    None,
    Linear,
    Radial,
}

impl From<BgGradient> for GradientType {
    fn from(value: BgGradient) -> Self {
        match value {
            BgGradient::None => GradientType::None,
            BgGradient::Linear => GradientType::Linear,
            BgGradient::Radial => GradientType::Radial,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ValueTextPos {
    #[default]
    Auto,
    Top,
    Center,
    Bottom,
}

impl From<ValueTextPos> for ValuePos {
    fn from(value: ValueTextPos) -> Self {
        match value {
            ValueTextPos::Auto => ValuePos::Auto,
            ValueTextPos::Top => ValuePos::Top,
            ValueTextPos::Center => ValuePos::Center,
            ValueTextPos::Bottom => ValuePos::Bottom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricType {
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
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::GpuVram | MetricType::RamUsage => 100.0,
            MetricType::GpuPower => 500.0,
            MetricType::SystemFan => 3000.0,
            MetricType::CpuVoltage => 2.0,
            MetricType::DiskWrite | MetricType::DiskRead => 500.0, // MB/s
            MetricType::NetDownload | MetricType::NetUpload => 125.0, // MB/s (1 Gbps)
        }
    }

    pub fn default_threshold(&self) -> Option<f32> {
        match self {
            MetricType::CpuTemp | MetricType::CpuPackageTemp => Some(80.0),
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::GpuVram | MetricType::RamUsage => Some(80.0),
            MetricType::GpuPower => Some(300.0),
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
            MetricType::GpuVram => "GPU VRAM",
            MetricType::GpuPower => "GPU Power",
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
            MetricType::CpuLoad | MetricType::GpuLoad | MetricType::GpuVram | MetricType::RamUsage => "%",
            MetricType::GpuPower => "W",
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

    // GPU selection
    pub gpu_card: Option<String>,

    // Memory display mode (percentage vs gigabytes)
    pub vram_display: MemoryDisplay,
    pub ram_display: MemoryDisplay,

    // Background customization
    pub bg_color: Option<String>,
    pub bg_gradient_color: Option<String>,
    pub bg_gradient: BgGradient,
    pub bg_balance: Option<u8>,
    pub bg_softness: Option<u8>,

    // Optional color overrides (empty/None = use line color or default)
    pub title_color: Option<String>,
    pub graph_fill_color: Option<String>,
    pub value_text_color: Option<String>,

    // Sizes (px) and positioning
    pub title_size: Option<f32>,
    pub value_text_size: Option<f32>,
    pub value_text_position: ValueTextPos,

    // Gauge geometry overrides
    pub gauge_outer_radius: Option<f32>,
    pub gauge_thickness: Option<f32>,
}

impl GraphSettings {
    pub fn memory_display(&self) -> MemoryDisplay {
        match self.metric_type {
            MetricType::GpuVram => self.vram_display,
            MetricType::RamUsage => self.ram_display,
            _ => MemoryDisplay::Percentage,
        }
    }

    pub fn effective_suffix(&self) -> &str {
        if matches!(self.metric_type, MetricType::GpuVram | MetricType::RamUsage)
            && self.memory_display() == MemoryDisplay::Gigabytes
        {
            "GB"
        } else {
            self.metric_type.value_suffix()
        }
    }
}

/// Data for a single graph instance
pub struct GraphData {
    data_points: VecDeque<f32>,
    pub settings: GraphSettings,
    ws_client: Option<Arc<WebSocketClient>>,
    /// Cached total memory in GB (auto-detected, used for graph max and display)
    pub vram_total_gb: Option<f32>,
    pub ram_total_gb: Option<f32>,
}

impl GraphData {
    pub fn new(settings: GraphSettings) -> Self {
        Self {
            data_points: VecDeque::with_capacity(MAX_DATA_POINTS),
            settings,
            ws_client: None,
            vram_total_gb: None,
            ram_total_gb: None,
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
            DataSource::Local => {
                // For system fan, show the fan number
                if matches!(self.settings.metric_type, MetricType::SystemFan) {
                    if let Some(fan_num) = self.settings.fan_number {
                        format!("Fan {}", fan_num)
                    } else {
                        "Fan 1".to_string()
                    }
                } else if matches!(self.settings.metric_type, MetricType::GpuVram | MetricType::RamUsage)
                    && self.settings.memory_display() == MemoryDisplay::Gigabytes
                {
                    match self.settings.metric_type {
                        MetricType::GpuVram => "VRAM GB".to_string(),
                        MetricType::RamUsage => "RAM GB".to_string(),
                        _ => unreachable!(),
                    }
                } else {
                    self.settings.metric_type.display_name().to_string()
                }
            }
            DataSource::WebSocket => "WebSocket".to_string(),
        };

        let background = BackgroundConfig {
            color1: self
                .settings
                .bg_color
                .as_deref()
                .and_then(parse_hex_color)
                .unwrap_or(BackgroundConfig::default().color1),
            color2: self
                .settings
                .bg_gradient_color
                .as_deref()
                .and_then(parse_hex_color)
                .unwrap_or(BackgroundConfig::default().color2),
            gradient: self.settings.bg_gradient.into(),
            balance: self.settings.bg_balance.unwrap_or(50),
            softness: self.settings.bg_softness.unwrap_or(50),
        };

        let title_color = self
            .settings
            .title_color
            .as_deref()
            .and_then(parse_hex_color);
        let fill_color = self
            .settings
            .graph_fill_color
            .as_deref()
            .and_then(parse_hex_color);
        let value_text_color = self
            .settings
            .value_text_color
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or(image::Rgba([255, 255, 255, 255]));

        GraphConfig {
            data_points: self.data_points.iter().copied().collect(),
            max_value: self
                .settings
                .max_value
                .unwrap_or_else(|| match self.settings.data_source {
                    DataSource::Local => {
                        let detected_total = match self.settings.metric_type {
                            MetricType::GpuVram if self.settings.memory_display() == MemoryDisplay::Gigabytes => self.vram_total_gb,
                            MetricType::RamUsage if self.settings.memory_display() == MemoryDisplay::Gigabytes => self.ram_total_gb,
                            _ => None,
                        };
                        detected_total.unwrap_or_else(|| self.settings.metric_type.default_max())
                    }
                    DataSource::WebSocket => 100.0,
                }),
            min_value: self.settings.min_value.unwrap_or(0.0),
            threshold: self
                .settings
                .threshold
                .or_else(|| match self.settings.data_source {
                    DataSource::Local => {
                        let default = self.settings.metric_type.default_threshold();
                        // In GB mode, scale the percentage-based threshold to GB
                        match self.settings.metric_type {
                            MetricType::GpuVram if self.settings.memory_display() == MemoryDisplay::Gigabytes => {
                                self.vram_total_gb.and_then(|total| default.map(|d| d / 100.0 * total))
                            }
                            MetricType::RamUsage if self.settings.memory_display() == MemoryDisplay::Gigabytes => {
                                self.ram_total_gb.and_then(|total| default.map(|d| d / 100.0 * total))
                            }
                            _ => default,
                        }
                    }
                    DataSource::WebSocket => None,
                }),
            color_scheme: ColorScheme {
                normal_color,
                warning_color,
            },
            title,
            background,
            title_color,
            fill_color,
            value_text_color,
            value_text: self.formatted_value_text(),
            value_text_position: self.settings.value_text_position.into(),
            value_text_size: self.settings.value_text_size.unwrap_or_else(|| {
                // Preserve historical defaults: 22 for graph (bottom), 26 for gauge (center).
                match self.settings.visualization_type {
                    VisualizationType::Gauge => 26.0,
                    VisualizationType::Graph => 22.0,
                }
            }),
            title_size: self.settings.title_size.unwrap_or(25.0),
            gauge_outer_radius: self
                .settings
                .gauge_outer_radius
                .unwrap_or(DEFAULT_GAUGE_OUTER_RADIUS),
            gauge_thickness: self
                .settings
                .gauge_thickness
                .unwrap_or(DEFAULT_GAUGE_THICKNESS),
        }
    }

    /// Format the current value (last data point) for in-image rendering, if enabled.
    pub fn formatted_value_text(&self) -> Option<String> {
        if !self.settings.show_value_text {
            return None;
        }
        let value = *self.data_points.back()?;
        if self.settings.memory_display() == MemoryDisplay::Gigabytes {
            let total = match self.settings.metric_type {
                MetricType::GpuVram => self.vram_total_gb,
                MetricType::RamUsage => self.ram_total_gb,
                _ => None,
            };
            if let Some(total) = total {
                return Some(format!("{:.1}/{:.0}GB", value, total));
            }
            return Some(format!("{:.1}GB", value));
        }
        let suffix = match self.settings.data_source {
            DataSource::Local => self.settings.effective_suffix(),
            DataSource::WebSocket => "",
        };
        Some(format!("{:.1}{}", value, suffix))
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
