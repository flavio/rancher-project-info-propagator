use crate::errors::{Error, Result};
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, ListParams, Patch, ResourceExt},
    client::Client,
    core::{params::PatchParams, ObjectMeta},
    CustomResource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::{debug, info};

pub const NAMESPACE_ANNOTATION: &str = "field.cattle.io/projectId";
const KEY_PROPAGATION_PREFIX: &str = "propagate.";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[cfg_attr(test, derive(Default))]
#[kube(
    kind = "Project",
    group = "management.cattle.io",
    version = "v3",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpec {
    // We don't really care about the contents of the Project.
    // So far we care only about its metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_name: Option<String>,
}

impl Project {
    /// Find all the Namespace that belong to the Project
    pub async fn namespaces(&self, client: Client) -> Result<Vec<Namespace>> {
        debug!(
            project = self.name_unchecked(),
            "finding list of namespaces that belong to project"
        );
        let namespaces: Api<Namespace> = Api::all(client);
        let lp = ListParams::default().labels(
            format!(
                "{}={}",
                NAMESPACE_ANNOTATION,
                self.metadata
                    .name
                    .clone()
                    .expect("project should always have a name")
            )
            .as_str(),
        );
        let expected_annotation = format!(
            "{}:{}",
            self.namespace()
                .expect("project should always have a namespace set"),
            self.name_unchecked()
        );

        namespaces
            .list(&lp)
            .await
            .map(|r| {
                r.items
                    .iter()
                    .filter(|ns| {
                        // the label doesn't include the cluster name,
                        // we have to filter by annotation
                        //
                        // We do a list filtered by label because labels are
                        // indexed inside of etcd, as opposed to annotations
                        ns.annotations().get(NAMESPACE_ANNOTATION) == Some(&expected_annotation)
                    })
                    .cloned()
                    .collect()
            })
            .map_err(Error::Kube)
    }

    pub async fn propagate_labels(&self, namespace: &Namespace, client: Client) -> Result<()> {
        if let Some(new_labels) = merge_labels(self.labels(), namespace.labels())? {
            let ns = Namespace {
                metadata: ObjectMeta {
                    labels: Some(new_labels),
                    ..ObjectMeta::default()
                },
                ..Namespace::default()
            };

            let patch = Patch::Apply(ns);
            let namespaces: Api<Namespace> = Api::all(client);
            let params = PatchParams::apply("racher-project-info-propagator").force();
            namespaces
                .patch(&namespace.name_unchecked(), &params, &patch)
                .await
                .map_err(Error::Kube)?;
            info!(namespace = namespace.name_unchecked(), "Labels propagated");
        };

        Ok(())
    }
}

fn merge_labels(
    project_labels: &BTreeMap<String, String>,
    namespace_labels: &BTreeMap<String, String>,
) -> Result<Option<BTreeMap<String, String>>> {
    let mut labels_changed = false;
    let mut namespace_labels = namespace_labels.clone();

    for (key, value) in project_labels.iter() {
        if key.starts_with(KEY_PROPAGATION_PREFIX) {
            let patched_key = key.strip_prefix(KEY_PROPAGATION_PREFIX).ok_or_else(|| {
                Error::Internal("strip prefix should always return something".to_string())
            })?;
            namespace_labels
                .entry(patched_key.to_owned())
                .and_modify(|v| {
                    if v != value {
                        *v = value.to_owned();
                        labels_changed = true;
                    }
                })
                .or_insert_with(|| {
                    labels_changed = true;
                    value.to_owned()
                });
        }
    }

    if labels_changed {
        Ok(Some(namespace_labels))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::json;

    #[rstest]
    #[case(
        // prj label is already defined inside of ns with the same value
        json!({
            "propagate.hello": "world",
            "foo": "bar",
        }),
        Some(json!({
            "hello": "world",
            "ciao": "mondo",
        })),
        None,
    )]
    #[case(
        // prj label is already defined inside of ns but with different value
        json!({
            "propagate.hello": "world",
            "foo": "bar",
        }),
        Some(json!({
            "hello": "world2",
            "ciao": "mondo",
        })),
        Some(json!({
            "hello": "world",
            "ciao": "mondo",
        })),
    )]
    #[case(
        // no labels to propagate from the prj
        json!({
            "foo": "bar",
        }),
        Some(json!({
            "ciao": "mondo",
        })),
        None,
    )]
    #[case(
        // label is missing from the ns
        json!({
            "propagate.hi": "world",
            "foo": "bar",
        }),
        None,
        Some(json!({
            "hi": "world",
        })),
    )]
    fn test_merge_labels(
        #[case] prj_labels: serde_json::Value,
        #[case] ns_labels: Option<serde_json::Value>,
        #[case] expected: Option<serde_json::Value>,
    ) {
        let project_labels: BTreeMap<String, String> =
            serde_json::from_value(prj_labels).expect("cannot deserialize project labels");

        let namespace_labels: BTreeMap<String, String> = ns_labels.map_or_else(
            || BTreeMap::new(),
            |labels| serde_json::from_value(labels).expect("cannot deserialize namespace labels"),
        );

        let expected_labels: Option<BTreeMap<String, String>> = expected.map(|labels| {
            serde_json::from_value(labels).expect("cannot deserialize expected labels")
        });

        let actual =
            merge_labels(&project_labels, &namespace_labels).expect("merge should not fail");

        assert_eq!(expected_labels, actual);
    }
}
