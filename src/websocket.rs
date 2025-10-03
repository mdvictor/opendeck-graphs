use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// WebSocket data source configuration
#[derive(Clone)]
pub struct WebSocketConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub init_messages: Vec<String>,
}

/// WebSocket data source client
pub struct WebSocketClient {
    config: WebSocketConfig,
    current_value: Arc<Mutex<f32>>,
}

impl WebSocketClient {
    pub fn new(config: WebSocketConfig) -> Self {
        Self {
            config,
            current_value: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Start the WebSocket connection and data fetching loop
    pub async fn start(&self) -> Result<()> {
        let config = self.config.clone();
        let current_value = self.current_value.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_connection(config, current_value).await {
                log::error!("WebSocket connection error: {}", e);
            }
        });

        Ok(())
    }

    /// Get the current value
    pub async fn get_value(&self) -> f32 {
        *self.current_value.lock().await
    }

    async fn run_connection(config: WebSocketConfig, current_value: Arc<Mutex<f32>>) -> Result<()> {
        loop {
            match Self::connect_and_run(&config, &current_value).await {
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
    ) -> Result<()> {
        log::info!("Connecting to WebSocket: {}", config.url);

        // Build request with Authorization header if API key is provided
        let ws_stream = if let Some(api_key) = &config.api_key {
            let request = Request::builder()
                .uri(&config.url)
                .header(
                    "Host",
                    config
                        .url
                        .split("//")
                        .nth(1)
                        .unwrap_or("")
                        .split('/')
                        .next()
                        .unwrap_or(""),
                )
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                )
                .header("Authorization", format!("Bearer {}", api_key))
                .body(())?;
            let (ws_stream, _) = connect_async(request).await?;
            ws_stream
        } else {
            let (ws_stream, _) = connect_async(&config.url).await?;
            ws_stream
        };

        let (write, mut read) = ws_stream.split();

        // Wrap write in Arc<Mutex<>> for sharing between tasks
        let write = Arc::new(Mutex::new(write));

        log::info!("WebSocket connected, sending initialization messages");

        // Send initialization messages and wait for responses
        for (idx, init_msg) in config.init_messages.iter().enumerate() {
            log::debug!("Sending init message {}: {}", idx + 1, init_msg);
            write
                .lock()
                .await
                .send(Message::Text(init_msg.clone()))
                .await?;

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
