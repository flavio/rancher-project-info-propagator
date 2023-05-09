use thiserror::Error;

/// Types of errors raised by the code
#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum Error {
    /// Issue related with Kubernetes operations
    #[error("Kube Error: {0}")]
    Kube(#[source] kube::Error),

    /// Issue related with the parsing of the Kubeconfig file
    #[error("Error parsing Kubeconfig: {0}")]
    Kubeconfig(#[source] kube::config::KubeconfigError),

    /// Caching issue with the internal sqlite database
    #[error("{0}: {1}")]
    Sqlite(String, #[source] sqlx::Error),

    /// A generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
