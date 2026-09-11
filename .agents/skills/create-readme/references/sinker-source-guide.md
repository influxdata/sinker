# Sinker Source Guide

Read this reference when documenting Sinker implementation, development, or CRD generation.

## Source Pointers

Sinker is a single Rust package with a controller binary and library modules. Use these pointers only when relevant to
the requested documentation; they are starting points for investigation, not a required README outline. Paths are
relative to the repository root.

- `src/main.rs` and `src/lib.rs`: CLI entrypoint, runtime setup, module visibility, and shared errors. Client and admin
  arguments are flattened from `kubert`, so their full interface is not declared locally.
- `src/resources.rs` and `manifests/crd.yml`: `ResourceSync` and `SinkerContainer` schemas, serialized field names,
  defaults, and validation. `SinkerContainer` has a manually supplied schema; inspect that as well as the Rust types.
  Check examples in `example.yaml` against these sources and runtime handling.
- `src/controller.rs`, `src/remote_watcher.rs`, `src/remote_watcher_manager.rs`, and `src/filters.rs`: reconciliation
  triggers, target application, status, deletion, watcher lifecycle, and filtering of self-generated events. Follow both
  reconciliation and watch paths before describing retries, drift correction, or cleanup guarantees.
- `src/resource_extensions.rs`: client selection, resource discovery, namespace resolution, and access checks for
  kubeconfig Secrets. Distinguish the namespace holding credentials from the source or target resource namespace, and
  check local, remote, and cluster-scoped cases when documenting references.
- `src/mapping.rs` and its tests: whole-resource copying, field selection, target construction, and metadata handling.
  Source selectors and destination paths use different parsing logic; verify their syntax and missing-value behavior
  separately.
- `manifests/`, `Dockerfile`, and `.github/workflows/rust.yml`: deployment, RBAC, container packaging, and build or
  publication commands. Derive operational examples from these files and identify placeholders or environment-specific
  values.

## Development and Generation Commands

Use commands appropriate to the documented target and verify them against the current CI workflow. This repository uses
`cargo build`, `cargo fmt`, `cargo test`, and `cargo clippy --all-targets --all-features`. Tests are inline in the Rust
modules. Cargo test filters match test names, not filesystem paths; if documenting a narrower command, check the
selected tests with `cargo test <filter> -- --list`.

The `manifests` subcommand in `src/main.rs` emits CRDs. CI runs `cargo run -- manifests > manifests/crd.yml` and checks
for drift. When verifying documentation, direct generated output to a temporary file for comparison so checked-in schema
changes are preserved. Keep CRD generation distinct from rendering the complete deployment through
`manifests/kustomization.yaml`.
