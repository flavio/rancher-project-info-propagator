> **Warning:** alpha software. This is a POC

Rancher Manager has the concept of Project. A Project can hold many
regular Kubernetes Namespaces.

This controller ensures that certain labels defined on a Project are
propagated to all its "children" Namespace objects.

The labels defined on the Project have precedence over the ones defined
inside of the Namespace.

On the Project, only the labels that start with the `propagate.` prefix
are propagated to its Namespaces. The `propagate.` prefix is stripped when
the copy operation is performed.

## Deployment models

A single instance of Rancher Manager can be used to manage multiple
Kubernetes clusters.

The are two types of Kubernetes clusters:

* `upstream`: this is the cluster where Rancher Manager is deployed.
* `downstream`: this is one of the clusters that are managed by Rancher
  Manager.

The `Project` Custom Resource definition is currently defined only inside
of the upstream cluster. This influence the deployment strategies of this
controller.

### Deployment inside of the upstream cluster

In this scenario, the controller and Rancher Manager are deployed inside of
the same cluster.

The controller must be run using a Service Account that has the following
RBAC rules:

```yaml
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: rancher-project-info-propagator-cluster-role
rules:
- apiGroups: ["apiregistration.k8s.io"]
  resources: ["*"]
  verbs: ["get", "watch", "list"]
- apiGroups: [""]
  resources: ["namespaces"]
  verbs: ["get", "watch", "list", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: rancher-project-info-propagator-cluster-role-binding
subjects:
- kind: ServiceAccount
  name: rancher-project-info-propagator
  namespace: <namespace where the controller is deployed>
roleRef:
  kind: ClusterRole
  name: rancher-project-info-propagator-cluster-role
  apiGroup: rbac.authorization.k8s.io
```

### Deployment inside of the downstream cluster

In this scenario, the controller is deployed inside of a cluster that
is managed by Rancher Manager.

The Project resources are defined inside of a specific Namespace that
exists on the upstream cluster. The Namespace is named using the
cluster ID associated to the downstream cluster.
Because of that, the controller has to be able to connect to the
upstream cluster.

To keep things secure, inside of the upstream cluster we have to create
one dedicated Service Account per downstream cluster.

Assuming the ID of the downstream cluster is `c-m-jz8q2m87`, we will
create a dedicated Service Account:

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: rancher-project-info-propagator
  # Note: this is the namespace associated with this cluster
  namespace: c-m-jz8q2m87
---
# This secret will be populated by Kubernetes with an authentication
# token for the Service Account defined above.
# This token never expires, it's automatically removed by Kubernetes
# when the related Service Account is deleted.
apiVersion: v1
kind: Secret
metadata:
  name: rancher-project-info-propagator-secret
  namespace: c-m-jz8q2m87
  annotations:
    kubernetes.io/service-account.name: rancher-project-info-propagator
type: kubernetes.io/service-account-token
```

Next, inside of the upstream cluster, we create the following RBAC rules:

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  namespace: c-m-jz8q2m87
  name: project-reader
rules:
- apiGroups: ["management.cattle.io"]
  resources: ["projects"]
  verbs: ["get", "watch", "list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: read-projects
  namespace: c-m-jz8q2m87
subjects:
- kind: ServiceAccount
  name: rancher-project-info-propagator
  namespace: c-m-jz8q2m87
roleRef:
  kind: Role
  name: project-reader
  apiGroup: rbac.authorization.k8s.io
```

These rules allow the Service Account to read the Project objects
that are defined inside of a specific Namespace, the one associated
with the downstream cluster.

Note how Namespace access is not required inside of the upstream cluster,
that's because these objects are managed only inside of the downstream cluster.


Finally, we have to create a `kubeconfig` file that can be used to
connect to the upstream cluster as the Service Account user.
This process requires some manual work

First of all, we have to obtain the authentication token of the Service Account:

```console
kubectl get secret \
  -n c-m-jz8q2m87 \
  rancher-project-info-propagator-secret \
  -o go-template={{.data.token}} | base64 -d
```

Then, create a copy of the kubeconfig used to connect to the upstream
cluster and make the following changes:

```
apiVersion: v1
clusters:
- cluster:
    certificate-authority-data: LEAVE UNCHANGED
    server: https://KUBE_API_SERVER:6443
  name: upstream
contexts:
- context:
    cluster: upstream
    namespace: c-m-jz8q2m87 # USE THE RIGHT NAMESPACE
    user: rancher-project-info-propagator
  name: upstream
current-context: upstream
kind: Config
preferences: {}
users:
- name: rancher-project-info-propagator
  user:
    token: THE TOKEN OBTAINED BEFORE
```

**Warning:** the controller must connect using the "vanilla" Kubernetes
API, not the one exposed by Rancher Manager. That's because Rancher Manager
Kubernetes API doesn't allow Service Accounts to access the `management.cattle.io`
API.

You can check the correctness of this kubeconfig by using the following
command:

```console
export KUBECONFIG=sa-kubeconfig.yaml kubectl \
  get \
  -n c-m-jz8q2m87 \
  projects.management.cattle.io p-q9rcp 
```

This should return a list of all the Project that belong to this downstream cluster.

Inside of the downstream cluster start by creating a dedicated Namespace where the controller is going to be deployed.
A dedicated Service Account must be created Inside of this Namespace.
Create a Secret that contains the `kubeconfig` created previously.

Finally, the following RBAC rules have to be defined:

```yaml
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: rancher-project-info-propagator-cluster-role
rules:
- apiGroups: [""]
  resources: ["namespaces"]
  verbs: ["get", "watch", "list", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: rancher-project-info-propagator-cluster-role-binding
subjects:
- kind: ServiceAccount
  name: rancher-project-info-propagator
  namespace: <namespace where the controller is deployed>
roleRef:
  kind: ClusterRole
  name: rancher-project-info-propagator-cluster-role
  apiGroup: rbac.authorization.k8s.io
```

Finally, the controller must be deployed. The secret containing the
kubeconfig file must be mounted inside of the container.

The container must run with the following environment variable set:

* `PROPAGATOR_KUBECONFIG_UPSTREAM`: path to the kubeconfig file used to
  connect to the upstream cluster
* `PROPAGATOR_CLUSTER_ID`: id of the cluster

## Downstream cluster and caching

When deployed inside of the downstream cluster, the controller maintains a cache
of the Project objects defined upstream (obviously the ones that are related token
the downstream cluster) and the relevant labels that are defined by them.

This cache is used to reconcile changes done to the Namespace objects when the
connection towards the upstream cluster is broken.

The cache is kept inside of a sqlite file. The file can be stored inside of a PersistentVolume or
inside of an [`emptyDir`](https://kubernetes.io/docs/concepts/storage/volumes/#emptydir).

## Missing items

This is a POC, some changes have still to be done, these are the major ones:

* [Create helm chart](https://github.com/flavio/rancher-project-info-propagator/issues/1)
* [GitHub action](https://github.com/flavio/rancher-project-info-propagator/issues/2):
  introuduce more automation and publish the container image to ghcr.io
* [Replace Namespace controller with Kubewarden policy](https://github.com/flavio/rancher-project-info-propagator/issues/3)



