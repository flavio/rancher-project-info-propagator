use crate::errors::{Error, Result};
use k8s_openapi::api::core::v1::Namespace;
use kube::{
    api::{Api, Patch, ResourceExt},
    client::Client,
    core::{params::PatchParams, ObjectMeta},
};
use std::collections::BTreeMap;
use tracing::{debug, info};

/// Ensure the given `namespace` has the provided list of `relevant_labels`
/// set.
///
/// Note: the actual Kubernetes object is changed only when needed
pub async fn propagate_labels(
    relevant_labels: &BTreeMap<String, String>,
    namespace: &Namespace,
    client: Client,
) -> Result<()> {
    if let Some(new_labels) = merge_labels(relevant_labels, namespace.labels())? {
        debug!(
            namespace = namespace.name_unchecked(),
            labels =? new_labels,
            "namespace labels have to be updated"
        );
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
    } else {
        debug!(
            namespace = namespace.name_unchecked(),
            "namespace are already up to date"
        );
    }

    Ok(())
}

/// Compute the list of labels that have to be set.
///
/// Returns `Ok(None)` when no change is required
fn merge_labels(
    relevant_labels: &BTreeMap<String, String>,
    namespace_labels: &BTreeMap<String, String>,
) -> Result<Option<BTreeMap<String, String>>> {
    let mut labels_changed = false;
    let mut namespace_labels = namespace_labels.clone();

    for (key, value) in relevant_labels.iter() {
        namespace_labels
            .entry(key.to_owned())
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
            "hello": "world",
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
            "hello": "world",
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
        }),
        Some(json!({
            "ciao": "mondo",
        })),
        None,
    )]
    #[case(
        // label is missing from the ns
        json!({
            "hi": "world",
        }),
        None,
        Some(json!({
            "hi": "world",
        })),
    )]
    fn test_merge_labels(
        #[case] relevant_labels: serde_json::Value,
        #[case] namespace_labels: Option<serde_json::Value>,
        #[case] expected: Option<serde_json::Value>,
    ) {
        let project_labels: BTreeMap<String, String> =
            serde_json::from_value(relevant_labels).expect("cannot deserialize project labels");

        let namespace_labels: BTreeMap<String, String> = namespace_labels.map_or_else(
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
