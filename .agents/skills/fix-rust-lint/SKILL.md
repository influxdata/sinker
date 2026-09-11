---
name: fix-rust-lint
description: Fix Rust Clippy and rustfmt issues in Sinker while preserving behavior and public contracts. Use when asked to run, diagnose, or fix Rust lint findings, review Clippy suggestions, or iterate until formatting passes and Clippy reports no warnings.
---

> **After completing tasks with this skill:** Invoke [improving-skills](../improving-skills/SKILL.md) to capture feedback
> and lessons learned. Combine this with the repository's required feedback pass.

# Fix Rust Lint

Fix Rust lint and formatting findings with small, behavior-preserving changes. Sinker is a single Cargo package with
a binary and library; run Cargo commands from the repository root. File links in this skill are relative to this file.

## Workflow

1. Read [AGENTS.md](../../../AGENTS.md) and use the [README source map](../../../README.md#development) to locate the
   requested code. Inspect `git status --short` before edits and preserve unrelated user changes. For a diagnosis-only
   request, use non-fixing commands and report findings without applying fixes.
2. Inspect [Cargo.toml](../../../Cargo.toml), [Cargo.lock](../../../Cargo.lock),
   [rust-toolchain.toml](../../../rust-toolchain.toml), [rustfmt.toml](../../../rustfmt.toml), and
   [CI](../../../.github/workflows/rust.yml). Check for additional Cargo or Clippy configuration if present. Use the
   pinned toolchain and locked dependencies; do not upgrade them merely to resolve lint findings. Inspect crate-level
   lint attributes and the affected implementation, callers, and tests before choosing a fix.
3. For a fix request, run the fixing command below. Review every automatic edit against the initial working tree.
   Keep incidental fixes in other source files only when they preserve behavior and are needed for the final Clippy
   check; identify those files in the final response. Do not overwrite pre-existing edits while removing unwanted fixes.
4. Resolve remaining findings using the [fixing rules](#fixing-rules). Prefer a behavior-preserving code change over a
   suppression. If a finding cannot reasonably be fixed, use the narrowest justified lint expectation or allowance.
5. Run `cargo fmt` to apply formatting, then review the diff. Treat formatting as an edit; use `cargo fmt --check` for
   read-only validation. Use [rust-unit-tests](../rust-unit-tests/SKILL.md) when fixing test code or adding regression
   coverage, as described under [Test code](#test-code).
6. Complete the [validation commands](#validation) after Rust edits. If validation reveals another finding, repeat the
   affected fixes and checks until Clippy reports no warnings and formatting passes, or report the concrete blocker.
7. Summarize fixes, incidental files changed, any lint expectations or suppressions, and checks actually run. Distinguish
   existing failures from regressions and describe any unverified behavior.

## Fixing rules

- Preserve function behavior, public signatures, trait implementations, return values, error variants and text,
  serialization, CLI flags, side effects, ordering, locking, and async/concurrency behavior. Follow
  [AGENTS.md's behavior-preservation rule](../../../AGENTS.md#code-style) if a contract change is unavoidable; explain
  the necessity and effect. A lint suggestion alone does not justify a behavior change.
- Keep fixes local and reuse existing helpers or established dependency APIs. Consolidate repeated logic when it
  improves clarity, without introducing broad abstractions or unrelated cleanup.
- Treat machine suggestions as proposals. Re-check ownership, borrowing, lifetimes, drop order, iterator laziness,
  allocations, and side effects. For async changes, inspect lock scope, cancellation, task joining, and ordering.
- Preserve panic behavior deliberately. Do not replace `unwrap`, `expect`, indexing, or panics with fallible behavior
  merely to satisfy a lint; that changes the contract.
- For API or layout suggestions such as `large_enum_variant`, `result_large_err`, `boxed_local`, `ptr_arg`,
  `too_many_arguments`, `new_without_default`, derives, or lifetime elision, inspect callers, trait bounds, serde
  behavior, and error handling before accepting the change.
- For iterator and collection fixes, preserve ordering, duplicates, short-circuiting, mutation, and error accumulation.
  Keep explicit control flow when it makes side effects or error handling clearer.
- Read the relevant [implementation constraints](../../../AGENTS.md#implementation-constraints) before accepting fixes
  that affect mappings, client resolution, status, ownership, or watches. These paths have contracts beyond what local
  unit tests establish.
- Place lint expectations or allowances on the narrowest applicable expression or item and name only the specific
  lint. Prefer `#[expect(..., reason = "...")]` when the pinned toolchain supports it; otherwise use `#[allow(...)]`
  with a concise explanation. Do not add broad suppressions such as `allow(warnings)` or `allow(clippy::all)`, or
  unexplained attributes. For example:

  ```rust
  #[expect(clippy::too_many_arguments, reason = "Signature must match the existing public API")]
  ```

- Review any generated, vendored, lockfile, or configuration changes before keeping them. Follow the established
  generator workflow where applicable. For API/schema changes, follow the
  [CRD comparison instructions](../../../README.md#generating-crds), generating into a temporary file. Preserve schema
  constraints and report any existing drift; do not regenerate tracked CRDs as routine lint cleanup.

## Test code

Use [rust-unit-tests](../rust-unit-tests/SKILL.md) for test edits and regression coverage instead of duplicating its
fixture, assertion, isolation, and async guidance here. Read nearby inline tests and preserve their existing coverage.
Add focused cases when a fix affects behavior or carries a meaningful regression risk; mechanical lint and formatting
edits do not by themselves require new tests or a coverage campaign.

Sinker uses ordinary Rust tests, `rstest`, and Tokio tests. Verify a focused test selection with
`cargo test --locked <filter> -- --list` before running `cargo test --locked <filter>`; filters match test names, not
file paths. Focused tests supplement the required full suite. Local unit tests do not verify live Kubernetes behavior.

## Validation

For lint fixes, run this from the repository root, then review the edits:

```bash
cargo clippy --locked --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings
cargo fmt
```

After Rust changes, complete the repository's build, formatting, test, and Clippy checks:

```bash
cargo build --locked
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
```

The final Clippy command adds `-D warnings` to the repository's required invocation so warnings fail this skill's
completion check; CI itself does not set that flag. Run additional schema or deployment checks only when the change
calls for them under [AGENTS.md](../../../AGENTS.md#development-and-verification).

If build artifacts cannot be written, use a writable target directory such as `CARGO_TARGET_DIR=/tmp/sinker-target`.
This does not relocate the dependency cache. If sandbox restrictions on cache writes, downloads, or toolchain components
block a necessary command, request the required execution approval; distinguish environment failures from code failures.

For skill-only edits, validate frontmatter, supporting metadata, relative links, and command claims against the local
sources. Execute examples only when needed to substantiate them. Report commands inspected separately from checks run.
