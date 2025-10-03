use openaction::OpenActionResult;

mod gfx;
mod plugin;
mod sensors;
mod websocket;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Debug,
        simplelog::Config::default(),
        simplelog::TerminalMode::Stdout,
        simplelog::ColorChoice::Never,
    )
    .unwrap();

    log::info!("Starting Graphs plugin...");

    plugin::init().await
}
