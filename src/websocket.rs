use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// WebSocket data source configuration
#[derive(Clone)]
pub struct WebSocketConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub init_messages: Vec<String>,
    pub send_pings: bool,
}

/// WebSocket data source client
pub struct WebSocketClient {
    config: WebSocketConfig,
    current_value: Arc<Mutex<f32>>,
    ping_id: Arc<Mutex<u64>>,
}

impl WebSocketClient {
    pub fn new(config: WebSocketConfig) -> Self {
        let initial_ping_id = config.init_messages.len() as u64;
        Self {
            config,
            current_value: Arc::new(Mutex::new(0.0)),
            ping_id: Arc::new(Mutex::new(initial_ping_id)),
        }
    }

    /// Start the WebSocket connection and data fetching loop
    pub async fn start(&self) -> Result<()> {
        let config = self.config.clone();
        let current_value = self.current_value.clone();
        let ping_id = self.ping_id.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_connection(config, current_value, ping_id).await {
                log::error!("WebSocket connection error: {}", e);
            }
        });

        Ok(())
    }

    /// Get the current value
    pub async fn get_value(&self) -> f32 {
        *self.current_value.lock().await
    }

    async fn run_connection(
        config: WebSocketConfig,
        current_value: Arc<Mutex<f32>>,
        ping_id: Arc<Mutex<u64>>,
    ) -> Result<()> {
        loop {
            match Self::connect_and_run(&config, &current_value, &ping_id).await {
                Ok(_) => {
                    log::info!("WebSocket connection closed, reconnecting in 5 seconds...");
                }
                Err(e) => {
                    log::error!("WebSocket error: {}, reconnecting in 5 seconds...", e);
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn connect_and_run(
        config: &WebSocketConfig,
        current_value: &Arc<Mutex<f32>>,
        ping_id: &Arc<Mutex<u64>>,
    ) -> Result<()> {
        // Build URL with API key if provided
        let url = if let Some(api_key) = &config.api_key {
            if config.url.contains('?') {
                format!("{}&apikey={}", config.url, api_key)
            } else {
                format!("{}?apikey={}", config.url, api_key)
            }
        } else {
            config.url.clone()
        };

        log::info!("Connecting to WebSocket: {}", url);
        let (ws_stream, _) = connect_async(&url).await?;
        let (write, mut read) = ws_stream.split();

        // Wrap write in Arc<Mutex<>> for sharing between tasks
        let write = Arc::new(Mutex::new(write));

        log::info!("WebSocket connected, sending initialization messages");

        // Send initialization messages and wait for responses
        for (idx, init_msg) in config.init_messages.iter().enumerate() {
            log::debug!("Sending init message {}: {}", idx + 1, init_msg);
            write.lock().await.send(Message::Text(init_msg.clone())).await?;

            // Wait for response
            if let Some(response) = read.next().await {
                match response {
                    Ok(Message::Text(text)) => {
                        log::debug!("Init response {}: {}", idx + 1, text);
                    }
                    Ok(Message::Close(_)) => {
                        return Err(anyhow!("WebSocket closed during initialization"));
                    }
                    Err(e) => {
                        return Err(anyhow!("Error receiving init response: {}", e));
                    }
                    _ => {}
                }
            }
        }

        log::info!("Initialization complete, starting data loop");

        // Start ping task if enabled
        let ping_handle = if config.send_pings {
            let write_clone = write.clone();
            let ping_id_clone = ping_id.clone();
            Some(tokio::spawn(async move {
                let mut ping_interval = interval(Duration::from_secs(4));
                loop {
                    ping_interval.tick().await;
                    let mut id = ping_id_clone.lock().await;
                    let ping_msg = format!(r#"{{"ping":{{}},"id":{}}}"#, *id);
                    *id += 1;
                    drop(id);

                    log::debug!("Sending ping: {}", ping_msg);
                    if let Err(e) = write_clone.lock().await.send(Message::Text(ping_msg)).await {
                        log::error!("Failed to send ping: {}", e);
                        break;
                    }
                }
            }))
        } else {
            None
        };

        // Read data messages
        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    log::debug!("Received message: {}", text);
                    // Try to parse as JSON and extract numeric value
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        if let Some(value) = Self::extract_value(&json) {
                            let mut current = current_value.lock().await;
                            *current = value;
                            log::debug!("Updated value to: {}", value);
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    log::info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    log::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // Abort ping task if running
        if let Some(handle) = ping_handle {
            handle.abort();
        }

        Ok(())
    }

    /// Extract a numeric value from JSON response
    /// This is a simple heuristic - looks for first numeric field
    fn extract_value(json: &Value) -> Option<f32> {
        match json {
            Value::Number(n) => n.as_f64().map(|v| v as f32),
            Value::Object(map) => {
                // Try common field names first
                for key in &["value", "data", "result", "temperature", "temp", "load"] {
                    if let Some(val) = map.get(*key) {
                        if let Some(num) = Self::extract_value(val) {
                            return Some(num);
                        }
                    }
                }
                // If not found, try first numeric value in any field
                for (_key, val) in map.iter() {
                    if let Some(num) = Self::extract_value(val) {
                        return Some(num);
                    }
                }
                None
            }
            Value::Array(arr) => {
                // Try first element
                arr.first().and_then(|v| Self::extract_value(v))
            }
            _ => None,
        }
    }
}
