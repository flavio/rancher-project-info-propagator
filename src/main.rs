mod cli;
mod controller;
mod errors;
mod project;

use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{filter::EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    // setup logging
    let level_filter = cli.log_level;
    let filter_layer = EnvFilter::from_default_env()
        .add_directive(level_filter.into())
        .add_directive("hyper=off".parse().unwrap()) // this crate generates tracing events we don't care about
        .add_directive("tower=off".parse().unwrap()); // this crate generates tracing events we don't care about
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    controller::run(cli.kubeconfig_upstream, cli.cluster_id).await?;

    Ok(())
}
