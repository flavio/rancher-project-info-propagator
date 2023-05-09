use crate::context::Context;
use crate::errors::{Error, Result};
use crate::namespace::propagate_labels;
use crate::project::Project;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, ResourceExt},
    runtime::{
        controller::{Action, Controller},
        reflector::ObjectRef,
        watcher,
    },
};
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::time::Duration;
use tracing::{error, info, warn};

lazy_static! {
    static ref RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);
}

/// Reconciliation loop of the Namespace controller.
async fn reconcile(namespace: Arc<Namespace>, ctx: Arc<Context>) -> Result<Action> {
    if namespace.metadata.deletion_timestamp.is_some() {
        // namespace has been deleted, nothing to do
        return Ok(Action::requeue(*RECONCILIATION_INTERVAL));
    }

    let project_ref: Option<ObjectRef<Project>> = namespace
        .annotations()
        .get(crate::project::NAMESPACE_ANNOTATION)
        .and_then(|project_annotation| {
            project_annotation
                .split_once(':')
                .map(|(prj_ns, prj_name)| ObjectRef::<Project>::new(prj_name).within(prj_ns))
        });

    if let Some(project_ref) = project_ref {
        info!(
            namespace = namespace.name_unchecked(),
            project_namespace = project_ref.namespace,
            project_name = project_ref.name,
            "Update to Namespace owned by a Project"
        );

        let relevant_labels = if ctx.is_downstream_cluster() {
            if ctx.is_upstream_cluster_reachable().await {
                // upstream cluster is reachable
                let projects = ctx.projects_api();
                let project = projects.get(&project_ref.name).await.map_err(Error::Kube)?;
                project.relevant_labels()
            } else {
                warn!("connection to upstream cluster is broken, relying on cached data");
                ctx.cache_labels_to_propagate(&project_ref.name)
                    .await?
                    .unwrap_or_default()
            }
        } else {
            // running inside of upstream cluster
            let projects = ctx.projects_api();
            let project = projects.get(&project_ref.name).await.map_err(Error::Kube)?;
            project.relevant_labels()
        };

        propagate_labels(&relevant_labels, &namespace, ctx.local_client()).await?;
    }

    // If no events were received, check back every 5 minutes
    Ok(Action::requeue(*RECONCILIATION_INTERVAL))
}

/// Error function called when the controller cannot run the reconciliation
/// loop
fn error_policy(namespace: Arc<Namespace>, error: &Error, ctx: Arc<Context>) -> Action {
    error!(
        namespace = ?namespace,
        is_downstream_cluster = ctx.is_downstream_cluster(),
        "reconcile failed: {error:?}");

    Action::requeue(*RECONCILIATION_INTERVAL)
}

/// Initialize the controller
pub async fn run(ctx: Arc<Context>) {
    let namespaces = Api::<Namespace>::all(ctx.local_client());

    Controller::new(namespaces, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;
}
