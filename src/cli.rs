use clap::builder::TypedValueParser;
use clap::Parser;
use tracing_subscriber::filter::LevelFilter;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {
    /// Log level
    #[arg(
        long,
        env = "PROPAGATOR_LOG_LEVEL",
        default_value_t = LevelFilter::INFO,
        value_parser = clap::builder::PossibleValuesParser::new(["trace", "debug", "info", "warn", "error"])
            .map(|s| s.parse::<LevelFilter>().unwrap()),
    )]
    pub log_level: LevelFilter,

    /// ID of the cluster. To be used when deployed inside of a downstream cluster
    #[clap(
        long,
        env = "PROPAGATOR_CLUSTER_ID",
        required(false),
        requires = "kubeconfig_upstream"
    )]
    pub cluster_id: Option<String>,

    /// Path to the kubeconfig file used to connect to the upstream cluster. To be used when
    /// deployed inside of a downstream cluster
    #[clap(
        long,
        env = "PROPAGATOR_KUBECONFIG_UPSTREAM",
        required(false),
        requires = "cluster_id"
    )]
    pub kubeconfig_upstream: Option<std::path::PathBuf>,

    /// Path where the sqlite database is going to be saved
    /// Required when the controller is deployed inside of a downstream cluster
    #[clap(long, env = "PROPAGATOR_DATA_PATH", required(false), default_value_t = String::from("."))]
    pub data_path: String,
}
