---
name: comment-code-diff
description: Add thorough, explicit, concise Rust documentation and inline comments, plus comments in canonical YAML manifests, for a requested commit, branch diff, file, Rust module, directory, crate, or manifest tree. Use for documentation-focused passes that explain behavior, API contracts, and operational intent while preserving runtime behavior and respecting generated-file boundaries.
---

> **After completing tasks with this skill:** Invoke [improving-skills](../improving-skills/SKILL.md) to capture feedback and lessons learned.

# Comment Code Diff

Make a documentation-focused pass over Rust code and handwritten YAML manifests in the requested scope. Explain
non-obvious behavior, contracts, and operational intent for maintainers and code users. Be thorough where context matters,
but keep comments concise and close to the code they explain; avoid narration that repeats the implementation.

Run commands and resolve unlinked source paths from the repository root. Markdown links are relative to this skill file.

## Workflow

1. Read applicable repository guidance and establish the working tree state.
   - Start with the root [AGENTS.md](../../../AGENTS.md) and [README.md](../../../README.md), then any nested instructions
     that apply to the requested files. Use the README's [source map](../../../README.md#development) to locate related
     implementation and its task-specific sections when documenting reconciliation, mappings, credentials, or watches.
   - Run `git status --short`, `git diff`, and `git diff --cached`. Preserve existing edits and inspect relevant untracked
     files separately. Keep working-tree changes distinct from a requested historical comparison.

2. Resolve the requested scope before choosing files to edit.
   - For one commit, including "the most recent commit," resolve its immutable hash and inspect only that commit with
     `git show <commit>`. Do not substitute a branch comparison or include neighboring commits.
   - For a branch or commit range, preserve the complete comparison in every range command, including `..` versus `...`
     (for example, `origin/main...HEAD`). For "this branch," "the diff," or an unspecified branch comparison, default to
     `origin/main...HEAD` unless the conversation identifies working-tree changes. Verify the base exists; do not silently
     substitute another base if it is unavailable.
   - For multiple named commits without an explicit range, inspect each with `git show` or `git diff-tree` and use the
     union of their changed files. Do not implicitly include intervening commits.
   - For explicit files, edit only the named files. Related callers, tests, and docs may be read to verify intent.
   - For a module, directory, crate, or manifest tree, resolve its actual files before enumerating them. Sinker has one
     Cargo package at the repository root, with binary and library entrypoints. A Rust module may be one `.rs` file;
     follow declarations in [lib.rs](../../../src/lib.rs) and the named module rather than assuming a module directory.
   - Intersect combined scopes: "changes in `src/controller.rs` since main" means the requested comparison filtered to
     that path. Do not add working-tree changes to a commit scope unless the user includes them.

3. Identify eligible files and read the relevant implementation.
   - For comparisons, start with `git diff --stat <comparison>` and `git diff --name-only <comparison>`, adding a path
     filter when requested. Read hunks with `git diff --unified=80 <comparison> -- <path>`.
   - For file scopes, read the named files directly. For directory or crate scopes, use `rg --files <scope>` to enumerate
     Rust files and canonical YAML. Include hidden directories explicitly when they are requested.
   - Skip generated output, vendored files, dependency metadata, and unrelated docs as edit targets unless explicitly
     requested. In Sinker, `manifests/crd.yml` is generated from Rust resource definitions with a checked-in validation
     addition; it is not an ordinary YAML comment target. Follow [Derived documentation](#derived-documentation) for
     intentional source documentation changes that affect it.
   - Handwritten deployment and RBAC files under [manifests/](../../../manifests/) and [example.yaml](../../../example.yaml)
     are eligible when in scope. Establish provenance from generators, CI, and local guidance; a YAML extension or missing
     generated header alone does not establish that a file is handwritten.
   - Read callers, inline `#[cfg(test)]` modules, and related documentation as needed to verify contracts and edge cases.
     Trace implementation through error and cleanup paths; do not treat existing comments, TODOs, or fixtures as proof.
     Report verified drift in documentation outside the edit scope.
   - For large diffs, prioritize handwritten production code before tests and tooling. Use `rg` to find related symbols
     and existing comment style.

4. Write comments that add useful context.
   - Use [Rust comment guidance](#rust-comment-guidance) for API contracts and implementation intent, and
     [YAML comment guidance](#yaml-comment-guidance) for operational explanations.
   - Explain invariants, lifecycle ordering, ownership, authorization, defaults, retries, data sensitivity, and deliberate
     omissions when they matter to the scoped code. Follow the relevant constraints in
     [AGENTS.md](../../../AGENTS.md#implementation-constraints) instead of reproducing the entire controller design.
   - Express review-relevant context as durable maintainer-facing intent. Do not mention PRs, reviewers, or the
     comment-writing task in committed comments.
   - Correct misleading comments when behavior is verified. Remove mechanical comments such as "construct request" or
     "return error" if there is no additional intent to explain.

5. Preserve behavior and scope, then verify the pass.
   - Keep signatures, visibility, executable statements, identifiers, error messages, logging, serialization, resource
     identities, CLI parsing, and reconciliation semantics unchanged. Preserve non-documentation attributes, including
     `serde`, `schemars`, `kube`, `clap`/`command`/`arg`, `cfg`, derive, and lint attributes.
   - String and raw-string contents are data even when they contain YAML or look like comments. Do not edit the
     `MANUAL_SCHEMA` string or manifest values under comment-only authority.
   - Rust doc comments can affect generated public output. Apply [Derived documentation](#derived-documentation) before
     editing comments consumed by macros; ordinary `//` comments are suitable for maintainer-only context.
   - Avoid refactors, test additions unrelated to documentation, and incidental formatting changes. Restore only changes
     introduced by the pass that fall outside its scope; preserve pre-existing work.

## Rust Comment Guidance

- Use `///` for item documentation and `//!` for crate or module documentation. Describe the purpose in a short opening
  sentence, then add detail needed by callers. Rustdoc prose need not start with the identifier's name.
- Document public types, functions, methods, traits, variants, and fields where their contract needs explanation. Add
  crate or module docs when their purpose or relationship to other modules is unclear and the file is in scope.
- Describe meaningful `Option`/`None`, empty-collection, `Default`, borrowing, ownership, error, and panic semantics.
  Explain asynchronous cancellation, locking, or task shutdown when callers or maintainers rely on those guarantees.
  Use `# Errors`, `# Panics`, `# Safety`, or `# Examples` only where relevant and supported by the implementation.
- For serialized API types, use the actual configuration field names and distinguish runtime checks from schema
  validation. Verify defaults and missing-value behavior through serialization attributes and callers, rather than
  inferring them from Rust field types alone. Follow the derived documentation rules below for schema-facing prose.
- Use `//` near non-obvious decisions, such as why a status read must be live, why a watcher is cancelled before joining,
  or why mapping source selectors and destination paths differ. Explain the applicable invariant rather than narrating
  each statement or duplicating nearby API documentation.
- Use resolvable intra-doc links for Rust symbols and Markdown links for URLs. The crate denies broken intra-doc links
  and bare URLs. Keep examples self-contained; Rust code fences are doctests by default. Mark examples requiring a live
  cluster as `no_run` and non-Rust snippets with their correct language, rather than using `ignore` to hide invalid code.
- Use synthetic values in examples. Source objects and mapped values can contain Secret data; do not copy credentials
  into comments or add payload logging while documenting a path.

## YAML Comment Guidance

- Add `#` comments only to canonical YAML within scope. Explain non-obvious operational assumptions, ownership, ordering,
  permissions, or why a setting must remain consistent with other resources. Avoid restating field names or values.
- Preserve parsed values and structure exactly, including key ordering, anchors, document separators, quoting, and block
  scalars. A `#` inside a block scalar or quoted value is payload, not a YAML comment.
- Put explanations for generated artifacts in their owning source when eligible, rather than hand-authoring comments
  in generated output. For CRDs, see the source documentation and schema constraints below.

## Derived Documentation

Before editing doc comments consumed by macros, inspect their downstream output. In Sinker, `JsonSchema` documentation
in [resources.rs](../../../src/resources.rs) can become CRD titles or descriptions, and `clap` documentation in
[main.rs](../../../src/main.rs) can become CLI help. These are public output changes even though the input is a comment.

Change derived descriptions or help only when that output is included in the requested documentation scope. Preserve
validation and runtime behavior. If the necessary companion output is outside an exact-file or single-commit scope,
keep the pass to maintainer comments and report the out-of-scope documentation need instead of expanding the edit set.

For an intentional CRD documentation update, follow [Generating CRDs](../../../README.md#generating-crds) and the API
instructions in [AGENTS.md](../../../AGENTS.md#implementation-constraints). Generate to temporary files before and after
the source edit to distinguish its effects from existing drift. Carry only intended, in-scope documentation changes
from generated output into the checked-in CRDs; preserve schema constraints, defaults, and serialized fields. Keep
affected examples accurate when they are included in the requested scope.

`SinkerContainer` uses `crd_with_manual_schema()` to preserve arbitrary `.spec` content, so documentation on its empty
Rust spec type does not describe the stored payload. Read the manual schema when explaining that API.

For intentional CLI help changes, compare `cargo run --locked -- --help` and the affected subcommand's help before and
afterward. Preserve flags, defaults, environment bindings, and parsing behavior. Running without a subcommand starts
reconciliation; use explicit help or `manifests` invocations for local documentation checks.

## Verification

1. For Rust edits, run the root [development checks](../../../AGENTS.md#development-and-verification) from the repository
   root, using the selected toolchain, locked dependency resolution, and read-only `cargo fmt --check`. Rust comment
   edits still require the prescribed build, format, test, and Clippy checks. Fix formatting only within the edited scope.
2. For changed Rustdoc, also run `cargo doc --locked --no-deps --document-private-items` to check links and rendering.
   Select `--lib` or `--bin sinker` as needed to cover the edited target. Inspect the relevant generated pages;
   the prescribed `cargo test --locked` includes library doctests. If using a focused test
   filter while investigating, confirm selection with `cargo test --locked <filter> -- --list`; filters match test names,
   not source paths. Local checks do not establish live Kubernetes behavior.
3. For YAML comment edits, compare parsed values with the pre-edit version, including examples outside the deployment
   bundle. When deployment manifests change, run `kubectl kustomize manifests` and compare rendered output before and
   after the pass. Rendering is local and does not deploy resources.
4. For source comments that feed CRDs or CLI help, perform the comparisons in
   [Derived documentation](#derived-documentation). Treat any unexpected public output change as a scope or correctness
   issue, and distinguish pre-existing CRD drift from changes introduced by the pass.
5. Run `git diff --check` and review the final diff against the initial working-tree state for scope, redundant or
   inaccurate comments, accidental literal or attribute changes, generated output, and YAML values or formatting.
   Restore incidental changes introduced by editing tools, including unrelated final-newline changes.

## Final Response

Summarize the requested scope, main files or areas documented, intentional changes to derived documentation, and checks
actually run. Note relevant skipped generated files, verified documentation drift, and any checks that could not run.
