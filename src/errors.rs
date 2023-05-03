use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Kube Error: {0}")]
    Kube(#[source] kube::Error),

    #[error("InternalError: {0}")]
    Internal(String),

    #[error("Error parsing Kubeconfig: {0}")]
    Kubeconfig(#[source] kube::config::KubeconfigError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
