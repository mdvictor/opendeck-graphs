use anyhow::Result;
use lazy_static::lazy_static;
use openaction::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::graph_data::{DataSource, GraphData, GraphSettings, MemoryDisplay, MetricType, VisualizationType};
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
        DataSource::Local => read_local_sensor_value(settings).await,
        DataSource::WebSocket => {
            if let Some(client) = ws_client {
                Ok(client.get_value().await)
            } else {
                Ok(0.0)
            }
        }
    }
}

async fn read_local_sensor_value(settings: &GraphSettings) -> Result<f32> {
    match settings.metric_type {
        MetricType::CpuTemp | MetricType::CpuPackageTemp => sensors::find_cpu_temperature().await,
        MetricType::CpuLoad => sensors::find_cpu_load().await,
        MetricType::GpuTemp => {
            let card = settings.gpu_card.as_deref().unwrap_or("card0");
            sensors::find_gpu_temperature(card).await
        }
        MetricType::GpuLoad => {
            let card = settings.gpu_card.as_deref().unwrap_or("card0");
            sensors::find_gpu_load(card).await
        }
        MetricType::GpuVram => {
            let card = settings.gpu_card.as_deref().unwrap_or("card0");
            match settings.vram_display {
                MemoryDisplay::Percentage => sensors::find_gpu_vram(card).await,
                MemoryDisplay::Gigabytes => sensors::find_gpu_vram_gb(card).await,
            }
        }
        MetricType::GpuPower => {
            let card = settings.gpu_card.as_deref().unwrap_or("card0");
            sensors::find_gpu_power(card).await
        }
        MetricType::MotherboardTemp => sensors::find_motherboard_temperature().await,
        MetricType::NvmeTemp => sensors::find_nvme_temperature().await,
        MetricType::SystemFan => {
            sensors::find_system_fan_speed(settings.fan_number.unwrap_or(1)).await
        }
        MetricType::CpuVoltage => sensors::find_cpu_voltage().await,
        MetricType::DiskWrite => sensors::find_disk_write().await,
        MetricType::DiskRead => sensors::find_disk_read().await,
        MetricType::RamUsage => {
            match settings.ram_display {
                MemoryDisplay::Percentage => sensors::find_ram_usage().await,
                MemoryDisplay::Gigabytes => sensors::find_ram_usage_gb().await,
            }
        }
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

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        let gpus = sensors::list_gpus();
        let gpu_list: Vec<serde_json::Value> = gpus
            .iter()
            .map(|g| {
                serde_json::json!({
                    "card": g.card,
                    "name": g.name,
                })
            })
            .collect();
        let _ = instance
            .send_to_property_inspector(serde_json::json!({
                "event": "gpuList",
                "gpus": gpu_list,
            }))
            .await;
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
            let old_gpu_card = graph_data.settings.gpu_card.clone();
            graph_data.settings = settings.clone();

            // Invalidate cached memory totals when the monitored GPU changes
            if settings.gpu_card != old_gpu_card {
                graph_data.vram_total_gb = None;
            }

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
                    // Cache memory totals when in GB mode (before borrowing ws_client)
                    if graph_data.settings.memory_display() == MemoryDisplay::Gigabytes {
                        match graph_data.settings.metric_type {
                            MetricType::GpuVram if graph_data.vram_total_gb.is_none() => {
                                let card = graph_data.settings.gpu_card.as_deref().unwrap_or("card0");
                                if let Ok(total) = sensors::find_gpu_vram_total_gb(card).await {
                                    if total > 0.0 {
                                        graph_data.vram_total_gb = Some(total);
                                    }
                                }
                            }
                            MetricType::RamUsage if graph_data.ram_total_gb.is_none() => {
                                if let Ok(total) = sensors::find_ram_total_gb().await {
                                    if total > 0.0 {
                                        graph_data.ram_total_gb = Some(total);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    let ws_client = graph_data.get_ws_client();

                    if let Ok(value) = read_sensor_value(&graph_data.settings, ws_client).await {
                        graph_data.add_data_point(value);

                        let config = graph_data.get_graph_config();

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
                            // Value text is drawn into the image (config.value_text); clear any
                            // previously-set OpenDeck title overlay so we don't double-render.
                            let _ = instance.set_title(None::<String>, None).await;
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
