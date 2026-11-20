use openaction::OpenActionResult;

#[cfg(windows)]
mod cpu_temperature;
#[cfg(windows)]
mod pdh_rates;
#[cfg(windows)]
mod ring_sample;
mod gfx;
mod graph_data;
#[cfg(windows)]
mod launcher;
mod plugin;
#[cfg(not(windows))]
mod sensors;
#[cfg(windows)]
#[path = "windows_sensors.rs"]
mod sensors;
#[cfg(windows)]
mod strip_alignment;
mod websocket;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--probe") {
        println!("{}", sensors::probe());
        return Ok(());
    }
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Info,
        simplelog::Config::default(),
        simplelog::TerminalMode::Stdout,
        simplelog::ColorChoice::Never,
    )
    .unwrap();

    log::info!("Starting Graphs plugin...");

    plugin::init().await
}
