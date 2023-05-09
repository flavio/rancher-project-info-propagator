mod cli;
mod context;
mod errors;
mod namespace;
mod namespaces_controller;
mod project;
mod projects_cache;
mod projects_controller;

use clap::Parser;
use std::{path::Path, sync::Arc};
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{filter::EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    // setup logging
    let level_filter = cli.log_level;
    let filter_layer = EnvFilter::from_default_env()
        .add_directive(level_filter.into())
        .add_directive("rustls=off".parse().unwrap()) // this crate generates tracing events we don't care about
        .add_directive("hyper=off".parse().unwrap()) // this crate generates tracing events we don't care about
        .add_directive("tower=off".parse().unwrap()); // this crate generates tracing events we don't care about
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    let context = Arc::new(match &cli.kubeconfig_upstream {
        Some(kubeconfig_upstream) => {
            // clap ensures cluster_id and kubeconfig_upstream are always
            // set at the same time
            let cluster_id = cli.cluster_id.as_ref().unwrap();

            let data_path = Path::new(&cli.data_path);

            info!(
                cluster_id,
                "monitoring Projects defined inside of upstream cluster"
            );

            context::Context::downstream_cluster(kubeconfig_upstream, cluster_id, data_path).await
        }
        None => {
            info!("monitoring Projects defined inside of local cluster");
            context::Context::upstream_cluster().await
        }
    }?);

    let projects_controller = projects_controller::run(context.clone());
    let namespaces_controller = namespaces_controller::run(context);

    // Both runtimes implements graceful shutdown, so poll until both are done
    tokio::join!(projects_controller, namespaces_controller).1;

    Ok(())
}
