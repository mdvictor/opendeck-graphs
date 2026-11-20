use super::*;
use imageproc::drawing::{draw_filled_circle_mut, draw_line_segment_mut, text_size};
const INK: Rgba<u8> = Rgba([237, 242, 247, 255]);
const MUTED: Rgba<u8> = Rgba([159, 180, 200, 255]);

fn label(img: &mut RgbaImage, text: &str, x: i32, y: i32, size: f32, width: f32, color: Rgba<u8>) {
    // Retain the chosen ink unless it disappears on a light material.
    let bg = img.get_pixel(
        (x.max(0) as u32).min(img.width() - 1),
        (y.max(0) as u32).min(img.height() - 1),
    );
    let luma =
        |p: &Rgba<u8>| (54 * u32::from(p[0]) + 183 * u32::from(p[1]) + 19 * u32::from(p[2])) / 256;
    let color = if luma(bg) > 145 && luma(&color).abs_diff(luma(bg)) < 105 {
        Rgba([19, 28, 38, 255])
    } else {
        color
    };
    if network_caption(img, text, Some(x), y, size, width, color) {
        return;
    }
    let (w, _) = text_size(size, typeface::face(), text);
    let fitted = size * (width / w.max(1) as f32).min(1.0);
    draw_text_mut(img, color, x, y, fitted, typeface::face(), text);
}

fn active(c: &GraphConfig) -> Rgba<u8> {
    if c.threshold
        .zip(c.data_points.last())
        .is_some_and(|(t, v)| *v > t)
    {
        c.color_scheme.warning_color
    } else {
        c.color_scheme.normal_color
    }
}

fn fraction(value: f32, c: &GraphConfig) -> f32 {
    let span = c.max_value - c.min_value;
    if span <= 0.0 {
        0.0
    } else {
        ((value - c.min_value) / span).clamp(0.0, 1.0)
    }
}

/// Align the live end of both histories at the right edge. A new sample occupies
/// one second of width even while the 60-second history is still filling.
fn trace(img: &mut RgbaImage, c: &GraphConfig, x: i32, y: i32, w: i32, h: i32) {
    let color = active(c);
    let bottom = y + h;
    let rule = blend_colors(
        *img.get_pixel(x as u32, bottom as u32),
        Rgba([159, 180, 200, 65]),
    );
    draw_line_segment_mut(
        img,
        (x as f32, bottom as f32),
        ((x + w) as f32, bottom as f32),
        rule,
    );
    let points: Vec<_> = c
        .data_points
        .iter()
        .rev()
        .take(60)
        .enumerate()
        .map(|(i, v)| {
            (
                (x + w) as f32 - i as f32 * w as f32 / 59.0,
                bottom as f32 - fraction(*v, c) * h as f32,
            )
        })
        .collect();
    let fill = c.fill_color.unwrap_or(color);
    for pair in points.windows(2) {
        let (right, left) = (pair[0], pair[1]);
        for col in left.0.ceil() as i32..=right.0.floor() as i32 {
            let t = (col as f32 - left.0) / (right.0 - left.0);
            let top = (left.1 + (right.1 - left.1) * t).round() as i32;
            for row in top..bottom {
                let alpha = 82.0 * (bottom - row) as f32 / (bottom - top).max(1) as f32;
                let p = img.get_pixel_mut(col as u32, row as u32);
                *p = blend_colors(*p, Rgba([fill[0], fill[1], fill[2], alpha as u8]));
            }
        }
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        draw_line_segment_mut(img, a, b, color);
        draw_line_segment_mut(img, (a.0, a.1 + 1.0), (b.0, b.1 + 1.0), color);
    }
    if let Some(&(px, py)) = points.first() {
        draw_filled_circle_mut(img, (px.round() as i32, py.round() as i32), 2, INK);
    }
}

fn readout(c: &GraphConfig) -> &str {
    if c.data_points.is_empty() {
        "No data"
    } else {
        c.value_text.as_deref().unwrap_or("")
    }
}

fn lane(img: &mut RgbaImage, c: &GraphConfig, y: i32) {
    label(
        img,
        &c.title,
        9,
        y,
        17.0,
        102.0,
        c.title_color.unwrap_or(MUTED),
    );
    label(img, readout(c), 9, y + 16, 28.0, 102.0, c.value_text_color);
    trace(img, c, 116, y + 11, 73, 28);
}

pub(super) fn strip_background(settings: &BackgroundConfig) -> RgbaImage {
    // Cache one static surface per N4 section. Measurements are redrawn every tick.
    static TINTS: std::sync::Mutex<std::collections::VecDeque<(BackgroundConfig, RgbaImage)>> =
        std::sync::Mutex::new(std::collections::VecDeque::new());
    let mut cache = TINTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, image)) = cache.iter().find(|(key, _)| key == settings) {
        return image.clone();
    }
    let image = image::imageops::resize(
        &fill_background(settings),
        200,
        100,
        image::imageops::FilterType::Triangle,
    );
    if cache.len() == 4 {
        cache.pop_front();
    }
    cache.push_back((*settings, image.clone()));
    image
}

#[cfg(test)]
pub fn strip_image(
    c: &GraphConfig,
    gauge: bool,
    companion: Option<&GraphConfig>,
) -> Result<String> {
    strip_image_for_slot(c, gauge, companion, None)
}

pub fn strip_image_for_slot(
    c: &GraphConfig,
    gauge: bool,
    companion: Option<&GraphConfig>,
    slot: Option<u8>,
) -> Result<String> {
    #[cfg(windows)]
    let art = slot
        .filter(|_| c.background.glass)
        .and_then(|slot| super::wallpaper::glass(slot, &c.background, c.transparency));
    #[cfg(not(windows))]
    let art = {
        let _ = slot;
        None::<RgbaImage>
    };
    let mut img = art.unwrap_or_else(|| strip_background(&c.background));
    if let Some(other) = companion {
        lane(&mut img, c, 3);
        draw_line_segment_mut(
            &mut img,
            (9.0, 50.0),
            (190.0, 50.0),
            Rgba([63, 75, 88, 255]),
        );
        lane(&mut img, other, 52);
    } else if gauge {
        label(
            &mut img,
            &c.title,
            10,
            11,
            c.title_size,
            103.0,
            c.title_color.unwrap_or(MUTED),
        );
        label(
            &mut img,
            readout(c),
            10,
            37,
            c.value_text_size * 1.35,
            101.0,
            c.value_text_color,
        );
        let r = (c.gauge_outer_radius * 0.7).clamp(14.0, 39.0);
        let inner = r - (c.gauge_thickness * 0.62).clamp(2.0, r - 2.0);
        let start = 135f32.to_radians();
        draw_thick_arc(
            &mut img,
            (151, 51),
            inner,
            r,
            start,
            45f32.to_radians(),
            &Rgba([63, 75, 88, 255]),
        );
        if let Some(v) = c.data_points.last() {
            let sweep = fraction(*v, c) * 270.0;
            if sweep > 0.0 {
                draw_thick_arc(
                    &mut img,
                    (151, 51),
                    inner,
                    r,
                    start,
                    (135.0 + sweep).to_radians(),
                    &active(c),
                );
            }
        }
    } else {
        label(
            &mut img,
            &c.title,
            10,
            4,
            c.title_size,
            179.0,
            c.title_color.unwrap_or(MUTED),
        );
        let value = readout(c);
        label(
            &mut img,
            value,
            9,
            24,
            c.value_text_size * 1.65,
            181.0,
            c.value_text_color,
        );
        let top = if value.is_empty() { 34 } else { 63 };
        trace(&mut img, c, 10, top, 178, 92 - top);
    }
    image_to_data_uri(&img)
}

pub fn missing_image(c: &GraphConfig) -> Result<String> {
    let mut img = fill_background(&c.background);
    draw_title(
        &mut img,
        &c.title,
        &c.title_color.unwrap_or(MUTED),
        c.title_size,
    );
    label(&mut img, "No data", 18, 57, 29.0, 108.0, INK);
    image_to_data_uri(&img)
}
