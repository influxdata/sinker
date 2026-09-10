---
name: fix-rust-lint
description: Fix Rust clippy and rustfmt issues in a specific crate or Cargo workspace in the Tubernetes monorepo using crate/workspace-scoped `cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings`. Use when the agent is asked to run, diagnose, or fix Clippy lint findings for Rust code such as tubectl, especially when fixes must preserve behavior and iterate until `cargo clippy` reports no warnings.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.

# Fix Rust Lint

## Overview

Fix crate-scoped Rust `clippy` and `rustfmt` findings in the Tubernetes monorepo without running Cargo from an unrelated repository root. Preserve existing function behavior, public contracts, error semantics, async/concurrency behavior, and serialization formats while making lint-compliant, idiomatic changes.

## Workflow

1. Identify the target Rust crate or Cargo workspace from the user's request. Use the directory that contains the relevant `Cargo.toml`, such as `tubectl`. Do not run Cargo from `<repo-root>` unless `<repo-root>/Cargo.toml` is the relevant crate or workspace manifest.
2. Inspect local state before edits with `git status --short` from the repository root. Do not revert or overwrite unrelated user changes.
3. Inspect the crate's lint and build configuration before choosing command flags: `Cargo.toml`, `Cargo.lock`, `.cargo/config*`, `rust-toolchain*`, `clippy.toml`, Makefile targets, and nearby CI scripts when present.
4. Run Clippy from the crate or workspace root. For a single-crate root, start with:

```bash
cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings
```

For a Cargo workspace, add `--workspace` when the user asked for the whole workspace, or use `-p <package>` to keep the run scoped to the affected package.

5. Read the Clippy output and inspect any files changed automatically by `--fix`. Treat auto-fixes as edits that still require review. If `--fix` changes files adjacent to, but not directly part of, the feature or bug fix you were working on, keep only behavior-preserving fixes required for the final warning-free Clippy run and call those incidental edits out in the final response.
6. Fix each reported issue as much as reasonably possible. Prefer behavior-preserving code changes over suppression.
7. If a finding is a false positive or cannot reasonably be fixed, use the narrowest targeted lint expectation or suppression that the crate's MSRV and local style support. Prefer `#[expect(..., reason = "...")]` when supported; otherwise use `#[allow(...)]` with a nearby concise explanation:

```rust
#[expect(clippy::lint_name, reason = "concise explanation of why this lint finding is intentionally accepted")]
```

```rust
#[allow(clippy::lint_name)] // concise explanation of why this lint finding is intentionally ignored
```

8. Run `cargo fmt --all` from the same crate or workspace root unless tooling already formatted the edited files.
9. Run relevant `cargo test` commands from the same crate or workspace root for packages whose behavior or tests changed.
10. Re-run a non-fixing Clippy command from the same crate or workspace root. Iterate on fixes and validation until it reports no warnings.

## Fixing Rules

- Maintain the pre-existing behavior and contract of every modified item. Do not change public signatures, trait implementations, return semantics, error variants, error text, serialization/deserialization behavior, CLI flags, side effects, locking, ordering, or async/concurrency behavior unless the lint issue cannot be fixed otherwise and the user has agreed.
- Keep fixes as small and local as practical. Avoid unrelated refactors.
- Keep code DRY where it materially improves clarity or removes repeated lint-prone logic. Do not introduce broad abstractions only to satisfy a single finding.
- Prefer the Rust standard library, well-established crate APIs already in use, and existing local helper APIs over ad hoc parsing, cloning, allocation, reflection-like patterns, or string manipulation.
- Treat Clippy machine suggestions as proposals, not proof of correctness. Re-check ownership, borrowing, lifetimes, drop order, iterator laziness, allocation behavior, and side effects after accepting a suggestion.
- Preserve panic behavior deliberately. Do not replace `unwrap`, `expect`, indexing, or panics with fallible behavior unless that is already part of the intended contract or the user agrees.
- Be careful with lints that can alter API shape or data layout, such as `large_enum_variant`, `large_error_err`, `boxed_local`, `ptr_arg`, `too_many_arguments`, `new_without_default`, `derive_*`, and lifetime elision suggestions. Verify downstream call sites, trait bounds, serde formats, and error handling before keeping the change.
- For iterator and collection lints, preserve ordering, duplicate handling, short-circuiting, mutation, and error accumulation behavior. Avoid "simplifying" loops when explicit control flow makes error handling or side effects clearer.
- Explain every lint expectation or suppression thoroughly but concisely. Name only the specific lint being suppressed or expected, and place the attribute on the narrowest applicable expression, item, module, or test.
- Never use broad suppressions such as `#[allow(warnings)]`, `#![allow(warnings)]`, `#[allow(clippy::all)]`, `#![allow(clippy::all)]`, or unexplained allow attributes.
- If `--fix` changes generated, vendored, lockfile, or config-derived files, inspect repository conventions before keeping the changes. Regenerate from the source tool when that is the established pattern.
- If `--fix` changes unrelated source files in the same crate or workspace, do not reflexively revert them. First determine whether they are behavior-preserving lint fixes needed to make the final non-fixing Clippy command pass. Keep those required fixes, avoid broad cleanup beyond what Clippy reported, and explicitly summarize the incidental files changed when reporting back to the user.

## Test Code

When lint findings or edits touch Rust test code, or when a lint fix changes behavior that needs regression coverage, also use `rust-unit-tests`. Apply that skill's style guidance while fixing lint issues so tests remain idiomatic for this repository.

Convert Go unit-test patterns into Rust test work as follows:

- Read the target file and nearby existing tests before adding or rewriting tests.
- When the user asks for tests based on a commit or commit range, inspect that exact scope first with `git show --name-only <commit>`, `git diff --name-only <base>..<head>`, or `git log --stat <range>`, then focus coverage on changed behavior while still reading nearby code and tests.
- Identify public behavior, private helpers worth testing from the same module, success paths, expected error paths, `None` or empty inputs, default and populated values, logical branches, loops over caller-provided collections, and meaningful combinations of independent inputs.
- Prefer table-driven tests using `rstest` wherever practical, with named cases that make the scenario obvious.
- Define shared fixtures before table cases. Use `#[fixture]` for `rstest` fixtures when it improves clarity.
- Use deterministic randomized fixtures for non-`None`, non-empty, and non-default values as much as reasonably possible. Seed randomness or use helper functions so tests do not become flaky.
- Implement focused helper builders only when they remove meaningful repetition. Keep table entries readable and explicit.
- Do not ignore `Result` values returned by setup, cleanup, or functions under test. Use test functions that return `Result` where that keeps success paths clear, and use `expect_err`, pattern matching, or precise assertions for expected failures.
- For expected errors, assert the error type when meaningful and assert the error value or message contents with a substring that identifies the actual cause. Do not merely assert that an error exists unless no stronger assertion is possible.
- Preemptively add explicit cases for complicated combinations of input parameters, even when they are redundant under the current implementation.
- When code loops over a slice, array, iterator, or map provided as input, include explicit multi-item cases as much as reasonably possible.
- When a loop body has multiple logical branches, include mixed-item cases that prove ordering, accumulation, mutation, and error handling across repeated iterations.
- For CLI, command, or process-level tests, identify global/static state, environment variables, current directory changes, output capture, and function hooks before relying on Rust's default parallel test execution.
- Reset process-wide or global state before the assertion path and again with cleanup guards so test order cannot leak credentials, env vars, flags, mocked collaborators, or tracing/logging subscribers between tests.
- Prefer fresh command or object instances per test case. Avoid concurrent tests when the code under test mutates shared global state, shared filesystem paths, process-wide environment variables, ports, timing-sensitive resources, or external services.
- For async code, use `#[tokio::test]` when it matches the crate's async runtime. Test success, failure, cancellation, ordering, and shared-state behavior where those paths affect observable behavior.
- For filesystem tests, use system temporary directories with unique paths per test case. Prefer existing temporary-directory helpers or `tempfile` if adding a dev-dependency is justified. Attempt cleanup when the test finishes and verify cleanup errors when cleanup is part of the behavior under test.
- Aim for 100% branch coverage where reasonable. Use the crate's established Rust coverage tool when available, such as `cargo llvm-cov` or `cargo tarpaulin`; otherwise run targeted `cargo test` commands and clearly report that coverage tooling was unavailable.

## Validation Commands

Run commands from the crate or workspace root.

For a single crate:

```bash
cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features -- -D warnings
cargo fmt --all
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

For a whole workspace:

```bash
cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets --all-features -- -D warnings
cargo fmt --all
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

For a specific workspace package:

```bash
cargo clippy --fix --allow-dirty --allow-staged -p <package> --all-targets --all-features -- -D warnings
cargo fmt --all
cargo test -p <package> --all-targets --all-features
cargo clippy -p <package> --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

If `--all-features` is invalid because the crate has mutually exclusive features or feature-gated platform behavior, use the crate's established CI or Makefile feature set and the narrowest additional feature set needed for the changed code. Report the deviation.

If the default Cargo target directory or cache is not writable in the sandbox, rerun with a writable target directory such as `CARGO_TARGET_DIR=/tmp/<crate>-target`. If dependency download, missing toolchain components, or network access blocks a necessary command, rerun with approval outside the sandbox rather than treating it as a code failure.

If package names or targets are unclear after a lint finding, derive the narrow package with `cargo metadata` from the crate or workspace root before running tests. Finish only after the final Clippy run reports no warnings and formatting checks pass, or clearly report any blocker that prevents reaching that state.
