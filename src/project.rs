use crate::errors::{Error, Result};
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, ListParams, ResourceExt},
    client::Client,
    CustomResource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::debug;

pub const NAMESPACE_ANNOTATION: &str = "field.cattle.io/projectId";
const KEY_PROPAGATION_PREFIX: &str = "propagate.";

/// Stripped down `Spec` of Rancher Project objects. Only the relevant
/// fields are defined.
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

    /// List of labels that have to be propagated to all the Namespace that
    /// belong to the Project.
    ///
    /// Note: the label keys are stripped of the `propagate.` prefix
    pub fn relevant_labels(&self) -> BTreeMap<String, String> {
        self.labels()
            .iter()
            .filter_map(|(k, v)| {
                if k.starts_with(KEY_PROPAGATION_PREFIX) {
                    let patched_key = k
                        .strip_prefix(KEY_PROPAGATION_PREFIX)
                        .expect("stripping the prefix should never fail");
                    Some((patched_key.to_string(), v.to_owned()))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use rstest::*;
    use serde_json::json;

    #[rstest]
    #[case(
        json!({
            "propagate.hello": "world",
            "foo": "bar",
        }),
        json!({
            "hello": "world",
        }),
    )]
    #[case(
        // prj label is already defined inside of ns with the same value
        json!({
            "foo": "bar",
        }),
        json!({
        }),
    )]
    #[case(
        // prj label is already defined inside of ns with the same value
        json!({
        }),
        json!({
        }),
    )]
    fn test_relevant_labels(
        #[case] prj_labels: serde_json::Value,
        #[case] expected_labels: serde_json::Value,
    ) {
        let project_labels: BTreeMap<String, String> =
            serde_json::from_value(prj_labels).expect("cannot deserialize project labels");

        let expected_labels: BTreeMap<String, String> =
            serde_json::from_value(expected_labels).expect("cannot deserialize expected labels");

        let project = Project {
            metadata: ObjectMeta {
                labels: Some(project_labels),
                ..Default::default()
            },
            spec: ProjectSpec {
                ..Default::default()
            },
        };

        let actual_labels = project.relevant_labels();
        assert_eq!(actual_labels, expected_labels);
    }
}
