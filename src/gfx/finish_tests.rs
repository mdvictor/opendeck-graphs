use super::*;
use crate::graph_data::{GraphData, GraphSettings};

fn background(settings: serde_json::Value) -> BackgroundConfig {
    let settings: GraphSettings = serde_json::from_value(settings).unwrap();
    GraphData::new(settings).get_graph_config().background
}

#[test]
fn finishes_preserve_existing_colors_and_blend_new_strips_at_the_bezel() {
    let glass = background(serde_json::json!({}));
    assert!(glass.glass);
    let pixels = fill_background(&glass);
    assert_eq!(pixels.get_pixel(72, 0), &Rgba([0, 0, 0, 255]));
    assert_eq!(pixels.get_pixel(0, 72), pixels.get_pixel(143, 72));
    assert_ne!(pixels.get_pixel(72, 20), pixels.get_pixel(72, 110));
    for settings in [
        serde_json::json!({"bg_color":"#123456"}),
        serde_json::json!({"bg_color":"#123456","finish":"solid"}),
    ] {
        let solid = background(settings);
        assert!(!solid.glass);
        assert!(fill_background(&solid)
            .pixels()
            .all(|p| *p == Rgba([18, 52, 86, 255])));
    }
    assert!(background(serde_json::json!({"bg_color":"#123456","finish":"glass"})).glass);
}

#[test]
#[ignore = "manual strip rendering benchmark"]
fn render_strip_frames() {
    let mut config = GraphConfig {
        title: "CPU Temp".into(),
        value_text: Some("58.9°C".into()),
        data_points: (0..60).map(|n| 50.0 + (n % 11) as f32).collect(),
        background: background(serde_json::json!({"finish":"glass"})),
        ..Default::default()
    };
    let now = std::time::Instant::now();
    for i in 0..1000 {
        config.data_points[59] = 50.0 + (i % 11) as f32;
        std::hint::black_box(strip_image(&config, false, None).unwrap());
    }
    println!(
        "1000 complete strip renders: {:.1} ms",
        now.elapsed().as_secs_f64() * 1000.0
    );
    let encoded = strip_image(&config, false, None).unwrap();
    let png = general_purpose::STANDARD
        .decode(encoded.split_once(',').unwrap().1)
        .unwrap();
    std::fs::create_dir_all("target/previews").unwrap();
    std::fs::write("target/previews/benchmark-strip.png", png).unwrap();
}
#[test]
fn network_captions_render_distinct_arrow_shapes_on_square_and_strip() {
    let mut panels = Vec::new();
    for metric in ["netupload", "netdownload"] {
        let settings =
            serde_json::from_value(serde_json::json!({"metric_type":metric,"finish":"solid"}))
                .unwrap();
        let mut data = GraphData::new(settings);
        data.add_data_point(1.0);
        panels.push(data.get_graph_config());
    }
    assert_eq!(
        (&panels[0].title, &panels[1].title),
        (&"Net 🡩".to_owned(), &"Net 🡫".to_owned())
    );
    let up = generate_graph(&panels[0]).unwrap();
    let down = generate_graph(&panels[1]).unwrap();
    assert_ne!(up, down);
    let uri = strip_image(&panels[0], false, Some(&panels[1])).unwrap();
    let bytes = general_purpose::STANDARD
        .decode(uri.split_once(',').unwrap().1)
        .unwrap();
    std::fs::create_dir_all("target/previews").unwrap();
    std::fs::write("target/previews/network-arrows.png", bytes).unwrap();
}

#[test]
fn opaque_blend_matches_source_over_at_every_alpha() {
    for alpha in 0..=255u8 {
        let a = f32::from(alpha) / 255.0;
        let out = a + (1.0 - a);
        for n in 0..=255u8 {
            let bg = Rgba([n, 0, 255 - n, 255]);
            let fg = Rgba([255 - n, n, 37, alpha]);
            let expected =
                |i: usize| ((f32::from(fg[i]) * a + f32::from(bg[i]) * (1.0 - a)) / out) as u8;
            assert_eq!(
                blend_colors(bg, fg),
                Rgba([expected(0), expected(1), expected(2), (out * 255.0) as u8])
            );
        }
    }
}
