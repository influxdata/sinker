---
name: rust-unit-tests
description: Write comprehensive Rust unit tests for a user-specified file, module, function, or code path. Use when the user asks to add, improve, review, or generate Rust tests, including table-driven testing with rstest, branch coverage, Tokio async tests, randomized fixtures, temporary filesystem behavior, and precise success and error assertions.
---

> **After completing tasks with this skill:** Invoke [improving-skills](../improving-skills/SKILL.md) to capture feedback
> and lessons learned. Combine this with the repository's required feedback pass.

# Rust Unit Tests

Write focused tests that comprehensively exercise the requested behavior. Preserve existing regression coverage and
keep changes within the requested code path. Aim for 100% code coverage of that scope as much as reasonably possible,
using measured coverage profiles to find gaps and guide additional cases.

## Workflow

1. Read the repository's [AGENTS.md](../../../AGENTS.md) and use the [README source map](../../../README.md#development)
   to locate the target. Inspect its implementation, callers, existing tests, and relevant documented invariants before
   choosing cases. File links in this skill are relative to this file.
2. Map reachable branches, boundary values, relevant parameter combinations, and success and error outcomes. Include
   absent, empty, default, and non-default inputs where they produce distinct behavior, plus the explicit combinations
   described under [Regression-Resistant Cases](#regression-resistant-cases).
3. Add or extend an inline `#[cfg(test)]` module beside the implementation, following Sinker's existing layout. Reuse setup
   helpers where useful, but keep each case's inputs and expected behavior visible. Avoid widening production visibility
   solely to test private helpers.
4. Run the focused tests and generate coverage profiles as described under [Coverage evaluation](#coverage-evaluation).
   Inspect uncovered code, add meaningful cases for remaining reachable paths, and regenerate profiles after changes
   until the requested scope reaches 100% where reasonable or the remaining gaps have concrete explanations.
5. Complete the required repository checks described under [Verification](#verification). Report measured coverage,
   assumptions, remaining gaps, and checks actually run; do not claim a coverage percentage without measurement.

## Dependencies and test style

Use [Cargo.toml](../../../Cargo.toml), [Cargo.lock](../../../Cargo.lock), and
[rust-toolchain.toml](../../../rust-toolchain.toml) for available dependencies, resolved APIs, and the toolchain.
Sinker already has `rstest`, `rand`, and `once_cell` as dev-dependencies, and Tokio as a runtime dependency.
Prefer standard assertions and existing helpers. Add a dev-dependency or feature only when the requested tests need it;
the project does not currently include an assertion library or temporary-directory helper.

- Prefer `#[rstest]` with named `#[case::scenario(...)]` cases for comparable inputs and outcomes. Use `#[test]` for
  synchronous tests that do not benefit from a table, and `#[tokio::test]` when the test needs an async runtime.
- Use helpers, builders, or `#[fixture]` for shared setup when they make the cases clearer. Share immutable fixture data
  or construct fresh state per case; avoid abstractions that obscure the behavior being asserted.
- Test observable behavior and meaningful invariants, including preservation of unrelated data when relevant. Derive
  expected values independently of the implementation so tests can detect regressions.
- Use `#[should_panic(expected = "...")]` only for intentional panic contracts. Report accidental panics encountered
  while designing cases instead of treating them as required behavior.

## Regression-Resistant Cases

- Preemptively add explicit cases for complicated combinations of input parameters, even when simpler tests already
  imply the same outcome under the current implementation. Preserve these cases during refactors even if they add no
  measured coverage.
- When a function loops over an input slice, array, `Vec`, or map, include explicit cases with multiple items as much
  as reasonably possible, alongside relevant empty and single-item cases.
- When a loop body has multiple logical branches, use mixed-item cases that hit and verify each reachable branch
  multiple times when practical. Assert ordering, accumulation, mutation, and error handling across repeated iterations
  where applicable. For early returns or errors propagated with `?`, use separate cases to place the stopping condition
  after earlier work and verify observable effects and skipped later work. Respect the collection's ordering contract;
  do not assume `HashMap` iteration order.
- When independent inputs can interact, cover meaningful combinations of flags, `None`, default and populated values
  (including relevant `Some` values), valid and invalid entries, duplicates where representable, and collaborators that
  succeed or fail. Prioritize combinations likely to regress during future refactors without an exhaustive Cartesian
  product. Keep each case's expected outcome explicit.

## Assertions and errors

Assert concrete return values, resulting state, or error variants and payloads. For JSON and Kubernetes objects, compare
the relevant structure rather than serialized key order or a broad string match.

Handle every `Result` from the test and its setup: use `expect`, `?` in a test returning `Result`, or explicit matching.
Cover relevant success and failure branches of the code under test; setup helpers do not each need their own error
matrix. Some functions return `Result` without a reachable error branch, so do not invent one solely to satisfy coverage.

For expected errors, prefer `expect_err` and a match on [Sinker's error variants](../../../src/lib.rs) or the relevant
module's error type, checking meaningful payloads. A cause-specific variant can be sufficient. Check a stable message
substring when text carries additional meaning or the error is opaque; avoid coupling to full dependency error wording.
An `is_err()` assertion alone is insufficient when a more precise check is possible. For `Result<()>`, successful
completion may be the whole return contract; also check side effects where applicable.

## Fixtures and isolation

Use explicit, distinct, non-empty and non-default values for fields relevant to the scenario; retain defaults for
irrelevant scaffolding. Include separate cases for defaults, empty values, and `None` when they affect behavior.

Use randomized fixtures when variation strengthens the test. Seed a local RNG with `StdRng::seed_from_u64`, use the
resolved `rand` API, and generate values that satisfy the intended domain. For example, alphanumeric sampling can
produce digits, so it is unsuitable without filtering for a namespace suffix meant to match `[a-z]`. Report the input
and seed on failure. Fixed seeds make failures reproducible; they do not make invalid fixture generation correct.

Use fixed timestamps for ordering or retained-time assertions. For Kubernetes timestamps, use
`k8s_openapi::jiff::Timestamp` as in [filters.rs](../../../src/filters.rs). When the code reads the current time internally,
bound the expected time around the call instead of relying on sleeps or an exact independently sampled timestamp.

For filesystem behavior, create files and directories under a unique system-temporary directory per case. Arrange
cleanup even when assertions fail, handle explicit cleanup results, and assert cleanup failures when they are part of
the behavior under test. Prefer a temporary-directory guard when available. Use repository files only when the target
requires them, and keep test output out of the checkout.

## Async behavior and Kubernetes boundaries

The Rust test harness already runs tests in parallel; an async annotation is not needed for test-level concurrency.
Isolate global state, environment variables, filesystem paths, and ports for both sync and async tests. Changing a test
to `#[test]` does not serialize it. When isolation is impossible, use explicit coordination or a serial test invocation.

For async behavior, test success, failure, cancellation, and ordering where applicable. Coordinate with channels or
barriers rather than sleeps, bound waits, and await spawned tasks while checking both join errors and returned results.
Cancel or otherwise stop background tasks during cleanup. Tokio's configured `full` feature does not include
`test-util`; check feature availability before using paused-time utilities.

Sinker's existing tests exercise local logic. For API-dependent code, inspect the boundary and use a controlled client
or a narrowly scoped test seam when needed. Tests must not rely on ambient kubeconfig or a live cluster. Keep assertions
about local decisions distinct from claims about API-server authorization, validation, server-side apply, or garbage
collection. Read the relevant [implementation constraints](../../../AGENTS.md#implementation-constraints) when testing
mapping, status, access checks, or watch cleanup. If the requested behavior requires live verification, report that gap
and follow the repository's live-testing instructions only when that work is in scope.

## Coverage evaluation

For test additions, improvements, or coverage reviews, use an appropriate Rust coverage tool to build profiles and
evaluate completeness. Prefer `cargo-llvm-cov` with LLVM tools compatible with the repository's pinned toolchain;
an equivalent tool is acceptable if it supplies useful coverage data for the requested scope. Read the
[coverage profiling workflow](references/coverage.md) before collecting profiles for commands, prerequisites, and
report interpretation. If tooling cannot run, report the specific blocker and mark coverage as unmeasured.

Inspect per-file and relevant function or region details for the requested production code, including zero-hit paths;
a repository-wide total alone cannot establish completeness for a scoped task. Review line, region, and function
coverage and branch coverage when supported. Explain unsupported metrics. Cross-check reports against the branch and
case inventory: 100% line coverage does not establish complete branch, input-combination, or assertion coverage.

Use uncovered paths to choose the next cases, retaining the [regression-resistant cases](#regression-resistant-cases)
even when percentages no longer increase. Explain each remaining gap, such as an unreachable defensive branch,
platform-specific code, or behavior requiring an external system. Preserve production contracts and meaningful
assertions; do not remove behavior or hide reachable code from reports solely to reach 100%.

## Verification

Run commands from the repository root. Cargo filters match module or test names, not file paths: replace `<filter>` in
`cargo test --locked <filter> -- --list` with a name found in the target's tests and confirm the intended cases are
selected before running `cargo test --locked <filter>`. A successful command that selects zero tests does not verify
the change.

For Rust changes, complete all checks in [AGENTS.md's development instructions](../../../AGENTS.md#development-and-verification),
including the full locked test suite. Use its additional checks if the task also changes schemas or deployment files.
For skill-only edits, validate frontmatter, relative links, and claims against the sources; execute examples only when
needed to substantiate them.

Summarize the behavior covered, verification results, and any untested paths or existing failures. For measured coverage,
include the tool and commands, test and report scope, features, exclusions, measured metrics, and profile/report paths.
Distinguish local unit-test results from live behavior and commands inspected from commands executed.
