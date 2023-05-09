use crate::context::Context;
use crate::errors::{Error, Result};
use crate::namespace::propagate_labels;
use crate::project::Project;

use futures::StreamExt;
use kube::{
    api::ResourceExt,
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
};
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::time::Duration;
use tracing::{error, info};

lazy_static! {
    static ref RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);
}

/// Reconciliation loop of the Project controller.
async fn reconcile(project: Arc<Project>, ctx: Arc<Context>) -> Result<Action> {
    let ns = project.namespace().expect("Project is namespaced");
    info!(
        "Reconciling Project \"{:?}\" ({}) in {}",
        project.spec.display_name,
        project.name_any(),
        ns
    );
    if project.metadata.deletion_timestamp.is_some() {
        if let Err(e) = ctx.cache_delete_project(&project.name_unchecked()).await {
            error!(error =? e, project = project.name_unchecked(), "CACHE: cannot delete project");
        }

        // project has been deleted, nothing to do
        return Ok(Action::requeue(*RECONCILIATION_INTERVAL));
    }

    if let Err(e) = ctx
        .cache_update_project(
            project.name_unchecked().as_str(),
            &project.relevant_labels(),
        )
        .await
    {
        error!(error =? e, project = project.name_unchecked(), "CACHE: cannot update project");
    }

    let relevant_labels = project.relevant_labels();

    let namespaces = project.namespaces(ctx.local_client()).await?;
    for ns in namespaces {
        if let Err(e) = propagate_labels(&relevant_labels, &ns, ctx.local_client()).await {
            error!(error = ?e, namespace = ns.name_unchecked(), "Cannot propagate labels to namespace");
        }
    }

    // If no events were received, check back every 5 minutes
    Ok(Action::requeue(*RECONCILIATION_INTERVAL))
}

/// Error function called when the controller cannot run the reconciliation
/// loop
fn error_policy(project: Arc<Project>, error: &Error, ctx: Arc<Context>) -> Action {
    error!(
        project = ?project,
        is_downstream_cluster = ctx.is_downstream_cluster(),
        "reconcile failed: {error:?}");
    Action::requeue(*RECONCILIATION_INTERVAL)
}

/// Initialize the controller
pub async fn run(context: Arc<Context>) {
    let projects = context.projects_api();

    Controller::new(projects, watcher::Config::default().any_semantic())
        .shutdown_on_signal()
        .run(reconcile, error_policy, context)
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;
}
