mod controller;
mod errors;
mod project;

use tracing_subscriber::prelude::*;
use tracing_subscriber::{
    filter::{EnvFilter, LevelFilter},
    fmt,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // setup logging
    //let verbose = matches
    //    .get_one::<bool>("verbose")
    //    .unwrap_or(&false)
    //    .to_owned();
    let verbose = true;
    let level_filter = if verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let filter_layer = EnvFilter::from_default_env()
        .add_directive(level_filter.into())
        .add_directive("hyper=off".parse().unwrap()) // this crate generates tracing events we don't care about
        .add_directive("tower=off".parse().unwrap()); // this crate generates tracing events we don't care about
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    let controller = controller::run();

    controller.await;
    Ok(())
}
