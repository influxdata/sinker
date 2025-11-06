# Sinker

Sinker is a Kubernetes controller that keeps resources in sync across clusters. It watches `ResourceSync` custom
resources (CRs), reads a source object, then projects it onto a target object—optionally in a different cluster—while
preserving the fields you care about. Sinker keeps watching both ends so that drifts introduced by other actors are
reconciled automatically.

## Features

- Cross-cluster or in-cluster synchronization for any Kubernetes API resource using a single declarative CR.
- JSONPath-based field mappings that let you clone entire objects or copy specific subtrees into the target.
- Safe lifecycle management through finalizers plus explicit per-resource annotations to control deletion behavior
  during outages.
- Remote watchers that observe both source and target objects and trigger reconciliations whenever something changes.
- Admin HTTP server exposing readiness and liveness probes (and, via the `kubert` runtime, metrics) so the controller
  integrates cleanly with cluster operations tooling.

## Getting Started

### Prerequisites

- Rust toolchain 1.85 or later (see `rust-toolchain.toml`) if you plan to build from source.
- Docker or another OCI-compatible builder to produce a controller image.
- Access to at least one Kubernetes cluster (1.33+) with `kubectl` and the ability to create CRDs, roles, and service
  accounts.
- Optional: access credentials for any remote clusters you want Sinker to read from or write to. These must be stored as
  Kubernetes secrets containing kubeconfigs such as those created by [CAPI](https://cluster-api.sigs.k8s.io/).

### Build the controller

```bash
git clone https://github.com/influxdata/sinker.git
cd sinker
cargo build --release
```

The compiled binary lives at `target/release/sinker`. To build a container image:

```bash
docker build -t <registry>/<repo>/sinker:<tag> .
docker push <registry>/<repo>/sinker:<tag>
```

### Deploy to Kubernetes

1. **Create an image pull secret** (if needed) in the namespace where Sinker will run, for example:
   ```bash
   kubectl create secret docker-registry gar-auth-sinker \
     --docker-server=<registry> \
     --docker-username=<username> \
     --docker-password=<password> \
     --namespace sinker
   ```
2. **Update the controller image** in `manifests/deployment.yml` (or overlay with Kustomize) to point at the image you
   pushed.
3. **Apply the bundled manifests** (CRDs, RBAC, Deployment, ServiceAccount, etc.):
   ```bash
   kubectl apply -k manifests
   ```
4. **Verify deployment**:
   ```bash
   kubectl -n sinker get pods
   kubectl -n sinker logs deploy/sinker
   ```
5. When the pod is ready, the controller listens on port `8080` for `/live`, `/ready`, and `/metrics` (Prometheus
   format) endpoints exposed by the admin server.

### Running locally against a cluster

You can also run the controller directly from your workstation:

```bash
cargo run -- --kubeconfig /path/to/kubeconfig --context my-context
```

Key CLI flags:

- `--log-level` (or `SINKER_LOG`) controls tracing filters (default `sinker=info,warn`).
- `--log-format` selects `plain` or `json`.
- `--kubeconfig`, `--context`, `--cluster`, and `--user` mirror the standard `kubectl` flags for choosing credentials.
- `--as` and `--as-group` let you impersonate another Kubernetes user or group.
- `--kube-api-response-headers-timeout` configures the Kubernetes client timeout (default `9s`).
- `--admin-addr` sets the admin HTTP server bind address (default `0.0.0.0:8080`).

## Defining resource syncs

Sinker ships two custom resources:

### ResourceSync

`ResourceSync` objects describe how to mirror a Kubernetes resource.

| Field                     | Required | Description                                                                                                                         |
|---------------------------|----------|-------------------------------------------------------------------------------------------------------------------------------------|
| `spec.source.resourceRef` | ✓        | API reference (`apiVersion`, `kind`, `name`) pointing to the object you want to copy.                                               |
| `spec.source.cluster`     |          | Optional remote cluster reference. When omitted, Sinker reads the source from the same cluster and namespace as the `ResourceSync`. |
| `spec.target.resourceRef` | ✓        | API reference for the target object.                                                                                                |
| `spec.target.cluster`     |          | Optional remote cluster reference. When omitted, the target lives in the same cluster as the `ResourceSync`.                        |
| `spec.mappings[]`         |          | Optional list of field mapping rules. See below.                                                                                    |

#### Cluster references

When you provide `spec.{source|target}.cluster`, Sinker reads a kubeconfig from a secret to talk to the remote cluster:

```yaml
cluster:
  namespace: other-namespace    # optional override; defaults to the Remote kubeconfig's namespace
  kubeConfig:
    secretRef:
      name: remote-kubeconfig
      key: value
```

Create the secret by embedding a standard kubeconfig:

```bash
kubectl -n <ns> create secret generic remote-kubeconfig \
  --from-file=value=/path/to/kubeconfig
```

Within the remote cluster, RBAC for the kubeconfig user limits what Sinker can access. For local clusters, Sinker uses
its in-cluster credentials.

#### Mappings

- When `spec.mappings` is empty, Sinker clones the entire source object, copying annotations and labels while cleaning
  the `kubectl.kubernetes.io/last-applied-configuration` annotation.
- When mappings are supplied, each mapping entry has `fromFieldPath` and/or `toFieldPath`:
    - `fromFieldPath` is a JSONPath evaluated against the source. Use `spec.data.someField` to copy a nested field, or
      leave it blank/omit it to select the entire object.
    - `toFieldPath` is a JSONPath-like dotted path inside the target (`metadata.*` and `spec|status|data` are
      supported). Omit `toFieldPath` to replace the entire target with the selected subtree.
- Sinker enforces that at least one of the fields is present per mapping. If `toFieldPath` targets `metadata`, the
  controller keeps metadata in sync using server-side apply.

Example `ResourceSync`:

```yaml
apiVersion: sinker.influxdata.io/v1alpha1
kind: ResourceSync
metadata:
  name: demo
  namespace: default
spec:
  source:
    resourceRef:
      apiVersion: v1
      kind: ConfigMap
      name: remote-demo
    cluster:
      namespace: default
      kubeConfig:
        secretRef:
          name: k3-test-27-kubeconfig
          key: value
  target:
    resourceRef:
      apiVersion: v1
      kind: ConfigMap
      name: demo
  mappings:
    - fromFieldPath: data.remote
      toFieldPath: data.remote
    - fromFieldPath: data.foo
      toFieldPath: data.bar
```

### SinkerContainer

`SinkerContainer` is a lightweight CRD that stores arbitrary structured data under `.spec`. Use it as either a source or
target when you want Sinker to materialize generated configuration into a typed CR (for example, to copy an inner spec
from one object into another).

## Annotations and finalizers

Sinker adds a finalizer (`sinker.influxdata.io/target`) to each `ResourceSync` so it can clean up target objects when
the CR is deleted. Two optional annotations modify that behavior:

- `sinker.influxdata.io/force-delete: "true"` – if set and the controller cannot contact a remote cluster while
  deleting, Sinker removes its finalizer so the `ResourceSync` can be garbage-collected.
- `sinker.influxdata.io/disable-target-deletion: "true"` – skip deleting the target object when the `ResourceSync` is
  removed. Useful when you want to manage the target lifecycle manually.

## Status and observability

- Sinker publishes a `ResourceSyncFailing` condition in `.status.conditions[]` with timestamps, reasons, and messages if
  reconciliation fails.
- Logs use the standard `tracing` crate; adjust verbosity with `SINKER_LOG`.
- The admin server exposes `/live`, `/ready`, and `/metrics` on port 8080. Wire these into your cluster’s probes and
  monitoring.
- Remote watchers reconcile whenever the source or target resource changes, even on remote clusters, which keeps
  synchronized objects fresh with minimal polling.

## Generating CRDs programmatically

To print the latest CRDs (e.g., for Helm packaging), use the built-in command:

```bash
cargo run -- manifests > out.yml
```

The output includes both `ResourceSync` and `SinkerContainer` definitions.

## Development notes

- Format and lint with `cargo fmt` and `cargo clippy --all-targets --all-features`.
- Run tests with `cargo test`.
- When developing new features, update `manifests` or regenerate CRDs via `sinker manifests` before deploying.
- The project uses MIT licensing; see `LICENSE` for details.
