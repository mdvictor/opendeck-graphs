use ab_glyph::{FontRef, PxScale};
use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use std::io::Cursor;

const ICON_SIZE: u32 = 144;
const GRAPH_PADDING: u32 = 10;
const TITLE_HEIGHT: u32 = 35;

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

/// Configuration for rendering a graph
pub struct GraphConfig {
    pub data_points: Vec<f32>,
    pub max_value: f32,
    pub min_value: f32,
    pub threshold: Option<f32>,
    pub color_scheme: ColorScheme,
    pub title: String,
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
        }
    }
}

/// Generate a timeseries graph image with gradient fill
pub fn generate_graph(config: &GraphConfig) -> Result<RgbaImage> {
    let mut img = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 255]));

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

    // Draw title at the top center
    draw_title(&mut img, &config.title, &line_color);

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
        &line_color,
    );

    // Draw the connected line
    draw_connected_line(
        &mut img,
        &points,
        GRAPH_PADDING,
        GRAPH_PADDING + TITLE_HEIGHT,
        &line_color,
    );

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

/// Draw title text centered at the top of the image with larger font
fn draw_title(img: &mut RgbaImage, title: &str, color: &Rgba<u8>) {
    // Use embedded DejaVu Sans font
    let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
    let font = match FontRef::try_from_slice(font_data) {
        Ok(f) => f,
        Err(_) => return, // Silently fail if font not available
    };

    let scale = PxScale::from(25.0); // Larger font size
    let text_color = *color;

    // Calculate text width for centering
    let text_width = title.len() as f32 * 12.5; // Rough estimate
    let x_offset = ((ICON_SIZE as f32 - text_width) / 2.0).max(5.0) as i32;
    let y_offset = 8; // Top padding

    draw_text_mut(img, text_color, x_offset, y_offset, scale, &font, title);
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
    let mut img = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 255]));

    if config.data_points.is_empty() {
        return Ok(img);
    }

    let current_value = config.data_points.last().copied().unwrap_or(0.0);
    let is_warning = config.threshold.map(|t| current_value > t).unwrap_or(false);

    let text_color = if is_warning {
        config.color_scheme.warning_color
    } else {
        config.color_scheme.normal_color
    };

    // Draw title at the top
    draw_title(&mut img, &config.title, &text_color);

    // Calculate gauge parameters for a horseshoe-shaped meter
    let center_x = ICON_SIZE / 2;
    let center_y = ICON_SIZE / 2 + 15; // Position center to keep arc within bounds
    let outer_radius = 55.0;
    let arc_thickness = 18.0; // Thick arc for better visibility
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
