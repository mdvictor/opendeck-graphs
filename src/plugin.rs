#[cfg(not(windows))]
use anyhow::Result;
use lazy_static::lazy_static;
use openaction::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::graph_data::{
    DataSource, GraphData, GraphSettings, MemoryDisplay, MetricType, VisualizationType,
};
use crate::sensors;

const UPDATE_INTERVAL_SECS: u64 = 1;

lazy_static! {
    static ref GRAPH_INSTANCES: Arc<Mutex<HashMap<String, GraphData>>> =
        Arc::new(Mutex::new(HashMap::new()));
}

#[cfg(not(windows))]
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
        MetricType::RamUsage => match settings.ram_display {
            MemoryDisplay::Percentage => sensors::find_ram_usage().await,
            MemoryDisplay::Gigabytes => sensors::find_ram_usage_gb().await,
        },
        MetricType::NetDownload => sensors::find_net_download().await,
        MetricType::NetUpload => sensors::find_net_upload().await,
        MetricType::RamTemp => sensors::find_ram_temperature().await,
    }
}

pub struct GraphAction;

#[async_trait]
impl Action for GraphAction {
    const UUID: ActionUuid = "com.stevengriffin.interstitial.graphs.action";
    type Settings = GraphSettings;

    async fn will_appear(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        let instance_id = instance.instance_id.clone();
        let mut instances = GRAPH_INSTANCES.lock().await;

        let mut graph_data = GraphData::new(settings.clone());
        graph_data.sync_companion(instance.controller == "Encoder");

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

    async fn dial_down(&self, instance: &Instance, _: &GraphSettings) -> OpenActionResult<()> {
        if instance.controller == "Encoder"
            && get_connected_devices()
                .await
                .get(&instance.device_id)
                .is_some_and(|d| d.name == "VSDInside N4 Pro")
        {
            if let Some(g) = GRAPH_INSTANCES.lock().await.get_mut(&instance.instance_id) {
                g.encoder_page = (g.encoder_page + 1) % if g.companion.is_some() { 3 } else { 2 };
            }
        }
        Ok(())
    }

    async fn property_inspector_did_appear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        if let Some(data) = GRAPH_INSTANCES.lock().await.get_mut(&instance.instance_id) {
            data.inspector_open = true;
        }
        let gpus = tokio::task::spawn_blocking(sensors::list_gpus)
            .await
            .unwrap_or_default();
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

    async fn send_to_plugin(
        &self,
        instance: &Instance,
        settings: &Self::Settings,
        payload: &serde_json::Value,
    ) -> OpenActionResult<()> {
        #[cfg(windows)]
        if matches!(
            payload["event"].as_str(),
            Some("getStripAlignment" | "setStripAlignment")
        ) {
            return crate::strip_alignment::respond(instance, payload).await;
        }
        if payload["event"] == "inspectorReady" {
            self.property_inspector_did_appear(instance, settings)
                .await?;
        }
        Ok(())
    }

    async fn property_inspector_did_disappear(
        &self,
        instance: &Instance,
        _settings: &Self::Settings,
    ) -> OpenActionResult<()> {
        if let Some(data) = GRAPH_INSTANCES.lock().await.get_mut(&instance.instance_id) {
            data.inspector_open = false;
        }
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
            let old = &graph_data.settings;
            let reconnect = old.data_source != settings.data_source
                || old.websocket_url != settings.websocket_url
                || old.websocket_api_key != settings.websocket_api_key
                || old.websocket_init_messages != settings.websocket_init_messages;
            if reconnect
                || old.metric_type != settings.metric_type
                || old.gpu_card != settings.gpu_card
                || old.ram_display != settings.ram_display
                || old.vram_display != settings.vram_display
                || old.fan_number != settings.fan_number
            {
                graph_data.clear_history();
                graph_data.vram_total_gb = None;
                graph_data.ram_total_gb = None;
            }
            graph_data.settings = settings.clone();
            graph_data.sync_companion(instance.controller == "Encoder");
            if reconnect {
                if let Err(error) = graph_data.initialize_websocket().await {
                    log::warn!("Telemetry source: {error}");
                }
            }
        }

        Ok(())
    }
}

async fn sample_graph(
    graph_data: &mut GraphData,
    #[cfg(windows)] snapshot: &sensors::Snapshot,
) -> crate::gfx::GraphConfig {
    // Cache memory totals when in GB mode (before borrowing ws_client)
    #[cfg(not(windows))]
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

    #[cfg(windows)]
    if graph_data.settings.memory_display() == MemoryDisplay::Gigabytes {
        let total = snapshot.total_gb(&graph_data.settings);
        match graph_data.settings.metric_type {
            MetricType::RamUsage => graph_data.ram_total_gb = total,
            MetricType::GpuVram => graph_data.vram_total_gb = total,
            _ => {}
        }
    }
    let sample = match graph_data.settings.data_source {
        DataSource::WebSocket => match graph_data.get_ws_client() {
            Some(client) => client.get_value().await,
            None => None,
        },
        DataSource::Local => {
            #[cfg(windows)]
            let sample = snapshot.read(&graph_data.settings);
            #[cfg(not(windows))]
            let sample = read_local_sensor_value(&graph_data.settings).await;
            sample.ok().filter(|v| v.is_finite())
        }
    };
    if let Some(value) = sample {
        graph_data.add_data_point(value);
    } else {
        graph_data.clear_history();
    }

    let config = graph_data.get_graph_config();
    #[cfg(windows)]
    let config = {
        let mut config = config;
        if graph_data.settings.data_source == DataSource::Local {
            if let Some(title) = snapshot.instrument_title(&graph_data.settings) {
                config.title = title;
            }
        }
        config
    };
    config
}

#[cfg(windows)]
async fn collect_visible_sensors(
    sampler: &mut crate::pdh_rates::Sampler,
    timings: &mut TickTimings,
) -> sensors::Snapshot {
    let instances = GRAPH_INSTANCES.lock().await;
    let metrics: Vec<_> = instances
        .values()
        .filter(|g| g.settings.data_source == DataSource::Local)
        .flat_map(|g| {
            std::iter::once(sensors::Request::from(&g.settings)).chain(
                g.companion
                    .as_ref()
                    .map(|c| sensors::Request::from(&c.settings)),
            )
        })
        .collect();
    drop(instances);
    if metrics.is_empty() {
        *sampler = Default::default();
        sensors::Snapshot::default()
    } else {
        let mut owned = std::mem::take(sampler);
        match tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let snapshot = sensors::Snapshot::collect(&metrics, &mut owned);
            (snapshot, owned, start.elapsed())
        })
        .await
        {
            Ok((s, owned, elapsed)) => {
                *sampler = owned;
                timings.provider = elapsed;
                s
            }
            Err(e) => {
                log::error!("Windows sampler: {e}");
                sensors::Snapshot::default()
            }
        }
    }
}

#[derive(Default)]
struct TickTimings {
    slowest: (Option<(u8, u8)>, Duration),
    provider: Duration,
    preparation: Duration,
    render: Duration,
    send: Duration,
}

impl TickTimings {
    fn record_send(&mut self, instance: &Instance, elapsed: Duration) {
        if elapsed > self.slowest.1 {
            self.slowest = (instance.coordinates.map(|p| (p.column, p.row)), elapsed);
        }
    }
    fn report_if_slow(
        &self,
        due: tokio::time::Instant,
        wake_late: Duration,
        house: Duration,
        sampling: Duration,
        last_warning: &mut Option<std::time::Instant>,
    ) {
        if due.elapsed() >= Duration::from_secs(2)
            && last_warning.is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
        {
            log::warn!(
                concat!(
                    "Slow graph tick: due_to_done_ms={} wake_late_ms={} housekeeping_ms={} ",
                    "sampler_wait_ms={} providers_ms={} prepare_ms={} render_ms={} ",
                    "send_ms={} slowest_send_at={:?} slowest_send_ms={}"
                ),
                due.elapsed().as_millis(),
                wake_late.as_millis(),
                house.as_millis(),
                sampling.saturating_sub(self.provider).as_millis(),
                self.provider.as_millis(),
                self.preparation.as_millis(),
                self.render.as_millis(),
                self.send.as_millis(),
                self.slowest.0,
                self.slowest.1.as_millis()
            );
            *last_warning = Some(std::time::Instant::now());
        }
    }
}

struct Measure<'a>(&'a mut Duration, std::time::Instant);
impl<'a> Measure<'a> {
    fn start(total: &'a mut Duration) -> Self {
        Self(total, std::time::Instant::now())
    }
}
impl Drop for Measure<'_> {
    fn drop(&mut self) {
        *self.0 += self.1.elapsed();
    }
}

fn is_n4_encoder(instance: &Instance) -> bool {
    instance.controller == "Encoder" && instance.device_id.starts_with("n4-")
}

fn render_strip_page(
    page: u8,
    config: &crate::gfx::GraphConfig,
    gauge: bool,
    companion: Option<&crate::gfx::GraphConfig>,
    slot: Option<u8>,
) -> anyhow::Result<String> {
    let wallpaper = slot.filter(|_| config.background.glass);
    let (metric, gauge, secondary) = match (page, companion) {
        (1, Some(_)) => (config, false, None),
        (2, Some(other)) => (other, false, None),
        (1, None) => (config, !gauge, None),
        _ => (config, gauge, companion),
    };
    crate::gfx::strip_image_for_slot(metric, gauge, secondary, wallpaper)
}

async fn update_display(
    instance: &Instance,
    n4_slot: Option<u8>,
    #[cfg(windows)] snapshot: &sensors::Snapshot,
    timings: &mut TickTimings,
) -> anyhow::Result<()> {
    let prep = Measure::start(&mut timings.preparation);
    let mut instances = GRAPH_INSTANCES.lock().await;
    let Some(graph_data) = instances.get_mut(&instance.instance_id) else {
        return Ok(());
    };
    let config = sample_graph(
        graph_data,
        #[cfg(windows)]
        snapshot,
    )
    .await;
    let companion = if let Some(other) = graph_data.companion.as_mut() {
        Some(
            sample_graph(
                other,
                #[cfg(windows)]
                snapshot,
            )
            .await,
        )
    } else {
        None
    };
    let inspector = graph_data.inspector_open;
    let gauge = graph_data.settings.visualization_type == VisualizationType::Gauge;
    let encoder = instance.controller == "Encoder";

    let page = graph_data.encoder_page;
    #[cfg(windows)]
    let reading = n4_slot
        .and_then(|_| crate::ring_sample::for_graph(graph_data, &config, companion.as_ref()));
    drop(instances);
    drop(prep);
    let render = Measure::start(&mut timings.render);
    let uri = if encoder {
        #[cfg(windows)]
        if let Some(n) = n4_slot {
            crate::ring_sample::publish(&instance.device_id, n, reading, snapshot.acquired_at);
        }
        render_strip_page(page, &config, gauge, companion.as_ref(), n4_slot)
    } else if config.data_points.is_empty() {
        crate::gfx::missing_image(&config)
    } else if gauge {
        crate::gfx::generate_gauge_data_uri(&config)
    } else {
        crate::gfx::generate_graph_data_uri(&config)
    }?;
    drop(render);
    let _send = Measure::start(&mut timings.send);
    send_frame(instance, uri, encoder, inspector).await
}

async fn send_frame(
    instance: &Instance,
    uri: String,
    encoder: bool,
    inspector: bool,
) -> anyhow::Result<()> {
    if inspector {
        let _ = instance
            .send_to_property_inspector(
                serde_json::json!({"event":"frame","image":uri,"encoder":encoder}),
            )
            .await;
    }
    if encoder {
        #[derive(serde::Serialize)]
        struct Feedback {
            plot: String,
        }
        instance.set_feedback(&Feedback { plot: uri }).await?;
    } else {
        instance.set_image(Some(uri), None).await?;
    }
    Ok(())
}

pub async fn start_sensor_monitoring() {
    tokio::spawn(async {
        let mut interval = interval(Duration::from_secs(UPDATE_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        #[cfg(windows)]
        let mut sampler = crate::pdh_rates::Sampler::default();
        let mut last_warning: Option<std::time::Instant> = None;
        loop {
            let due = interval.tick().await;
            let wake_late = due.elapsed();
            let mut timings = TickTimings::default();
            let house = std::time::Instant::now();
            #[cfg(windows)]
            crate::launcher::refresh_theme().await;
            let visible = visible_instances(GraphAction::UUID).await;
            if visible.is_empty() {
                #[cfg(windows)]
                {
                    sampler = Default::default();
                }
                continue;
            }
            let house = house.elapsed();
            let sampling = std::time::Instant::now();
            #[cfg(windows)]
            let snapshot = collect_visible_sensors(&mut sampler, &mut timings).await;
            let sampling = sampling.elapsed();
            let lookup = Measure::start(&mut timings.render);
            let devices = if visible.iter().any(|i| is_n4_encoder(i)) {
                get_connected_devices().await
            } else {
                HashMap::new()
            };
            drop(lookup);
            for instance in visible {
                let slot = instance
                    .coordinates
                    .filter(|_| {
                        is_n4_encoder(&instance)
                            && devices
                                .get(&instance.device_id)
                                .is_some_and(|d| d.name == "VSDInside N4 Pro")
                    })
                    .map(|pos| pos.column);
                let before = timings.send;
                if let Err(error) = update_display(
                    &instance,
                    slot,
                    #[cfg(windows)]
                    &snapshot,
                    &mut timings,
                )
                .await
                {
                    log::warn!("Display update failed: {error}");
                }
                timings.record_send(&instance, timings.send - before);
            }
            timings.report_if_slow(due, wake_late, house, sampling, &mut last_warning);
        }
    });
}

pub async fn init() -> OpenActionResult<()> {
    log::info!("Initializing Graphs plugin");

    start_sensor_monitoring().await;
    register_action(GraphAction).await;
    #[cfg(windows)]
    register_action(crate::launcher::Launcher).await;

    run(std::env::args().collect()).await
}
