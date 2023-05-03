use crate::errors::{Error, Result};
use crate::project::Project;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Namespace;
use kube::config::Kubeconfig;
use kube::{
    api::{Api, ResourceExt},
    client::Client,
    runtime::{
        controller::{Action, Controller},
        reflector::ObjectRef,
        watcher,
    },
};
use lazy_static::lazy_static;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::time::Duration;
use tracing::{debug, error, info};

lazy_static! {
    static ref RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);
}

#[derive(Clone)]
pub struct UpstreamClusterContext {
    /// Kubernetes client for the upstream cluster
    pub client_upstream: Client,

    /// ID of the downstream cluster
    pub cluster_id: String,
}

// Context for our reconciler
#[derive(Clone)]
pub struct Context {
    /// Kubernetes client for the local cluster
    pub client_local: Client,

    /// Context data of the upstream cluster - Used only when the controller is deployed inside of
    /// a downstream cluster
    pub upstream_cluster_ctx: Option<UpstreamClusterContext>,
}

async fn reconcile(project: Arc<Project>, ctx: Arc<Context>) -> Result<Action> {
    let ns = project.namespace().expect("Project is namespaced");
    info!(
        "Reconciling Project \"{:?}\" ({}) in {}",
        project.spec.display_name,
        project.name_any(),
        ns
    );
    if project.metadata.deletion_timestamp.is_some() {
        // project has been deleted, nothing to do
        return Ok(Action::requeue(*RECONCILIATION_INTERVAL));
    }

    let namespaces = project.namespaces(ctx.client_local.clone()).await?;
    for ns in namespaces {
        if let Err(e) = project
            .propagate_labels(&ns, ctx.client_local.clone())
            .await
        {
            error!(error = ?e, namespace = ns.name_unchecked(), "Cannot propagate labels to namespace");
        }
    }

    // If no events were received, check back every 5 minutes
    Ok(Action::requeue(*RECONCILIATION_INTERVAL))
}

fn error_policy(project: Arc<Project>, error: &Error, ctx: Arc<Context>) -> Action {
    let is_downstream_cluster = ctx.upstream_cluster_ctx.is_some();
    error!(project = ?project, is_downstream_cluster, "reconcile failed: {error:?}");
    Action::requeue(*RECONCILIATION_INTERVAL)
}

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run(kubeconfig_upstream: Option<PathBuf>, cluster_id: Option<String>) -> Result<()> {
    if kubeconfig_upstream.is_some() != cluster_id.is_some() {
        panic!("Non matching kubeconfig_upstream and cluster_id, clap should prevent that from happening");
    }

    let upstream_cluster_ctx = match kubeconfig_upstream {
        None => None,
        Some(path) => {
            let client_upstream = create_upstream_client(path.as_path()).await?;
            Some(UpstreamClusterContext {
                client_upstream,
                cluster_id: cluster_id.unwrap(),
            })
        }
    };

    let client_local = Client::try_default().await.map_err(Error::Kube)?;

    let projects = match &upstream_cluster_ctx {
        Some(upstream_ctx) => {
            info!(
                cluster_id = upstream_ctx.cluster_id,
                "monitoring Projects defined inside of upstream cluster"
            );
            Api::<Project>::namespaced(
                upstream_ctx.client_upstream.clone(),
                &upstream_ctx.cluster_id,
            )
        }
        None => {
            info!("monitoring Projects defined inside of local cluster");
            Api::<Project>::all(client_local.clone())
        }
    };

    let context = Context {
        client_local: client_local.clone(),
        upstream_cluster_ctx,
    };

    Controller::new(projects, watcher::Config::default().any_semantic())
        .watches(
            Api::<Namespace>::all(client_local),
            watcher::Config::default().any_semantic(),
            |ns| {
                if let Some(project_annotation) =
                    ns.annotations().get(crate::project::NAMESPACE_ANNOTATION)
                {
                    if let Some((prj_ns, prj_name)) = project_annotation.split_once(':') {
                        debug!(
                            namespace = ns.name_unchecked(),
                            project_namespace = prj_ns,
                            project_name = prj_name,
                            "Update to Namespace owned by a Project"
                        );
                        return Some(ObjectRef::new(prj_name).within(prj_ns));
                    }
                }
                None
            },
        )
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(context))
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

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
