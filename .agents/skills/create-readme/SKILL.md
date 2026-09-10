---
name: create-readme
description: Create or update repository, module, package, or directory READMEs grounded in code, tests, and configuration. Use when the user asks to write README documentation for users and maintainers, including architecture and control-flow explanations with Mermaid diagrams where useful.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.

# Create README

Write README documentation based on the actual implementation, tests, configuration, and existing docs. Explain how to
use and maintain the target, with detail proportional to its complexity and the user's requested scope.

## Workflow

1. Identify the requested target, output path, and applicable `AGENTS.md` instructions. If the target is ambiguous,
   inspect likely paths before asking for clarification. Resolve module names to actual files; a Rust module may be a
   single `.rs` file rather than a directory. Use the containing directory's README or existing documentation coverage
   unless the user specifies another location.
2. Establish any requested change scope before choosing edits:
    - Between refs: inspect `git diff --name-status <base>...<head>` and the relevant hunks for branch changes since the
      merge base; use `<base>..<head>` for a direct comparison of the two trees.
    - Explicit commits: inspect each with `git show --name-status --patch <commit>`.
    - Since the README was last updated: find its last commit with `git log -1 --format=%H -- <README>`, then inspect
      `git diff <commit>..HEAD -- <target>`. If it has no history, inspect the current implementation directly.
3. Read the implementation thoroughly enough to explain responsibilities, public interfaces, data flow, side effects,
   and maintenance concerns. Follow callers, related modules, tests, examples, and build or deployment files where they
   clarify behavior. For historical documentation, validate against the requested revision; otherwise use current code.
4. For scoped updates, extract changed identifiers, configuration fields, and behaviors from the diff. Search the
   complete target README and relevant ancestor or linked documentation for affected usage examples, lifecycle
   descriptions, and summaries. Include related files outside the target when needed to verify behavior, but keep
   documentation edits tied to the requested scope.
5. Draft or update the README, preserving accurate existing content and the user's chosen structure.
6. Verify claims, examples, commands, and links against their sources. Report validation performed and any unresolved
   discrepancies.

## Investigation Guidance

- Use `rg --files <target>` to map the directory and `rg` to find definitions, callers, configuration keys, and tests.
  Include hidden paths explicitly when inspecting CI or repository instructions.
- Read package metadata, toolchain configuration, build scripts, and CI before documenting development commands. For
  Rust, inspect `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, module declarations, public items and re-exports,
  serialization attributes, and inline `#[cfg(test)]` modules.
- Trace behavior from entrypoints through transformations to side effects, including error and cleanup paths.
  Distinguish public interfaces from internal helpers and implemented behavior from comments or TODOs.
- For Kubernetes behavior, inspect API types, schemas, reconciliation, watches, finalizers, ownership, RBAC manifests,
  and deployment configuration as relevant. Distinguish controller logic from API-server validation and permissions
  required from permissions actually supplied by the deployment.
- Compare generated artifacts with their generator and checked-in versions when they affect the documentation. If they
  disagree, explain which behavior each source establishes and report the discrepancy; do not silently choose one as
  authoritative or regenerate tracked files merely to write a README.
- Verify dependency-provided flags and runtime behavior against the resolved dependency source or executable help. Do
  not turn a build-time API feature selection or pinned toolchain into a claim about minimum supported runtime versions
  without supporting evidence.

### Sinker Source Pointers

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

### Development and Generation Commands

Use commands appropriate to the documented target and verify them against the current CI workflow. This repository uses
`cargo build`, `cargo fmt`, `cargo test`, and `cargo clippy --all-targets --all-features`. Tests are inline in the Rust
modules. Cargo test filters match test names, not filesystem paths; if documenting a narrower command, check the
selected tests with `cargo test <filter> -- --list`.

The `manifests` subcommand in `src/main.rs` emits CRDs. CI runs `cargo run -- manifests > manifests/crd.yml` and checks
for drift. When verifying documentation, direct generated output to a temporary file for comparison so checked-in schema
changes are preserved. Keep CRD generation distinct from rendering the complete deployment through
`manifests/kustomization.yaml`.

## README Content

Include sections that fit the target. Do not force every section or expand a focused module README into a full
deployment guide.

- Purpose and scope: what the target does, who uses it, and what is handled elsewhere.
- Architecture and control flow: the main components, interfaces, and paths through execution.
- Usage: public APIs, commands, configuration, examples, and integration points.
- Operations: runtime assumptions, observability, failure modes, lifecycle behavior, and dependencies.
- Development and maintenance: how to test, generate, validate, or extend the target; invariants, source-of-truth files,
  and coupling to other modules.

## Mermaid Diagrams

Include an embedded Mermaid diagram when it clarifies non-trivial control flow, reconciliation, build or generation
pipelines, data movement, or resource ownership. Keep diagrams simple enough to maintain and include only relationships
supported by the implementation.

Use `flowchart TD` for most control flow and resource relationships. Use `sequenceDiagram` when ordering between actors
is central to understanding behavior.

## Writing and Verification

- Be detailed but concise. Separate usage from implementation details when that helps the reader.
- Name real files, types, commands, resources, and configuration fields. Use serialized names in configuration examples
  and Rust identifiers when discussing code.
- Link to source files and related documentation using paths relative to the README's location.
- Treat existing docs and example fixtures as evidence to check, not proof that a behavior or deployment is supported.
  Mark consequential unknowns explicitly.
- Reread prose, tables, examples, and diagrams together against the implementation so a scoped update leaves no
  contradictory descriptions.
- Run checks that substantiate the documentation changes; distinguish commands verified by inspection from checks
  actually executed. Report implementation or manifest discrepancies without expanding a documentation task into
  unrelated repairs.
