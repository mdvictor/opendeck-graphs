//! Edit only the device plugin's top-level offset. Preserve all other settings.
use anyhow::{anyhow, ensure, Context, Result};
use openaction::{get_connected_devices, Instance, OpenActionResult};
use serde_json::{json, Value};
use std::{io::Write, path::Path};
use tokio::sync::Mutex;
use toml_edit::{DocumentMut, Item};

static EDIT: Mutex<()> = Mutex::const_new(());

fn access(path: &Path, desired: Option<i64>) -> Result<Option<i64>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let original = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = original.parse()?;
    if doc.get("continuous_strip").and_then(Item::as_bool) != Some(true) {
        return Ok(None);
    }
    let current = match doc.get("strip_center_offset") {
        None => 0,
        Some(item) => item.as_integer().context("Invalid strip_center_offset")?,
    };
    ensure!(
        (-30..=30).contains(&current),
        "Stored alignment is out of range"
    );
    let Some(value) = desired else {
        return Ok(Some(current));
    };
    ensure!(
        (-30..=30).contains(&value),
        "Choose an offset from -30 to +30"
    );
    if current != value {
        let mut updated = toml_edit::Value::from(value);
        if let Some(old) = doc.get("strip_center_offset").and_then(Item::as_value) {
            *updated.decor_mut() = old.decor().clone();
        }
        doc["strip_center_offset"] = Item::Value(updated);
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("Missing parent")?)?;
        temp.write_all(doc.to_string().as_bytes())?;
        temp.flush()?;
        ensure!(
            std::fs::read_to_string(path)? == original,
            "Device configuration changed while saving"
        );
        temp.persist(path)?;
    }
    Ok(Some(value))
}

pub async fn respond(instance: &Instance, message: &Value) -> OpenActionResult<()> {
    let hardware = get_connected_devices().await;
    if instance.controller != "Encoder"
        || hardware
            .get(&instance.device_id)
            .is_none_or(|d| d.name != "VSDInside N4 Pro")
    {
        return Ok(());
    }
    let request = message["request"].as_u64().unwrap_or(0);
    let write = message["event"] == "setStripAlignment";
    let value = message["value"].as_i64();
    let _edit = EDIT.lock().await;
    let result = tokio::task::spawn_blocking(move || {
        if write && value.is_none() {
            return Err(anyhow!("Alignment must be a whole number"));
        }
        let path = dirs::config_dir()
            .context("Missing configuration directory")?
            .join("Interstitial/config.toml");
        access(&path, if write { value } else { None })
    })
    .await;
    let response = match result {
        Ok(Ok(value)) => json!({"event":"stripAlignment","request":request,"value":value}),
        other => {
            log::warn!("N4 alignment access failed: {other:?}");
            json!({"event":"stripAlignment","request":request,"error":"Could not read or save the N4 strip alignment."})
        }
    };
    drop(_edit);
    instance.send_to_property_inspector(response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_only_the_offset_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original="continuous_strip = true\nstrip_center_offset = 14 # physical calibration\n[leds]\nbrightness = 60\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(access(&path, None).unwrap(), Some(14));
        assert_eq!(access(&path, Some(0)).unwrap(), Some(0));
        assert!(access(&path, Some(31)).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.replace("14 #", "0 #")
        );
        assert_eq!(access(&path, Some(14)).unwrap(), Some(14));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}
