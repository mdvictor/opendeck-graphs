use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use std::io::Cursor;

const ICON_SIZE: u32 = 144;
const GRAPH_PADDING: u32 = 10;
const TITLE_HEIGHT: u32 = 35;
pub const DEFAULT_GAUGE_OUTER_RADIUS: f32 = 55.0;
pub const DEFAULT_GAUGE_THICKNESS: f32 = 18.0;

/// Color scheme for graph based on threshold
#[derive(Clone, Copy)]
pub struct ColorScheme {
    pub normal_color: Rgba<u8>,
    pub warning_color: Rgba<u8>,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            normal_color: Rgba([0, 255, 0, 255]),  // Green
            warning_color: Rgba([255, 0, 0, 255]), // Red
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GradientType {
    None,
    Linear,
    Radial,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ValuePos {
    /// Bottom for graph view, center for gauge view.
    Auto,
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Copy)]
pub struct BackgroundConfig {
    pub color1: Rgba<u8>,
    pub color2: Rgba<u8>,
    pub gradient: GradientType,
    pub balance: u8,
    pub softness: u8,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            color1: Rgba([0, 0, 0, 255]),
            color2: Rgba([32, 32, 32, 255]),
            gradient: GradientType::None,
            balance: 50,
            softness: 50,
        }
    }
}

/// Configuration for rendering a graph
pub struct GraphConfig {
    pub data_points: Vec<f32>,
    pub max_value: f32,
    pub min_value: f32,
    pub threshold: Option<f32>,
    pub color_scheme: ColorScheme,
    pub title: String,
    pub background: BackgroundConfig,
    /// Override for the metric title color. Falls back to the active line color.
    pub title_color: Option<Rgba<u8>>,
    /// Override for the area-under-line fill color. Falls back to the active line color.
    pub fill_color: Option<Rgba<u8>>,
    /// Color used when drawing the current-value text into the image.
    pub value_text_color: Rgba<u8>,
    /// Formatted value string to render into the image (e.g. "75.5°C"). None = don't draw.
    pub value_text: Option<String>,
    /// Where to place the value text inside the icon.
    pub value_text_position: ValuePos,
    /// Font size (px) for the value text.
    pub value_text_size: f32,
    /// Font size (px) for the metric title.
    pub title_size: f32,
    /// Outer radius of the gauge arc, in pixels.
    pub gauge_outer_radius: f32,
    /// Thickness of the gauge arc, in pixels.
    pub gauge_thickness: f32,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            data_points: Vec::new(),
            max_value: 100.0,
            min_value: 0.0,
            threshold: None,
            color_scheme: ColorScheme::default(),
            title: String::new(),
            background: BackgroundConfig::default(),
            title_color: None,
            fill_color: None,
            value_text_color: Rgba([255, 255, 255, 255]),
            value_text: None,
            value_text_position: ValuePos::Auto,
            value_text_size: 22.0,
            title_size: 25.0,
            gauge_outer_radius: DEFAULT_GAUGE_OUTER_RADIUS,
            gauge_thickness: DEFAULT_GAUGE_THICKNESS,
        }
    }
}

/// Generate a timeseries graph image with gradient fill
pub fn generate_graph(config: &GraphConfig) -> Result<RgbaImage> {
    let mut img = fill_background(&config.background);

    if config.data_points.is_empty() {
        return Ok(img);
    }

    // Title is always shown at the top
    let graph_height = ICON_SIZE - GRAPH_PADDING * 2 - TITLE_HEIGHT;
    let graph_width = ICON_SIZE - GRAPH_PADDING * 2;

    // Determine if we're in warning state (current value exceeds threshold)
    let current_value = config.data_points.last().copied().unwrap_or(0.0);
    let is_warning = config.threshold.map(|t| current_value > t).unwrap_or(false);

    let line_color = if is_warning {
        config.color_scheme.warning_color
    } else {
        config.color_scheme.normal_color
    };
    let title_color = config.title_color.unwrap_or(line_color);
    let fill_color = config.fill_color.unwrap_or(line_color);

    // Draw title at the top center
    draw_title(&mut img, &config.title, &title_color, config.title_size);

    // Normalize data points to graph coordinates
    let points = normalize_points(
        &config.data_points,
        config.min_value,
        config.max_value,
        graph_width,
        graph_height,
    );

    // Draw gradient fill under the line
    draw_gradient_fill(
        &mut img,
        &points,
        GRAPH_PADDING,
        GRAPH_PADDING + TITLE_HEIGHT,
        graph_height,
        &fill_color,
    );

    // Draw the connected line
    draw_connected_line(
        &mut img,
        &points,
        GRAPH_PADDING,
        GRAPH_PADDING + TITLE_HEIGHT,
        &line_color,
    );

    if let Some(text) = &config.value_text {
        draw_value_text(
            &mut img,
            text,
            config.value_text_position,
            config.value_text_size,
            &config.value_text_color,
            false,
            0.0,
        );
    }

    Ok(img)
}

/// Normalize data points to graph coordinates
fn normalize_points(
    data: &[f32],
    min_val: f32,
    max_val: f32,
    width: u32,
    height: u32,
) -> Vec<(u32, u32)> {
    let range = max_val - min_val;
    if range == 0.0 {
        return data
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let x = (i as f32 / (data.len() - 1).max(1) as f32 * width as f32) as u32;
                (x, height / 2)
            })
            .collect();
    }

    data.iter()
        .enumerate()
        .map(|(i, &val)| {
            let x = if data.len() > 1 {
                (i as f32 / (data.len() - 1) as f32 * width as f32) as u32
            } else {
                width / 2
            };

            // Invert Y because image coordinates go top to bottom
            let normalized = ((val - min_val) / range).clamp(0.0, 1.0);
            let y = height - (normalized * height as f32) as u32;

            (x, y)
        })
        .collect()
}

/// Draw connected line through data points with antialiasing
fn draw_connected_line(
    img: &mut RgbaImage,
    points: &[(u32, u32)],
    offset_x: u32,
    offset_y: u32,
    color: &Rgba<u8>,
) {
    if points.len() < 2 {
        if let Some(&(x, y)) = points.first() {
            draw_point(img, x + offset_x, y + offset_y, color);
        }
        return;
    }

    for i in 0..points.len() - 1 {
        let (x0, y0) = points[i];
        let (x1, y1) = points[i + 1];

        draw_line_segment(
            img,
            x0 + offset_x,
            y0 + offset_y,
            x1 + offset_x,
            y1 + offset_y,
            color,
        );
    }
}

/// Draw a line segment using Bresenham's algorithm
fn draw_line_segment(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, color: &Rgba<u8>) {
    let x0 = x0 as i32;
    let y0 = y0 as i32;
    let x1 = x1 as i32;
    let y1 = y1 as i32;

    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;

    let mut x = x0;
    let mut y = y0;

    loop {
        if x >= 0 && x < ICON_SIZE as i32 && y >= 0 && y < ICON_SIZE as i32 {
            img.put_pixel(x as u32, y as u32, *color);
        }

        if x == x1 && y == y1 {
            break;
        }

        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

/// Draw a single point
fn draw_point(img: &mut RgbaImage, x: u32, y: u32, color: &Rgba<u8>) {
    if x < ICON_SIZE && y < ICON_SIZE {
        img.put_pixel(x, y, *color);
    }
}

/// Draw gradient fill under the line
fn draw_gradient_fill(
    img: &mut RgbaImage,
    points: &[(u32, u32)],
    offset_x: u32,
    offset_y: u32,
    graph_height: u32,
    color: &Rgba<u8>,
) {
    if points.is_empty() {
        return;
    }

    // For each x position in the graph, fill from the line down to the bottom with gradient
    let min_x = points.iter().map(|(x, _)| *x).min().unwrap_or(0);
    let max_x = points.iter().map(|(x, _)| *x).max().unwrap_or(0);

    for x in min_x..=max_x {
        // Find the y value at this x by interpolating between points
        let y = interpolate_y_at_x(points, x);

        // Draw gradient from the line down to bottom
        let bottom_y = graph_height;
        for py in y..=bottom_y {
            let actual_x = x + offset_x;
            let actual_y = py + offset_y;

            if actual_x < ICON_SIZE && actual_y < ICON_SIZE {
                // Calculate alpha based on distance from line (gradient effect)
                let distance_from_line = (py - y) as f32;
                let gradient_range = (bottom_y - y).max(1) as f32;
                let alpha = (1.0 - (distance_from_line / gradient_range)) * 0.6; // Max 60% opacity

                let gradient_color = Rgba([color[0], color[1], color[2], (alpha * 255.0) as u8]);

                let bg = img.get_pixel(actual_x, actual_y);
                let blended = blend_colors(*bg, gradient_color);
                img.put_pixel(actual_x, actual_y, blended);
            }
        }
    }
}

/// Interpolate Y value at a given X coordinate
fn interpolate_y_at_x(points: &[(u32, u32)], x: u32) -> u32 {
    // Find the two points that bracket this x value
    for i in 0..points.len() - 1 {
        let (x0, y0) = points[i];
        let (x1, y1) = points[i + 1];

        if x >= x0 && x <= x1 {
            if x1 == x0 {
                return y0;
            }

            // Linear interpolation
            let t = (x - x0) as f32 / (x1 - x0) as f32;
            return (y0 as f32 + t * (y1 as f32 - y0 as f32)) as u32;
        }
    }

    // If x is outside range, return the nearest endpoint
    if let Some(&(_, y)) = points.first() {
        if x < points.first().unwrap().0 {
            return y;
        }
    }
    if let Some(&(_, y)) = points.last() {
        return y;
    }

    0
}

/// Blend two colors with alpha blending
fn blend_colors(bg: Rgba<u8>, fg: Rgba<u8>) -> Rgba<u8> {
    let fg_alpha = fg[3] as f32 / 255.0;
    let bg_alpha = bg[3] as f32 / 255.0;

    if fg_alpha == 0.0 {
        return bg;
    }

    let final_alpha = fg_alpha + bg_alpha * (1.0 - fg_alpha);

    if final_alpha == 0.0 {
        return Rgba([0, 0, 0, 0]);
    }

    let r = ((fg[0] as f32 * fg_alpha + bg[0] as f32 * bg_alpha * (1.0 - fg_alpha)) / final_alpha)
        as u8;
    let g = ((fg[1] as f32 * fg_alpha + bg[1] as f32 * bg_alpha * (1.0 - fg_alpha)) / final_alpha)
        as u8;
    let b = ((fg[2] as f32 * fg_alpha + bg[2] as f32 * bg_alpha * (1.0 - fg_alpha)) / final_alpha)
        as u8;
    let a = (final_alpha * 255.0) as u8;

    Rgba([r, g, b, a])
}

/// Draw title text centered at the top of the image.
fn draw_title(img: &mut RgbaImage, title: &str, color: &Rgba<u8>, size: f32) {
    let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
    let font = match FontRef::try_from_slice(font_data) {
        Ok(f) => f,
        Err(_) => return,
    };
    let scale = PxScale::from(size.clamp(8.0, 60.0));
    let width = measure_text_width(&font, scale, title);
    let x = ((ICON_SIZE as f32 - width) / 2.0).max(2.0).round() as i32;
    let y = 8; // top padding
    draw_text_mut(img, *color, x, y, scale, &font, title);
}

/// Convert image to base64 data URI
pub fn image_to_data_uri(img: &RgbaImage) -> Result<String> {
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    img.write_to(&mut cursor, image::ImageFormat::Png)?;

    let base64 = general_purpose::STANDARD.encode(&buffer);
    Ok(format!("data:image/png;base64,{}", base64))
}

/// Generate a gauge visualization
pub fn generate_gauge(config: &GraphConfig) -> Result<RgbaImage> {
    let mut img = fill_background(&config.background);

    if config.data_points.is_empty() {
        return Ok(img);
    }

    let current_value = config.data_points.last().copied().unwrap_or(0.0);
    let is_warning = config.threshold.map(|t| current_value > t).unwrap_or(false);

    let active_color = if is_warning {
        config.color_scheme.warning_color
    } else {
        config.color_scheme.normal_color
    };
    let title_color = config.title_color.unwrap_or(active_color);

    // Draw title at the top
    draw_title(&mut img, &config.title, &title_color, config.title_size);

    // Calculate gauge parameters for a horseshoe-shaped meter
    let center_x = ICON_SIZE / 2;
    let center_y = ICON_SIZE / 2 + 15; // Position center to keep arc within bounds
    let outer_radius = config.gauge_outer_radius.clamp(20.0, 70.0);
    let arc_thickness = config
        .gauge_thickness
        .clamp(2.0, outer_radius - 4.0);
    let inner_radius = outer_radius - arc_thickness;

    let start_angle = 135.0_f32.to_radians(); // Start at 135° for symmetric horseshoe
    let end_angle = 45.0_f32.to_radians(); // End at 45° (270 degree arc, symmetric)
    let arc_range = end_angle - start_angle + 2.0 * std::f32::consts::PI; // Handle wrap around

    // Calculate the percentage for current value
    let range = config.max_value - config.min_value;
    let percentage = if range > 0.0 {
        ((current_value - config.min_value) / range).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Calculate threshold percentage
    let threshold_percentage = config
        .threshold
        .map(|t| {
            if range > 0.0 {
                ((t - config.min_value) / range).clamp(0.0, 1.0)
            } else {
                0.8
            }
        })
        .unwrap_or(0.8);

    let filled_angle = start_angle + (percentage * arc_range);
    let threshold_angle = start_angle + (threshold_percentage * arc_range);

    // Draw background arc with threshold coloring
    // First, draw the normal zone (0% to threshold)
    let normal_bg_color = Rgba([
        config.color_scheme.normal_color[0] / 3,
        config.color_scheme.normal_color[1] / 3,
        config.color_scheme.normal_color[2] / 3,
        180,
    ]);
    draw_thick_arc(
        &mut img,
        center_x,
        center_y,
        inner_radius,
        outer_radius,
        start_angle,
        threshold_angle,
        &normal_bg_color,
    );

    // Then, draw the warning zone (threshold to 100%)
    if config.threshold.is_some() {
        let warning_bg_color = Rgba([
            config.color_scheme.warning_color[0] / 3,
            config.color_scheme.warning_color[1] / 3,
            config.color_scheme.warning_color[2] / 3,
            180,
        ]);
        draw_thick_arc(
            &mut img,
            center_x,
            center_y,
            inner_radius,
            outer_radius,
            threshold_angle,
            end_angle,
            &warning_bg_color,
        );
    } else {
        // No threshold, draw remaining arc in normal color
        draw_thick_arc(
            &mut img,
            center_x,
            center_y,
            inner_radius,
            outer_radius,
            threshold_angle,
            end_angle,
            &normal_bg_color,
        );
    }

    // Draw filled arc (progress) - determine color based on threshold
    let fill_color = if percentage > threshold_percentage && config.threshold.is_some() {
        config.color_scheme.warning_color
    } else {
        config.color_scheme.normal_color
    };

    draw_thick_arc(
        &mut img,
        center_x,
        center_y,
        inner_radius,
        outer_radius,
        start_angle,
        filled_angle,
        &fill_color,
    );

    if let Some(text) = &config.value_text {
        draw_value_text(
            &mut img,
            text,
            config.value_text_position,
            config.value_text_size,
            &config.value_text_color,
            true,
            center_y as f32,
        );
    }

    Ok(img)
}

/// Draw a thick arc between two angles
fn draw_thick_arc(
    img: &mut RgbaImage,
    center_x: u32,
    center_y: u32,
    inner_radius: f32,
    outer_radius: f32,
    start_angle: f32,
    end_angle: f32,
    color: &Rgba<u8>,
) {
    let cx = center_x as f32;
    let cy = center_y as f32;

    // Iterate through all pixels in the bounding box
    let min_x = (cx - outer_radius).max(0.0) as u32;
    let max_x = (cx + outer_radius).min(ICON_SIZE as f32) as u32;
    let min_y = (cy - outer_radius).max(0.0) as u32;
    let max_y = (cy + outer_radius).min(ICON_SIZE as f32) as u32;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let distance = (dx * dx + dy * dy).sqrt();

            // Check if pixel is within the arc ring
            if distance >= inner_radius && distance <= outer_radius {
                // Calculate angle of this pixel
                let mut angle = dy.atan2(dx);

                // Normalize angle to 0..2π range
                if angle < 0.0 {
                    angle += 2.0 * std::f32::consts::PI;
                }

                // Check if angle is within the arc range
                let mut start = start_angle;
                let mut end = end_angle;

                // Normalize start and end angles to 0..2π range
                if start < 0.0 {
                    start += 2.0 * std::f32::consts::PI;
                }
                if end < 0.0 {
                    end += 2.0 * std::f32::consts::PI;
                }

                let in_range = if start <= end {
                    angle >= start && angle <= end
                } else {
                    // Arc wraps around 0
                    angle >= start || angle <= end
                };

                if in_range {
                    img.put_pixel(x, y, *color);
                }
            }
        }
    }
}

/// Generate a graph and return it as a data URI
pub fn generate_graph_data_uri(config: &GraphConfig) -> Result<String> {
    let img = generate_graph(config)?;
    image_to_data_uri(&img)
}

/// Generate a gauge and return it as a data URI
pub fn generate_gauge_data_uri(config: &GraphConfig) -> Result<String> {
    let img = generate_gauge(config)?;
    image_to_data_uri(&img)
}

/// Fill the entire icon with the configured background (solid or gradient).
fn fill_background(bg: &BackgroundConfig) -> RgbaImage {
    match bg.gradient {
        GradientType::None => RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, bg.color1),
        GradientType::Linear | GradientType::Radial => {
            // Match SVG-style stops as in nvidia-gpu-stats: midpoint in 25..75, half-width in 0..50
            let midpoint = (bg.balance.min(100) as f32) / 100.0 * 0.5 + 0.25;
            let half_width = (bg.softness.min(100) as f32) / 100.0 * 0.5;
            let stop1 = (midpoint - half_width).clamp(0.0, 1.0);
            let stop2 = (midpoint + half_width).clamp(0.0, 1.0);

            let cx = ICON_SIZE as f32 / 2.0;
            let cy = ICON_SIZE as f32 / 2.0;
            // Use ~70% of half-size as the gradient radius, matching the SVG reference (r="70%")
            let max_radial = (ICON_SIZE as f32 / 2.0) * 0.7;

            let mut img = RgbaImage::new(ICON_SIZE, ICON_SIZE);
            for y in 0..ICON_SIZE {
                for x in 0..ICON_SIZE {
                    let offset = match bg.gradient {
                        GradientType::Radial => {
                            let dx = x as f32 - cx;
                            let dy = y as f32 - cy;
                            ((dx * dx + dy * dy).sqrt() / max_radial).clamp(0.0, 1.0)
                        }
                        // Linear vertical (top → bottom)
                        _ => y as f32 / (ICON_SIZE - 1) as f32,
                    };

                    let color = interpolate_stops(offset, stop1, stop2, bg.color1, bg.color2);
                    img.put_pixel(x, y, color);
                }
            }
            img
        }
    }
}

/// Linearly interpolate between two SVG-style stops.
fn interpolate_stops(
    offset: f32,
    stop1: f32,
    stop2: f32,
    color1: Rgba<u8>,
    color2: Rgba<u8>,
) -> Rgba<u8> {
    if offset <= stop1 {
        return color1;
    }
    if offset >= stop2 {
        return color2;
    }
    let span = (stop2 - stop1).max(1e-6);
    let t = ((offset - stop1) / span).clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| {
        (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
    };
    Rgba([
        lerp(color1[0], color2[0]),
        lerp(color1[1], color2[1]),
        lerp(color1[2], color2[2]),
        lerp(color1[3], color2[3]),
    ])
}

/// Measure rendered width of `text` at the given scale using the embedded font.
fn measure_text_width(font: &FontRef, scale: PxScale, text: &str) -> f32 {
    let scaled = font.as_scaled(scale);
    text.chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum()
}

/// Resolve `ValuePos::Auto` based on visualization (gauge → Center, graph → Bottom).
fn resolve_value_pos(pos: ValuePos, is_gauge: bool) -> ValuePos {
    match pos {
        ValuePos::Auto => {
            if is_gauge {
                ValuePos::Center
            } else {
                ValuePos::Bottom
            }
        }
        other => other,
    }
}

/// Draw the current value centered horizontally, at the requested vertical position.
fn draw_value_text(
    img: &mut RgbaImage,
    text: &str,
    pos: ValuePos,
    size: f32,
    color: &Rgba<u8>,
    is_gauge: bool,
    gauge_center_y: f32,
) {
    let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
    let font = match FontRef::try_from_slice(font_data) {
        Ok(f) => f,
        Err(_) => return,
    };
    let scale = PxScale::from(size.clamp(8.0, 60.0));
    let width = measure_text_width(&font, scale, text);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let height = ascent - descent;

    let x = ((ICON_SIZE as f32 - width) / 2.0).round() as i32;
    let resolved = resolve_value_pos(pos, is_gauge);
    let y = match resolved {
        ValuePos::Top => 6,
        ValuePos::Center => {
            // For gauge, center on the ring's center for visual alignment; for graph use icon midpoint.
            let cy = if is_gauge {
                gauge_center_y
            } else {
                ICON_SIZE as f32 / 2.0
            };
            (cy - height / 2.0).round() as i32
        }
        ValuePos::Bottom => (ICON_SIZE as f32 - height - 6.0).round() as i32,
        ValuePos::Auto => unreachable!(),
    };
    draw_text_mut(img, *color, x, y, scale, &font, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_points() -> Vec<f32> {
        (0..60)
            .map(|i| {
                let t = i as f32 / 60.0;
                40.0 + 35.0 * (t * std::f32::consts::TAU).sin().abs() + (i as f32 * 0.3) % 5.0
            })
            .collect()
    }

    fn write_png(name: &str, img: &RgbaImage) {
        let path = format!("/tmp/opendeck-graphs-preview/{}.png", name);
        std::fs::create_dir_all("/tmp/opendeck-graphs-preview").unwrap();
        img.save(&path).unwrap();
        println!("wrote {}", path);
    }

    #[test]
    fn render_preview_samples() {
        let base_data = sample_points();

        let base = || GraphConfig {
            data_points: base_data.clone(),
            max_value: 100.0,
            min_value: 0.0,
            threshold: Some(80.0),
            color_scheme: ColorScheme {
                normal_color: Rgba([0, 220, 130, 255]),
                warning_color: Rgba([255, 80, 80, 255]),
            },
            title: "CPU Temp".to_string(),
            value_text: Some("75.5°C".to_string()),
            ..Default::default()
        };

        // 1. Solid black bg (baseline)
        let cfg = base();
        write_png("graph_solid_black", &generate_graph(&cfg).unwrap());
        write_png("gauge_solid_black", &generate_gauge(&cfg).unwrap());

        // 2. Linear vertical gradient
        let mut cfg = base();
        cfg.background = BackgroundConfig {
            color1: Rgba([12, 18, 28, 255]),
            color2: Rgba([60, 80, 120, 255]),
            gradient: GradientType::Linear,
            balance: 50,
            softness: 60,
        };
        write_png("graph_linear", &generate_graph(&cfg).unwrap());
        write_png("gauge_linear", &generate_gauge(&cfg).unwrap());

        // 3. Radial gradient
        let mut cfg = base();
        cfg.background = BackgroundConfig {
            color1: Rgba([40, 50, 80, 255]),
            color2: Rgba([5, 5, 10, 255]),
            gradient: GradientType::Radial,
            balance: 50,
            softness: 70,
        };
        write_png("graph_radial", &generate_graph(&cfg).unwrap());
        write_png("gauge_radial", &generate_gauge(&cfg).unwrap());

        // 4. Thin external gauge with bg
        let mut cfg = base();
        cfg.background = BackgroundConfig {
            color1: Rgba([20, 20, 30, 255]),
            color2: Rgba([5, 5, 10, 255]),
            gradient: GradientType::Radial,
            balance: 50,
            softness: 70,
        };
        cfg.gauge_outer_radius = 68.0;
        cfg.gauge_thickness = 6.0;
        cfg.value_text_color = Rgba([240, 240, 255, 255]);
        write_png("gauge_thin_external", &generate_gauge(&cfg).unwrap());

        // 5. Custom title + fill colors
        let mut cfg = base();
        cfg.background = BackgroundConfig {
            color1: Rgba([20, 20, 30, 255]),
            ..Default::default()
        };
        cfg.title_color = Some(Rgba([200, 200, 200, 255]));
        cfg.fill_color = Some(Rgba([255, 180, 50, 255]));
        write_png("graph_custom_colors", &generate_graph(&cfg).unwrap());

        // 6. Value text at top (graph)
        let mut cfg = base();
        cfg.value_text_position = ValuePos::Top;
        cfg.value_text_size = 28.0;
        cfg.title = String::new(); // hide title to avoid overlap
        write_png("graph_value_top_big", &generate_graph(&cfg).unwrap());

        // 7. Value at center, larger (graph)
        let mut cfg = base();
        cfg.value_text_position = ValuePos::Center;
        cfg.value_text_size = 30.0;
        cfg.value_text_color = Rgba([255, 240, 200, 255]);
        write_png("graph_value_center_big", &generate_graph(&cfg).unwrap());

        // 8. Title and value both larger (graph)
        let mut cfg = base();
        cfg.title_size = 18.0;
        cfg.value_text_size = 30.0;
        cfg.value_text_position = ValuePos::Bottom;
        write_png("graph_small_title_big_value", &generate_graph(&cfg).unwrap());

        // 9. Gauge with value at bottom instead of center
        let mut cfg = base();
        cfg.gauge_outer_radius = 55.0;
        cfg.gauge_thickness = 14.0;
        cfg.value_text_position = ValuePos::Bottom;
        cfg.value_text_size = 24.0;
        write_png("gauge_value_bottom", &generate_gauge(&cfg).unwrap());
    }
}
