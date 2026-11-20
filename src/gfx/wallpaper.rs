//! OpenDeck flattens encoder pixmaps to JPEG. Sample the installed artwork
//! under this column before drawing text, the trace and the glass reflection.
use super::*;
use std::{
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};
use toml_edit::DocumentMut;

struct Scene {
    art: RgbaImage,
    day: Option<RgbaImage>,
    config: std::path::PathBuf,
    cache: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    position: Option<(Instant, Option<u32>)>,
    crops: [Option<CachedCrop>; 4],
}
struct CachedCrop {
    key: CropKey,
    pixels: RgbaImage,
}
#[derive(PartialEq, Eq)]
struct CropKey {
    top: u32,
    day: bool,
    transparency: u8,
    background: BackgroundConfig,
}
impl Cache {
    fn top(&mut self, path: &std::path::Path) -> Option<u32> {
        if self
            .position
            .is_none_or(|(seen, _)| seen.elapsed() >= Duration::from_millis(100))
        {
            let top = std::fs::read_to_string(path)
                .ok()
                .and_then(|s| s.parse::<DocumentMut>().ok())
                .and_then(|d| placement(&d));
            self.position = Some((Instant::now(), top));
        }
        self.position?.1
    }
}
static SCENE: LazyLock<Option<Scene>> = LazyLock::new(|| {
    let dir = dirs::config_dir()?.join("Interstitial");
    let art = image::open(dir.join("live-wallpaper.png"))
        .ok()?
        .into_rgba8();
    if art.dimensions() != (800, 480) {
        return None;
    }
    Some(Scene {
        art,
        day: image::open(dir.join("day-wallpaper.png"))
            .ok()
            .filter(|im| im.width() == 800 && im.height() == 480)
            .map(|im| im.into_rgba8()),
        config: dir.join("config.toml"),
        cache: Mutex::new(Cache::default()),
    })
});

fn placement(doc: &DocumentMut) -> Option<u32> {
    if doc.get("continuous_strip")?.as_bool() != Some(true) {
        return None;
    }
    let y = doc
        .get("strip_native_y")
        .and_then(|v| v.as_integer())
        .unwrap_or(0);
    let shift = doc
        .get("strip_center_offset")
        .and_then(|v| v.as_integer())
        .unwrap_or(0);
    ((0..=320).contains(&y) && (-30..=30).contains(&shift)).then(|| (350 - y + shift) as u32)
}

pub(crate) fn light() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((seen, value)) = *cache {
        if seen.elapsed() < Duration::from_millis(160) {
            return value;
        }
    }
    let value = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .unwrap_or(0)
        != 0;
    *cache = Some((Instant::now(), value));
    value
}
pub(crate) fn artwork(day: bool) -> Option<&'static RgbaImage> {
    let scene = SCENE.as_ref()?;
    Some(if day {
        scene.day.as_ref().unwrap_or(&scene.art)
    } else {
        &scene.art
    })
}
pub(super) fn glass(
    slot: u8,
    background: &BackgroundConfig,
    transparency: u8,
) -> Option<RgbaImage> {
    let scene = SCENE.as_ref()?;
    let mut cache = scene.cache.lock().ok()?;
    let key = CropKey {
        top: cache.top(&scene.config)?,
        day: light(),
        transparency,
        background: *background,
    };
    let entry = cache.crops.get_mut(usize::from(slot))?;
    if entry.as_ref().is_none_or(|crop| crop.key != key) {
        let mut pixels = glass_crop(artwork(key.day)?, slot, key.top);
        tint_crop(&mut pixels, background, transparency);
        *entry = Some(CachedCrop { key, pixels });
    }
    Some(entry.as_ref()?.pixels.clone())
}

fn tint_crop(image: &mut RgbaImage, background: &BackgroundConfig, transparency: u8) {
    if transparency < 100 {
        let base = super::strip::strip_background(background);
        for (pix, mat) in image.pixels_mut().zip(base.pixels()) {
            for channel in 0..3 {
                let alpha = u32::from(transparency);
                pix[channel] = ((u32::from(pix[channel]) * alpha
                    + u32::from(mat[channel]) * (100 - alpha)
                    + 50)
                    / 100) as u8;
            }
        }
    }
}

fn glass_crop(art: &RgbaImage, slot: u8, top: u32) -> RgbaImage {
    RgbaImage::from_fn(200, 100, |x, y| {
        let mut pixel = *art.get_pixel(200 * u32::from(slot) + x, top + y);
        let u = x as f32 / 199.0;
        let v = y as f32 / 99.0;
        let sweep = (1.0 - ((v + 0.25 * u - 0.26) / 0.26).abs()).max(0.0);
        // Specular reflection only. The artwork supplies the entire base.
        let light = 0.18 * sweep * sweep * (4.0 * u * (1.0 - u));
        for (channel, tint) in pixel.0[..3].iter_mut().zip([170.0, 201.0, 223.0]) {
            *channel = (*channel as f32 + (tint - *channel as f32) * light).round() as u8;
        }
        pixel
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_absolute_artwork_and_keeps_seams_free_of_a_blue_fill() {
        let art = RgbaImage::from_fn(800, 480, |x, y| {
            Rgba([(x % 197) as u8, (y % 193) as u8, 53, 255])
        });
        for i in 0..4 {
            let tile = glass_crop(&art, i, 364);
            assert_eq!(
                tile.get_pixel(0, 73),
                art.get_pixel(u32::from(i) * 200, 437)
            );
            assert_eq!(
                tile.get_pixel(199, 73),
                art.get_pixel(u32::from(i) * 200 + 199, 437)
            );
        }
    }
}
