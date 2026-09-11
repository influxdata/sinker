# Coverage Profiling

Use this workflow when adding or improving tests or evaluating their completeness. Commands run from the repository
root; file links are relative to this reference. Keep the requested production code scope explicit throughout.

## Tool selection

Check `cargo llvm-cov --version`, `cargo llvm-cov --help`, `cargo llvm-cov report --help`, and
`rustup component list --installed`. Use the toolchain from [rust-toolchain.toml](../../../../rust-toolchain.toml).
The repository's toolchain file requests rustfmt and Clippy; coverage tooling is a separate prerequisite.

Prefer [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov), which manages Rust coverage instrumentation and
profile reporting. When prerequisites are missing, use its installation instructions and a compatible LLVM component
for the pinned toolchain, or an available equivalent tool. LLVM profile tools must match the compiler's coverage format;
see the [Rust coverage documentation](https://doc.rust-lang.org/rustc/instrument-coverage.html#installing-llvm-coverage-tools).
Report a tooling blocker if no suitable option can run.

Check metric support against the installed tool and toolchain. `cargo-llvm-cov` currently marks `--branch` and doctest
coverage as unstable; do not assume they work on the pinned stable toolchain. Use supported metrics and inspect logical
branches manually when branch instrumentation is unavailable. Report this limitation explicitly.

## Collect and inspect profiles

Choose tests that exercise the requested code and confirm any name filter selects them using the
[skill's verification instructions](../SKILL.md#verification). Capture a baseline before edits when practical, then use
the same selection, features, and reporting scope for comparisons.

For a full-suite profile and multiple report formats from the same test execution:

```bash
sinker_coverage_dir="$(mktemp -d "${TMPDIR:-/tmp}/sinker-coverage.XXXXXX")"
export CARGO_LLVM_COV_TARGET_DIR="$sinker_coverage_dir/target"
cargo llvm-cov --locked --all-features --json --output-path "$sinker_coverage_dir/coverage.json"
cargo llvm-cov report --locked --all-features --html --output-dir "$sinker_coverage_dir/html"
cargo llvm-cov report --locked --all-features --show-missing-lines > "$sinker_coverage_dir/missing-lines.txt"
```

Run these sequentially and check every exit status. The first Cargo command runs instrumented tests and produces
profiles plus a JSON export; the `report` commands reuse those profiles. Preserve the directory path for review. Keep
the same coverage target directory and build options for reporting. The example covers the normal test targets with
all features enabled; it does not instrument doctests or exercise live Kubernetes behavior.

For quicker iteration, use `cargo llvm-cov test --locked --all-features <filter> --json --output-path <path>` with
verified test names and actual output paths. Test selection and report scope are separate: a test-name filter does not
restrict the report to the function under test. Inspect the relevant files and function regions in the JSON or HTML
report, including unexecuted code. Identify whether inline test helpers or generated code contribute to reported totals.

Use a fresh profile directory for each independent baseline or final collection, or the tool's documented cleaning
behavior. Do not reuse stale profiles after source changes or merge different runs unintentionally with `--no-clean`.
After filling gaps, rerun the relevant instrumented tests and regenerate reports; running `report` alone cannot
measure new code. Retain artifacts until the result has been reported.

## Evaluate completeness

Follow [Coverage evaluation](../SKILL.md#coverage-evaluation) to turn uncovered lines, regions, and functions into
additional cases. Compare the final report with the case inventory and any baseline. Distinguish a whole-file metric
from a calculation for a requested function; state the measured scope and metric with every percentage.

Record filters, features, platform, exclusions, and unsupported metrics so the result can be reproduced. Explain
legitimate exclusions such as generated code; do not exclude reachable production paths just to improve the percentage.
Coverage runs complement the [required repository checks](../../../../AGENTS.md#development-and-verification).
