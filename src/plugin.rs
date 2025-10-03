use anyhow::Result;
use lazy_static::lazy_static;
use openaction::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::graph_data::{DataSource, GraphData, GraphSettings, MetricType, VisualizationType};
use crate::sensors;
use crate::websocket::WebSocketClient;

const UPDATE_INTERVAL_SECS: u64 = 1;

lazy_static! {
    static ref GRAPH_INSTANCES: Arc<Mutex<HashMap<String, GraphData>>> =
        Arc::new(Mutex::new(HashMap::new()));
}

async fn read_sensor_value(
    settings: &GraphSettings,
    ws_client: Option<&Arc<WebSocketClient>>,
) -> Result<f32> {
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
        MetricType::CpuTemp | MetricType::CpuPackageTemp => sensors::find_cpu_temperature().await,
        MetricType::CpuLoad => sensors::find_cpu_load().await,
        MetricType::GpuTemp => sensors::find_gpu_temperature().await,
        MetricType::GpuLoad => sensors::find_gpu_load().await,
        MetricType::MotherboardTemp => sensors::find_motherboard_temperature().await,
        MetricType::NvmeTemp => sensors::find_nvme_temperature().await,
        MetricType::SystemFan => {
            sensors::find_system_fan_speed(settings.fan_number.unwrap_or(1)).await
        }
        MetricType::CpuVoltage => sensors::find_cpu_voltage().await,
        MetricType::DiskWrite => sensors::find_disk_write().await,
        MetricType::DiskRead => sensors::find_disk_read().await,
        MetricType::RamUsage => sensors::find_ram_usage().await,
        MetricType::NetDownload => sensors::find_net_download().await,
        MetricType::NetUpload => sensors::find_net_upload().await,
        MetricType::RamTemp => sensors::find_ram_temperature().await,
    }
}

pub struct GraphAction;

#[async_trait]
impl Action for GraphAction {
    const UUID: ActionUuid = "com.victormarin.graphs.action";
    type Settings = GraphSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        let instance_id = instance.instance_id.clone();
        let mut instances = GRAPH_INSTANCES.lock().await;

        let mut graph_data = GraphData::new(settings.clone());

        if let Err(e) = graph_data.initialize_websocket().await {
            log::error!("Failed to initialize WebSocket: {}", e);
        }

        instances.insert(instance_id, graph_data);

        Ok(())
    }

    async fn will_disappear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
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
            if settings.data_source == DataSource::WebSocket && old_source != DataSource::WebSocket
            {
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
                    let ws_client = graph_data.get_ws_client();

                    if let Ok(value) = read_sensor_value(&graph_data.settings, ws_client).await {
                        graph_data.add_data_point(value);

                        let config = graph_data.get_graph_config();

                        // Prepare title text before dropping instances
                        let title_option = if graph_data.settings.show_value_text {
                            let suffix = match graph_data.settings.data_source {
                                DataSource::LmSensors => {
                                    graph_data.settings.metric_type.value_suffix()
                                }
                                DataSource::WebSocket => "",
                            };
                            Some(format!("{:.1}{}", value, suffix))
                        } else {
                            None
                        };

                        let data_uri_result = match graph_data.settings.visualization_type {
                            VisualizationType::Graph => {
                                crate::gfx::generate_graph_data_uri(&config)
                            }
                            VisualizationType::Gauge => {
                                crate::gfx::generate_gauge_data_uri(&config)
                            }
                        };

                        if let Ok(data_uri) = data_uri_result {
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
