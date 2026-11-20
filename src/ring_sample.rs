//! N4 owns a session-local, volatile mapping. Each slot is a single aligned word:
//! high 32 = Unix seconds, next 8 = metric family, low 16 = tenths of a degree Celsius or tenths of a percent.
use crate::{gfx::GraphConfig, graph_data::MetricType};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::CloseHandle,
        System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_WRITE},
    },
};

fn value(metric: MetricType, config: &GraphConfig) -> Option<(u8, f32)> {
    use MetricType::*;
    let v = *config.data_points.last()?;
    let pair = match metric {
        CpuTemp | CpuPackageTemp | GpuTemp | RamTemp | NvmeTemp | MotherboardTemp => (1, v),
        RamUsage if config.max_value > 0.0 => (2, 100.0 * v / config.max_value),
        NetUpload | NetDownload if config.max_value > 0.0 => (3, 100.0 * v / config.max_value),
        _ => return None,
    };
    pair.1.is_finite().then_some(pair)
}

pub fn for_graph(
    graph: &crate::graph_data::GraphData,
    config: &GraphConfig,
    companion: Option<&GraphConfig>,
) -> Option<(u8, f32)> {
    if graph.settings.data_source != crate::graph_data::DataSource::Local {
        return None;
    }
    let primary = value(graph.settings.metric_type, config);
    let other = graph
        .companion
        .as_ref()
        .zip(companion)
        .and_then(|(g, c)| value(g.settings.metric_type, c));
    match (primary, other) {
        (Some((kind @ (1 | 3), a)), Some((other_kind, b))) if kind == other_kind => {
            Some((kind, a.max(b)))
        }
        (_, Some(h @ (1, _))) => Some(h),
        (a, b) => a.or(b),
    }
}

pub fn publish(id: &str, slot: u8, value: Option<(u8, f32)>, acquired_at: u64) {
    if slot >= 4 || id.contains(['\\', '\0']) {
        return;
    }
    let name = HSTRING::from(format!(r"Local\Interstitial.Rings.v1.{id}"));
    let sample = value.map_or(0, |(kind, v)| {
        acquired_at << 32
            | u64::from(kind) << 24
            | u64::from((v.clamp(0., 150.) * 10.).round() as u16)
    });
    unsafe {
        let Ok(h) = OpenFileMappingW(FILE_MAP_WRITE.0, false, &name) else {
            return;
        };
        let view = MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, 32);
        let _ = CloseHandle(h);
        if !view.Value.is_null() {
            (&*view.Value.cast::<AtomicU64>().add(usize::from(slot)))
                .store(sample, Ordering::Release);
            let _ = UnmapViewOfFile(view);
        }
    }
}
