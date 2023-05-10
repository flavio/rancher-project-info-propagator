use crate::errors::{Error, Result};
use crate::projects_cache::ProjectsCache;
use kube::{client::Client, config::Kubeconfig};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::RwLock;
use tracing::error;

/// Holds the details of the upstream cluster
#[derive(Clone)]
pub struct UpstreamClusterContext {
    /// Kubernetes client for the upstream cluster
    client_upstream: Client,

    /// ID of the downstream cluster
    cluster_id: String,
}

impl UpstreamClusterContext {
    /// Create a new instance of `UpstreamClusterContext`
    ///
    /// * `kubeconfig_upstream`: path to the kubeconfig file to be used to
    ///   connect to the upstream cluster
    /// * `cluster_id`: ID of the cluster upstream. Used to locate the Namespace
    ///   inside of the upstream cluster where all the Project objects are kept
    pub async fn new(kubeconfig_upstream: &Path, cluster_id: &str) -> Result<Self> {
        let client_upstream = Self::create_upstream_client(kubeconfig_upstream).await?;
        Ok(UpstreamClusterContext {
            client_upstream,
            cluster_id: cluster_id.to_string(),
        })
    }

    /// Create the `kube::Client` used to connect to the upstream cluster
    async fn create_upstream_client(kubeconfig_path: &Path) -> Result<Client> {
        let kubeconfig = Kubeconfig::read_from(kubeconfig_path).map_err(Error::Kubeconfig)?;

        let client_config = kube::Config::from_custom_kubeconfig(
            kubeconfig,
            &kube::config::KubeConfigOptions::default(),
        )
        .await
        .map_err(Error::Kubeconfig)?;

        Client::try_from(client_config).map_err(Error::Kube)
    }
}

/// Context for our reconcilers
#[derive(Clone)]
pub struct Context {
    /// Kubernetes client for the local cluster
    client_local: Client,

    /// Context data of the upstream cluster - Used only when the controller is
    /// deployed inside of a downstream cluster
    upstream_cluster_ctx: Option<UpstreamClusterContext>,

    /// Cache of the known Projects. Used only the the controller is deployed
    /// inside of a downstream cluster
    project_labels_cache: Option<Arc<RwLock<ProjectsCache>>>,
}

impl Context {
    /// `kube::Client` used to connect to the Kubernetes API server hosting
    /// the controller
    pub fn local_client(&self) -> Client {
        self.client_local.clone()
    }

    /// Whether the controller has been deployed inside of the downstream
    /// cluster or not
    pub fn is_downstream_cluster(&self) -> bool {
        self.upstream_cluster_ctx.is_some()
    }

    /// Checks whether the connection to the upstream cluster is still active.
    /// Relevant only when the controller is deployed inside of a downstream
    /// cluster
    pub async fn is_upstream_cluster_reachable(&self) -> bool {
        match &self.upstream_cluster_ctx {
            None => {
                error!("trying to verify connectivity towards upstream cluster, but the controller is deployed inside of the upstream cluster!");
                false
            }
            Some(ctx) => {
                let body: Vec<u8> = Vec::new();
                let request = http::Request::get("/version").body(body).unwrap();
                ctx.client_upstream.request_text(request).await.is_ok()
            }
        }
    }

    /// Create the context used when the controller is deployed inside of the
    /// cluster where Rancher Manager is running - aka the "upstream cluster"
    pub async fn upstream_cluster() -> Result<Self> {
        let client_local = Client::try_default().await.map_err(Error::Kube)?;
        Ok(Self {
            client_local,
            upstream_cluster_ctx: None,
            project_labels_cache: None,
        })
    }

    /// Create the context used when then controller is deployed inside of
    /// a cluster managed by Rancher Manager - aka a "downstream cluster"
    pub async fn downstream_cluster(
        kubeconfig_upstream: &Path,
        cluster_id: &str,
        data_path: &Path,
    ) -> Result<Self> {
        let client_local = Client::try_default().await.map_err(Error::Kube)?;
        let upstream_cluster_ctx =
            Some(UpstreamClusterContext::new(kubeconfig_upstream, cluster_id).await?);
        let project_labels_cache =
            Some(Arc::new(RwLock::new(ProjectsCache::init(data_path).await?)));

        Ok(Self {
            client_local,
            upstream_cluster_ctx,
            project_labels_cache,
        })
    }

    /// Build the `kube::Api` object required to interact with `Project` objects.
    ///
    /// The type of `Api` is built depending whether the controller is deployed
    /// inside of the upstream cluster or not
    pub fn projects_api(&self) -> kube::Api<crate::project::Project> {
        match &self.upstream_cluster_ctx {
            Some(upstream_ctx) => kube::Api::<crate::project::Project>::namespaced(
                upstream_ctx.client_upstream.clone(),
                &upstream_ctx.cluster_id,
            ),
            None => {
                kube::Api::<crate::project::Project>::namespaced(self.client_local.clone(), "local")
            }
        }
    }

    /// Cache: remove all references of a given project.
    /// Relevant only when the controller is deployed inside of a downstream
    /// cluster
    pub async fn cache_delete_project(&self, project_name: &str) -> Result<()> {
        match &self.project_labels_cache {
            Some(cache) => cache.write().await.delete_project(project_name).await,
            None => Ok(()),
        }
    }

    /// Cache: update the details of the given project
    /// Relevant only when the controller is deployed inside of a downstream
    /// cluster
    ///
    /// **Important:** `relevant_labels` must contain only the labels that have
    /// to be propagated. The keys must be stripped of the `propagate.` prefix
    pub async fn cache_update_project(
        &self,
        project_name: &str,
        relevant_labels: &BTreeMap<String, String>,
    ) -> Result<()> {
        match &self.project_labels_cache {
            Some(cache) => {
                cache
                    .write()
                    .await
                    .cache_labels(project_name, relevant_labels)
                    .await
            }
            None => Ok(()),
        }
    }

    /// Cache: obtain the list of relevant labels of the given project
    /// Relevant only when the controller is deployed inside of a downstream
    /// cluster
    pub async fn cache_labels_to_propagate(
        &self,
        project_name: &str,
    ) -> Result<Option<BTreeMap<String, String>>> {
        match &self.project_labels_cache {
            Some(cache) => cache.read().await.labels_to_propagate(project_name).await,
            None => Ok(None),
        }
    }
}
