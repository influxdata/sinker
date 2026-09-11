# Working on Sinker

This file applies throughout the repository. Sinker is a single Rust package with a binary and library, implementing
one-way Kubernetes resource synchronization within or between clusters. `ResourceSync` drives reconciliation;
`SinkerContainer` supplies an arbitrary-object schema and has no separate controller.

Start with [README.md](README.md) for project context. Use its [source map](README.md#development) to locate the relevant
modules and its [control-flow diagram](README.md#how-it-works) when changing reconciliation or watches. Read the linked
task-specific sections below as needed.

## Development and verification

Use the toolchain in [rust-toolchain.toml](rust-toolchain.toml) and the dependency versions resolved in
[Cargo.lock](Cargo.lock). [Cargo.toml](Cargo.toml) and [rustfmt.toml](rustfmt.toml) select Rust 2021. When changing the
toolchain or dependencies, also check the Rust builder image in [Dockerfile](Dockerfile). The `k8s-openapi` feature
selects build-time API bindings; it does not establish a tested minimum Kubernetes server version.

Run these checks from the repository root for Rust changes, matching [CI](.github/workflows/rust.yml) while keeping
format validation read-only and dependency resolution locked:

```bash
cargo build --locked
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features
```

Tests live in inline `#[cfg(test)]` modules and use ordinary Rust tests, `rstest`, and Tokio tests. Add focused regression
coverage for behavior changes beside the affected implementation. Cargo test filters match test names, not file paths;
use `cargo test --locked <filter> -- --list` to check selection. These tests exercise local logic, not live Kubernetes
reconciliation, authorization, garbage collection, or server-side apply.

For API/schema changes, also perform the CRD comparison described below. For deployment changes, render with
`kubectl kustomize manifests`. For documentation-only changes, verify claims, relative links, and section anchors;
execute examples only when needed to substantiate them. Report checks actually run and distinguish existing failures
from regressions introduced by the change.

## Code style

Keep project code as DRY (Don't Repeat Yourself) as reasonably possible. Reuse existing helpers and consolidate repeated
logic when doing so improves clarity and maintainability. Avoid abstractions that obscure meaningful differences or add
unnecessary complexity, and keep refactoring focused on the requested task.

Preserve existing behaviors and contracts when changing code, including during refactoring and deduplication. Change
them only when the user explicitly requests it or there is no other reasonable way to complete the requested task. In
the latter case, keep the change minimal and explain why it is necessary and which behaviors or contracts it affects.

## Implementation constraints

- **API and serialization:** Update [resources.rs](src/resources.rs), [manifests/crd.yml](manifests/crd.yml), and affected
  [README examples](README.md#defining-resource-syncs) together when changing the public API. Serialized fields use
  `camelCase`. `SinkerContainer` uses `crd_with_manual_schema()` to preserve arbitrary fields under `.spec`; its empty
  Rust spec type does not describe the stored payload. Check schema validation separately from runtime validation.
- **Mappings:** In [mapping.rs](src/mapping.rs), source selection uses JSONPath, while destination construction uses
  dotted object keys. Preserve the distinction between an empty mappings list (whole-resource copy) and an empty
  mapping entry (error), ordered mappings, and special handling of `DynamicObject` metadata. Read
  [Mappings](README.md#mappings) before changing path parsing, subtree replacement, or cloning behavior.
- **Client and namespace resolution:** In [resource_extensions.rs](src/resource_extensions.rs), kubeconfig Secrets are
  read from the controller's cluster. Their namespace is distinct from the source/target resource namespace. Preserve
  cross-namespace Secret authorization through `sinker.influxdata.io/allowed-namespaces`: absent, invalid, or
  nonmatching expressions deny access. Same-namespace references bypass this annotation check. Read
  [namespace resolution](README.md#cluster-references-and-namespaces) and [permissions](README.md#permissions) when
  changing this path; local operations use the controller's identity, not the `ResourceSync` creator's permissions.
- **Target writes and event filtering:** [controller.rs](src/controller.rs) uses forced server-side apply with field
  manager `sinker.influxdata.io`. [filters.rs](src/filters.rs) and [remote_watcher.rs](src/remote_watcher.rs) use that same
  manager to suppress self-generated events; unknown ownership triggers reconciliation. Review these paths together
  when changing field ownership or event handling.
- **Watch lifecycle:** [remote_watcher_manager.rs](src/remote_watcher_manager.rs) keys watches by resource reference
  and owning sync, for both local and remote resources. Preserve watcher cancellation and joining during cleanup and
  shutdown. The main `ResourceSync` stream uses kube's generation predicate with UID-aware caching and a 24-hour idle
  TTL. Kubeconfig Secret changes are not explicit triggers. Read [event and retry behavior](README.md#status-and-observability)
  before changing configuration refresh or reconciliation scheduling.
- **Status:** In [controller.rs](src/controller.rs), compute `ResourceSyncFailing` from live status, not the reflector
  cache. Success clears failure, unchanged condition values retain `lastTransitionTime`, and successful deletion
  cleanup skips the status write because the object may already be gone. Preserve the regression coverage for these
  transitions when changing reconciliation results or status updates.
- **Deletion and ownership:** Read [Annotations and finalizers](README.md#annotations-and-finalizers) before changing
  cleanup. Preserve unrelated finalizers. Normal cleanup waits for target absence before stopping watches and removing
  Sinker's finalizer. Both APIs are resolved before cleanup; `force-delete` only bypasses failures at that resolution
  step. Local targets also have an owner reference, so `disable-target-deletion` alone does not retain them. Account for
  the documented local cluster-scoped ownership limitation when changing target scope or retention behavior.

## CRD generation

Follow [Generating CRDs](README.md#generating-crds): generate into a temporary file and compare before replacing
[manifests/crd.yml](manifests/crd.yml). The `manifests` subcommand emits only the two CRDs and needs no cluster connection;
Kustomize renders the complete deployment bundle.

Preserve schema constraints, defaults, and serialized fields when reviewing generated differences. Establish drift
from the current generated and checked-in schemas, and report differences outside the task's scope. See
[ResourceSync](README.md#resourcesync) for the bundled schema's validation behavior.

## Runtime and deployment changes

Use [main.rs](src/main.rs) for runtime wiring and `cargo run --locked -- --help` to verify CLI flags; client and admin
arguments come from `kubert`. Consult [observability](README.md#status-and-observability) when changing admin endpoints
or logging. Source objects and mapped values can contain Secret data; avoid adding payloads to routine logs.

For live testing, follow [Running locally against a cluster](README.md#running-locally-against-a-cluster) with an explicit
kubeconfig/context and a cluster without another active Sinker controller. Running the binary without a subcommand
starts reconciliation. The controller watches all namespaces and has no leader election.

When changing supported operations or kinds, review [RBAC](manifests/clusterrole.yml) alongside the code; discovery does
not grant permissions. For packaging, review [deployment defaults](README.md#deploy-to-kubernetes),
[Dockerfile](Dockerfile), and the [publication workflow](.github/workflows/rust.yml). The bundled image tag and pull
Secret require environment-specific configuration; [example.yaml](example.yaml) also requires external resources.

## Documentation and self-improvement

Use [create-readme](.agents/skills/create-readme/SKILL.md) for README and agent-documentation changes. Keep this file
focused on actionable project guidance and link to existing README sections for detailed usage and operations.

After completing a task governed by this file, use
[improving-skills](.agents/skills/improving-skills/SKILL.md) to review the skills and AGENTS.md instructions used and
capture concrete feedback. Apply improvements within the authorized scope; propose changes outside it. Combine feedback
into one pass, including improving-skills' self-review, without recursively invoking completion hooks.
