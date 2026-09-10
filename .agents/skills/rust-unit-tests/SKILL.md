---
name: rust-unit-tests
description: write comprehensive rust unit tests for a user-specified file, module, function, or code path. use when the user asks to add, improve, review, or generate rust tests, especially for table-driven testing with rstest, branch coverage, tokio async tests, randomized fixtures, temporary filesystem behavior, precise assertions, and explicit verification of success and error results.
---

# Rust Unit Tests

Write comprehensive Rust tests for the file, module, function, or code path specified by the user.

## Core workflow

1. Inspect the target code before writing tests.
2. Identify all public behavior, logical branches, edge cases, parameter combinations, and error paths.
3. Prefer table-driven tests using `rstest` wherever practical.
4. Aim for 100% code coverage as much as reasonably possible.
5. Keep tests readable, focused, deterministic, and DRY.
6. Do not ignore `Result` values, error branches, or cleanup failures.

## Test style

Use `rstest` for table-based tests whenever reasonable.

Prefer named and parameterized cases, for example:

```rust
#[rstest]
#[case::empty_input("", Expected::Empty)]
#[case::valid_input("abc", Expected::Parsed)]
#[case::invalid_input("!", Expected::Error)]
fn parses_input(#[case] input: &str, #[case] expected: Expected) {
    // ...
}
```

Use `#[should_panic]` only when the behavior being tested is intentionally panic-based and cannot be more precisely verified with `Result` assertions.

When table-driven tests need shared randomized fixtures, define those fixtures before constructing the test case table using the `#[fixture]` attribute.

## Assertions

Use standard Rust assertions when they are clear and sufficient.

You may also use `https://github.com/google/assertor` when it improves readability or precision.

Keep assertions:

- precise
- focused
- readable
- tied directly to the expected behavior of the scenario

Avoid broad assertions that only prove the function “does something.”

## Error handling requirements

If any function used in a test returns a `Result`, explicitly verify both success and error outcomes for relevant branches.

Do not ignore errors.

For expected errors:

1. Assert the error type when the type is meaningful.
2. Assert the error value or error contents when the value is meaningful.
3. Use `expect_err`, pattern matching, `err.to_string().contains("...")`, or an equivalent assertion helper to verify that the error message includes a substring indicating the cause of the error.

Do not merely assert that an error exists unless no stronger assertion is possible.

## Fixtures and randomized values

For fixtures, use randomized non-`None`, non-empty, and non-default values as much as reasonably possible.

Randomized values must still produce deterministic and reliable tests. Prefer seeded randomness or helper functions that generate valid randomized values without introducing flakiness.

Use meaningful defaults only when the specific default value is part of the behavior under test.

## Async tests

Use `#[tokio::test]` as much as reasonably possible so tests can run concurrently.

Do not use concurrent async tests when concurrency would cause issues, such as:

- shared mutable global state
- shared filesystem paths
- process-wide environment variables
- timing-sensitive behavior
- external services or ports
- tests that intentionally mutate common resources

In those cases, isolate the state, use serial execution, or use a regular test where appropriate.

## Filesystem tests

If a test creates files or directories:

1. Create them only inside the system temporary directory.
2. Use unique paths for each test case.
3. Attempt cleanup when the test finishes.
4. Verify cleanup errors when cleanup is part of the behavior being tested.
5. Avoid relying on repository-relative paths unless the target code explicitly requires them.

Prefer temporary directory helpers where available.

## Coverage expectations

Ensure tests cover all logical branches as much as reasonably possible, including:

- valid inputs
- invalid inputs
- empty inputs
- boundary values
- default values
- non-default values
- optional values present and absent
- all relevant combinations of input parameters
- branching paths
- internal conditions that affect observable behavior
- success results
- expected error results
- panic behavior, only when intentional
- filesystem success and failure paths when applicable
- async success, failure, cancellation, or ordering behavior when applicable

Explicitly test all logical combinations of input parameters, branching paths, internal conditions, and expected outputs as much as reasonably possible.

## DRYness

Be DRY as much as reasonably possible.

Prefer helpers, fixtures, builders, and table-driven cases over repeated setup code.

Do not over-abstract tests if doing so makes the behavior harder to understand.

## Output expectations

When writing tests:

1. Add or update the appropriate test module or test file.
2. Include any required imports, dev-dependencies, or feature flags.
3. Explain any assumptions made about the code under test.
4. Call out branches that could not reasonably be tested and why.
5. Ensure the resulting tests are idiomatic Rust and should compile in the project context.

When modifying dependency files, add only the dependencies needed for the tests.
