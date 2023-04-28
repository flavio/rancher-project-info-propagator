use crate::errors::{Error, Result};
use crate::project::Project;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, ListParams, ResourceExt},
    client::Client,
    runtime::{
        controller::{Action, Controller},
        reflector::ObjectRef,
        watcher,
    },
};
use std::sync::Arc;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

// Context for our reconciler
#[derive(Clone)]
pub struct Context {
    /// Kubernetes client
    pub client: Client,
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
        return Ok(Action::requeue(Duration::from_secs(5 * 60)));
    }

    let namespaces = project.namespaces(ctx.client.clone()).await?;
    println!(
        "Project {}/{} has these children:",
        project.namespace().unwrap(),
        project.name_unchecked(),
    );
    if namespaces.is_empty() {
        println!("none");
    }
    for ns in namespaces {
        println!("- {}", ns.name_unchecked());
        if let Err(e) = project.propagate_labels(&ns, ctx.client.clone()).await {
            error!(error = ?e, namespace = ns.name_unchecked(), "Cannot propagate labels to namespace");
        }
    }

    // If no events were received, check back every 5 minutes
    Ok(Action::requeue(Duration::from_secs(5 * 60)))
}

fn error_policy(project: Arc<Project>, error: &Error, ctx: Arc<Context>) -> Action {
    warn!("reconcile failed: {:?}", error);
    Action::requeue(Duration::from_secs(5 * 60))
}

/// Initialize the controller and shared state (given the crd is installed)
pub async fn run() {
    let client = Client::try_default()
        .await
        .expect("failed to create kube Client");
    let projects = Api::<Project>::all(client.clone());
    if let Err(e) = projects.list(&ListParams::default().limit(1)).await {
        error!("CRD is not queryable; {e:?}. Is the CRD installed?");
        std::process::exit(1);
    }
    let context = Context {
        client: client.clone(),
    };

    Controller::new(projects, watcher::Config::default().any_semantic())
        .watches(
            Api::<Namespace>::all(client),
            watcher::Config::default().any_semantic(),
            |ns| {
                if let Some(project_annotation) =
                    ns.annotations().get(crate::project::NAMESPACE_ANNOTATION)
                {
                    if let Some((prj_ns, prj_name)) = parse_project_annotation(project_annotation) {
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
}

fn parse_project_annotation(annotation: &String) -> Option<(&str, &str)> {
    annotation.split_once(':')
}
