use clap::builder::TypedValueParser;
use clap::Parser;
use tracing_subscriber::filter::LevelFilter;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {
    /// Log level
    #[arg(
        long,
        default_value_t = LevelFilter::INFO,
        value_parser = clap::builder::PossibleValuesParser::new(["trace", "debug", "info", "warn", "error"])
            .map(|s| s.parse::<LevelFilter>().unwrap()),
    )]
    pub log_level: LevelFilter,

    /// ID of the cluster. To be used when deployed inside of a downstream cluster
    #[clap(long, required(false), requires = "kubeconfig_upstream")]
    pub cluster_id: Option<String>,

    /// Path to the kubeconfig file used to connect to the upstream cluster. To be used when
    /// deployed inside of a downstream cluster
    #[clap(long, required(false), requires = "cluster_id")]
    pub kubeconfig_upstream: Option<std::path::PathBuf>,
}
