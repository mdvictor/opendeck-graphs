#![cfg(windows)]
use anyhow::{ensure, Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use image::GenericImageView;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{net::TcpListener, process::Command, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const ACTION: &str = "com.stevengriffin.interstitial.graphs.action";

/// Exercise the shipped binary through its ordinary OpenDeck socket.
#[tokio::test]
async fn independent_encoder_and_key_frames() -> Result<()> {
    let binary =
        std::env::var("GRAPHS_TEST_EXE").unwrap_or_else(|_| env!("CARGO_BIN_EXE_oa-graphs").into());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port().to_string();
    let mut child = Command::new(&binary)
        .args([
            "-port",
            &port,
            "-pluginUUID",
            "graphs-test",
            "-registerEvent",
            "registerPlugin",
            "-info",
            r#"{"devices":[{"id":"n4-test","name":"VSDInside N4 Pro","type":7,"size":{"rows":2,"columns":5}}]}"#,
        ])
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .spawn()?;
    let (stream, _) = timeout(Duration::from_secs(10), listener.accept()).await??;
    let mut host = accept_async(stream).await?;
    let reg: Value = serde_json::from_str(host.next().await.context("register")??.to_text()?)?;
    ensure!(reg == json!({"event":"registerPlugin","uuid":"graphs-test"}));
    let probe = Command::new(&binary).arg("--probe").output().await?;
    ensure!(probe.status.success());
    let data: Value = serde_json::from_slice(&probe.stdout)?;
    let adapters = data["adapters"].as_array().context("adapters")?;
    let alternate = adapters
        .get(1)
        .and_then(|g| g["id"].as_str())
        .unwrap_or("card0");
    let settings = [
        json!({"metric_type":"gpuload","companion_metric":"gputemp"}),
        json!({"metric_type":"gpuload","gpu_card":alternate}),
        json!({"metric_type":"ramusage","visualization_type":"gauge"}),
        json!({"metric_type":"cputemp","companion_metric":"cpuload"}),
        json!({"data_source":"lmsensors","metric_type":"ramusage","normal_color":"#123456"}),
    ];
    let lifecycle = |i: usize, event: &str| {
        json!({"event":event,"action":ACTION,"context":format!("slot-{i}"),"device":"n4-test","payload":{
            "settings": settings[i], "coordinates":{"row":0,"column":i},"controller":if i<4 {"Encoder"} else {"Keypad"},"isInMultiAction":false
        }})
    };
    for i in 0..5 {
        host.send(Message::Text(lifecycle(i, "willAppear").to_string()))
            .await?;
    }
    host.send(Message::Text(json!({"event":"propertyInspectorDidAppear","action":ACTION,"context":"slot-0","device":"n4-test"}).to_string())).await?;
    let mut samples: HashMap<String, Vec<Instant>> = HashMap::new();
    let mut last = HashMap::new();
    let mut inspector = false;
    let mut gpu_list = false;
    let count = std::env::var("GRAPHS_TEST_FRAMES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(3)
        .max(3);
    timeout(Duration::from_secs((count * 4 + 15) as u64), async {
        while (0..5).any(|i| samples.get(&format!("slot-{i}")).map_or(0, Vec::len) < count)
            || !inspector
            || !gpu_list
        {
            let msg: Value = serde_json::from_str(
                host.next()
                    .await
                    .context("frame stream closed")??
                    .to_text()?,
            )?;
            let context = msg["context"].as_str().unwrap_or("");
            if msg["event"] == "sendToPropertyInspector" {
                inspector |= msg["payload"]["event"] == "frame";
                gpu_list |= msg["payload"]["event"] == "gpuList";
                continue;
            }
            let encoder = context != "slot-4";
            ensure!(msg["event"] == if encoder { "setFeedback" } else { "setImage" });
            let uri = msg["payload"][if encoder { "plot" } else { "image" }]
                .as_str()
                .context("missing image")?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(
                uri.strip_prefix("data:image/png;base64,")
                    .context("PNG prefix")?,
            )?;
            let image = image::load_from_memory(&bytes)?;
            ensure!(image.dimensions() == if encoder { (200, 100) } else { (144, 144) });
            if context == "slot-4" {
                ensure!(
                    image.to_rgb8().pixels().any(|p| p.0 == [18, 52, 86]),
                    "saved custom color lost"
                );
            }
            last.insert(context.to_owned(), image);
            samples
                .entry(context.to_owned())
                .or_default()
                .push(Instant::now());
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    let times = &samples["slot-0"];
    println!(
        "{} encoder frames over {:.2}s",
        times.len(),
        times.last().unwrap().duration_since(times[0]).as_secs_f64()
    );
    if let Some(path) = std::env::var_os("GRAPHS_PREVIEW_DIR") {
        let path = PathBuf::from(path);
        std::fs::create_dir_all(&path)?;
        let mut strip = image::RgbaImage::from_pixel(704, 112, image::Rgba([0, 0, 0, 255]));
        for i in 0..4 {
            let frame = last[&format!("slot-{i}")]
                .resize_exact(176, 88, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            image::imageops::overlay(&mut strip, &frame, i * 176, 12);
        }
        strip.save(path.join("n4-strip.png"))?;
        last["slot-0"].save(path.join("paired.png"))?;
    }
    let mut last_title = None;
    for paired in [false, false, true] {
        host.send(Message::Text(lifecycle(0, "dialDown").to_string()))
            .await?;
        timeout(Duration::from_secs(6), async {
            loop {
                let msg: Value = serde_json::from_str(
                    host.next()
                        .await
                        .context("closed during dial test")??
                        .to_text()?,
                )?;
                if msg["context"] != "slot-0" || msg["event"] != "setFeedback" {
                    continue;
                }
                let uri = msg["payload"]["plot"]
                    .as_str()
                    .context("missing dial frame")?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(
                    uri.strip_prefix("data:image/png;base64,")
                        .context("missing PNG")?,
                )?;
                let frame = image::load_from_memory(&bytes)?.into_rgb8();
                let divider = (10..188).all(|x| frame[(x, 50)].0 == [63, 75, 88]);
                let title = image::imageops::crop_imm(&frame, 10, 6, 100, 22).to_image();
                if divider == paired && (paired || last_title.as_ref() != Some(&title)) {
                    last_title = Some(title);
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
    for i in 0..5 {
        host.send(Message::Text(lifecycle(i, "willDisappear").to_string()))
            .await?;
    }
    host.close(None).await?;
    drop(host);
    ensure!(timeout(Duration::from_secs(8), child.wait())
        .await??
        .success());
    Ok(())
}
