//! Windows Start-menu launchers. Only catalogued app IDs can be launched.
use image::{imageops, Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};
use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU8, Ordering},
        LazyLock,
    },
};
use tokio::sync::Mutex;

pub struct Launcher;
#[derive(Default, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct Settings {
    app_id: String,
    show_label: bool,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    label: String,
    slot: usize,
}
struct App {
    meta: Entry,
    icon: RgbaImage,
}
static APPS: LazyLock<Vec<App>> = LazyLock::new(|| {
    let load = || -> Option<Vec<App>> {
        let dir = dirs::config_dir()?.join("Interstitial/launchers");
        let entries: Vec<Entry> =
            serde_json::from_slice(&std::fs::read(dir.join("catalog.json")).ok()?).ok()?;
        Some(
            entries
                .into_iter()
                .filter_map(|meta| {
                    let icon = image::open(dir.join(format!("icon-{}.png", meta.slot)))
                        .ok()?
                        .into_rgba8();
                    Some(App { meta, icon })
                })
                .collect(),
        )
    };
    load().unwrap_or_default()
});
static VISIBLE: LazyLock<Mutex<HashMap<String, Settings>>> = LazyLock::new(Default::default);
static LAST_THEME: AtomicU8 = AtomicU8::new(2);

fn render(app: &App, label: bool, col: u8, row: u8, light: bool) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(
        144,
        144,
        if light {
            Rgba([237, 242, 247, 255])
        } else {
            Rgba([19, 28, 38, 255])
        },
    );
    if let Some(paper) = crate::gfx::wallpaper::artwork(light) {
        let cols = [(35, 114), (187, 115), (339, 114), (490, 115), (643, 113)];
        if let (Some(&(x, w)), Some(&(y, h))) = (
            cols.get(col as usize),
            [(6, 114), (159, 113)].get(row as usize),
        ) {
            img = imageops::resize(
                &imageops::crop_imm(paper, x, y, w, h).to_image(),
                144,
                144,
                imageops::FilterType::Triangle,
            );
        }
    }
    let (size, y) = if label { (72, 14) } else { (112, 16) };
    let icon = imageops::resize(&app.icon, size, size, imageops::FilterType::Lanczos3);
    imageops::overlay(&mut img, &icon, ((144 - size) / 2) as i64, y);
    if label {
        let font = crate::gfx::typeface::face();
        let width = text_size(23.0, font, &app.meta.label).0;
        let scale = 23.0 * (132.0 / width.max(1) as f32).min(1.0);
        let x = (144 - text_size(scale, font, &app.meta.label).0 as i32) / 2;
        let (ink, shadow) = if light {
            (Rgba([19, 28, 38, 255]), Rgba([255, 255, 255, 255]))
        } else {
            (Rgba([244, 247, 250, 255]), Rgba([9, 13, 18, 255]))
        };
        draw_text_mut(&mut img, shadow, x, 104, scale, font, &app.meta.label);
        draw_text_mut(&mut img, ink, x, 103, scale, font, &app.meta.label);
    }
    img
}
async fn paint(i: &Instance, s: &Settings) -> OpenActionResult<()> {
    let Some(app) = APPS.iter().find(|a| a.meta.id == s.app_id) else {
        return i.set_image(None::<String>, None).await;
    };
    let Some(pos) = i.coordinates else {
        return Ok(());
    };
    match crate::gfx::image_to_data_uri(&render(
        app,
        s.show_label,
        pos.column,
        pos.row,
        crate::gfx::wallpaper::light(),
    )) {
        Ok(uri) => i.set_image(Some(uri), None).await,
        Err(e) => {
            log::error!("Launcher image: {e}");
            Ok(())
        }
    }
}
async fn sync(i: &Instance, s: &Settings) -> OpenActionResult<()> {
    VISIBLE
        .lock()
        .await
        .insert(i.instance_id.clone(), s.clone());
    paint(i, s).await
}
async fn inspector(i: &Instance, s: &Settings) -> OpenActionResult<()> {
    let list: Vec<_> = APPS
        .iter()
        .map(|a| json!({"id":a.meta.id,"label":a.meta.label}))
        .collect();
    i.send_to_property_inspector(json!({"event":"launcherState","apps":list,"settings":s}))
        .await
}
pub async fn refresh_theme() {
    let light = crate::gfx::wallpaper::light() as u8;
    if LAST_THEME.swap(light, Ordering::Relaxed) == light {
        return;
    }
    let settings = VISIBLE.lock().await.clone();
    for i in visible_instances(Launcher::UUID).await {
        if let Some(s) = settings.get(&i.instance_id) {
            if let Err(e) = paint(&i, s).await {
                log::warn!("Repainting launcher: {e}");
            }
        }
    }
}
#[async_trait]
impl Action for Launcher {
    const UUID: ActionUuid = "com.stevengriffin.interstitial.graphs.launcher";
    type Settings = Settings;
    async fn will_appear(&self, i: &Instance, s: &Settings) -> OpenActionResult<()> {
        sync(i, s).await
    }
    async fn will_disappear(&self, i: &Instance, _: &Settings) -> OpenActionResult<()> {
        VISIBLE.lock().await.remove(&i.instance_id);
        Ok(())
    }
    async fn did_receive_settings(&self, i: &Instance, s: &Settings) -> OpenActionResult<()> {
        sync(i, s).await
    }
    async fn property_inspector_did_appear(
        &self,
        i: &Instance,
        s: &Settings,
    ) -> OpenActionResult<()> {
        inspector(i, s).await
    }
    async fn send_to_plugin(
        &self,
        i: &Instance,
        s: &Settings,
        msg: &Value,
    ) -> OpenActionResult<()> {
        match msg["event"].as_str() {
            Some("launcherReady") => inspector(i, s).await?,
            Some("applyLabelsToDevice") => {
                let Some(show) = msg["show"].as_bool() else {
                    return Ok(());
                };
                let all = VISIBLE.lock().await.clone();
                for target in visible_instances(Self::UUID).await {
                    if target.device_id != i.device_id {
                        continue;
                    }
                    if let Some(mut value) = all.get(&target.instance_id).cloned() {
                        value.show_label = show;
                        target.set_settings(&value).await?;
                        sync(&target, &value).await?;
                        target
                            .send_to_property_inspector(
                                json!({"event":"launcherLabels","show":show}),
                            )
                            .await?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    async fn key_down(&self, i: &Instance, s: &Settings) -> OpenActionResult<()> {
        let Some(app) = APPS.iter().find(|a| a.meta.id == s.app_id) else {
            return i.show_alert().await;
        };
        let Some(dir) = std::env::var_os("WINDIR") else {
            return i.show_alert().await;
        };
        let exe = std::path::Path::new(&dir).join("explorer.exe");
        if let Err(error) = std::process::Command::new(exe)
            .arg(format!("shell:AppsFolder\\{}", app.meta.id))
            .spawn()
        {
            log::warn!("Could not launch {}: {error}", app.meta.label);
            i.show_alert().await?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hiding_the_label_expands_and_centers_the_icon() {
        let red = Rgba([255, 0, 0, 255]);
        let app = App {
            meta: Entry {
                id: "test".into(),
                label: "Test".into(),
                slot: 0,
            },
            icon: RgbaImage::from_pixel(64, 64, red),
        };
        for light in [false, true] {
            let large = render(&app, false, 0, 0, light);
            let named = render(&app, true, 0, 0, light);
            assert_eq!(large.get_pixel(16, 16), &red);
            assert_eq!(large.get_pixel(127, 127), &red);
            assert_ne!(large.get_pixel(15, 15), &red);
            assert_eq!(named.get_pixel(36, 14), &red);
            assert_ne!(named.get_pixel(16, 100), &red);
        }
    }
}
