---
name: comment-code-diff
description: Add thorough, explicit, concise comments to Go code and canonical YAML manifests, plus operator-facing spec.notes for applicable RollingUpdate resources, in a requested scope such as a branch diff, file, directory, package, Go module, or manifest tree. Use for documentation-focused passes that should explain behavior and operational intent while avoiding generated files and behavior changes outside that narrow notes exception.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.

# Comment Code Diff

## Overview

Use this skill to make a documentation-focused pass over Go code and canonical YAML manifests in the user's requested scope. Starfleet is a multi-module Go monorepo for Kubernetes controllers, operators, CLI tooling, shared APIs, and shipyard manifest generation. Add comments appropriate for code maintainers, code users, and PR reviewers where they clarify behavior, API contracts, operational assumptions, or reviewer-relevant intent. When a supported third-party dependency or its committed `RollingUpdate` is in scope, also add or update applicable operator-facing `.spec.notes`. This field is the skill's sole standing exception to a comment-only edit. Be thorough and explicit about non-obvious context while keeping documentation concise and avoiding narration that simply repeats the code or manifest.

## Workflow

1. Inspect repository guidance and working tree state.
   - Read the root `AGENTS.md` and any component-level `AGENTS.md` that applies to the requested scope, such as `starfleet-controller/AGENTS.md`, `squadron-operator/AGENTS.md`, or `fleetctl/AGENTS.md`.
   - Run `git status --short`.
   - Identify the requested scope before reading code:
     - For a request naming one commit, including "the most recent commit," resolve the revision to its immutable commit hash and inspect only that commit with `git show <commit>` or `<commit>^..<commit>`. Do not substitute a branch comparison or include neighboring commits; when using the range form, preserve that exact comparison in every range command.
     - For branch or commit-range requests, preserve the complete named comparison in every range command, including whether it uses `..` or `...` (for example, `73c4a096..HEAD` or `origin/main...HEAD`).
     - Inspect current working-tree changes separately with `git status` and an unqualified `git diff`. Do not fold them into a requested commit range unless the user explicitly includes them in scope.
     - For requests that name multiple commits without an explicit range, inspect the union of those commits' changed files with `git show` or `git diff-tree` for each commit, and do not implicitly include intervening commits unless the user names a range.
     - For "this branch", "the diff", or unspecified branch comparison, default to `origin/main...HEAD`.
     - For explicit file requests, use only the named files unless the user asks to include related files.
     - For package, directory, or module requests, use the named directory/package/module scope. In this repo, module roots are directories with their own `go.mod`, including `starfleet-controller`, `squadron-operator`, `fleetctl`, `starfleet-kit`, `release-manager`, `cluster-test-probes`, `instance-creation-test`, and `region-classifier`.
     - When the user combines scopes, such as "changes in `squadron-operator/internal/controller` since main", intersect them: use the diff range filtered to that path.

2. Scope the review to relevant changed files.
   - For diff-range scopes, start with `git diff --stat <comparison>` and `git diff --name-only <comparison>`, where `<comparison>` is the complete requested range rather than only its base revision.
   - For explicit files, read those files directly, and use `git diff -- <file>` or `git diff <comparison> -- <file>` only if the user asked for comments based on changes.
   - For package, directory, or module scopes, enumerate Go files and canonical YAML manifests with `rg --files <scope>` and, when a diff comparison is relevant, filter with `git diff --name-only <comparison> -- <scope>`.
   - Skip generated files, vendored files, dependency metadata, and unrelated docs unless the user explicitly asks for them. In Starfleet, this includes `zz_generated.deepcopy.go`, generated mocks such as `*_mock_test.go` when their header says they are generated, and controller-gen output such as generated CRD or RBAC YAML.
   - Read related changed documentation as supporting context when it describes eligible code or manifest behavior. Do not edit it during a comment-only pass unless the user separately requests documentation changes; report any verified documentation drift in the final response.
   - Treat repo-owned canonical YAML as eligible for comments when the requested scope includes it. Known canonical YAML trees include `manifests/release/**`, `**/dist/**`, and most `**/config/**` YAML; these are source manifests, so useful `#` comments may be appropriate when they clarify non-obvious operational intent.
   - Treat `starfleet-controller/dist/manifests/management-cluster/config/rollingupdates/**` as canonical YAML. When branch, multi-commit range, directory, module, or manifest-tree changes add, modify, or partially remove a supported third-party dependency, its corresponding `RollingUpdate` is an eligible synchronization file for `.spec.notes` even if that file was not already changed. Do not use this exception to audit unrelated dependencies. Keep an exact single-commit request limited to files changed by that commit and an explicit-file request limited to the named files; report an applicable out-of-scope note instead.
   - Treat generated YAML snapshots as read-only output. In particular, skip `**/rendered_manifests/**`: those files are snapshot-test outputs produced from fixed inputs, canonical manifests, and the applicable generator or transformer code, so hand-written comments there will be overwritten.
   - Treat generated YAML under `**/config/**` as read-only too. In `starfleet-controller` and `squadron-operator`, this includes CRD and RBAC YAML generated from Go API types, kubebuilder markers, or controller-gen configuration, even though other config YAML is generally canonical.
   - For large diffs, prioritize handwritten production code before tests, docs, or tooling.

3. Read changed code before editing.
   - Use `git diff --unified=80 <comparison> -- <path>` for changed context when a diff comparison applies.
   - For explicit files or module scopes without a diff comparison, read the whole target file plus nearby tests or callers as needed.
   - Read surrounding files and tests when needed to understand contracts, Kubernetes reconciliation semantics, shipyard generator or transformer behavior, CLI command behavior, status propagation, or edge cases.
   - When a supported third-party dependency or a committed `RollingUpdate` is in scope, read [RollingUpdate Notes](references/rollingupdate-notes.md) before editing. It defines the update-system sources of truth, applicability threshold, dependency-specific research, content boundaries, and verification for `.spec.notes`.
   - Prefer `rg` for finding related exported identifiers, callers, and existing comment style.

4. Add comments only where they carry useful intent.
   - Follow GoDoc conventions for exported packages, types, funcs, vars, consts, interface methods, and struct fields that are part of a public or cross-package contract.
   - Add package docs for new public packages when the package purpose is not already documented.
   - Write for the relevant audience: maintainers need invariants, lifecycle ordering, ownership, and maintenance hazards; code users need API contracts, defaults, nil and zero-value behavior, authorization, and compatibility expectations; PR reviewers need intent behind changed behavior, tradeoffs, and risk-sensitive decisions.
   - Add inline comments for non-obvious behavior: reconciliation ordering, finalizers, ownership, pruning, Sinker sync boundaries, status and condition transitions, shipyard manifest filtering or emission, cloud-provider assumptions, multi-tenant safety, security posture, nil semantics, retries, matching rules, compatibility behavior, data sensitivity, or deliberately skipped work.
   - In Starfleet API type packages, treat comments on CRD types and fields as externally visible API documentation because kubebuilder can copy them into CRD schema descriptions. Preserve `+kubebuilder`, RBAC, deepcopy, and other code-generation markers exactly unless the user explicitly asked to edit them.
   - In `fleetctl`, comment command behavior where it clarifies flag/config/env precedence, interactive prompts, generated equivalent commands, watch selectors, or compatibility with existing automation.
   - In shipyard generators and transformers, explain why resources are emitted, dropped, merged, or cloud-specialized when the reason is not obvious from the manifest shape.
   - In canonical YAML manifests, use YAML `#` comments sparingly for operational intent that is not obvious from resource kind, name, labels, or field values. Avoid comments that merely restate Kubernetes field names or duplicate adjacent Go transformer comments.
   - In a canonical `RollingUpdate`, put durable guidance needed by the person approving a discovered dependency version in `.spec.notes`, not only in YAML `#` comments. Keep maintainer-only rendering and identity invariants as YAML comments. Add, revise, or remove notes only according to [RollingUpdate Notes](references/rollingupdate-notes.md).
   - Replace mechanical comments like "construct request", "set header", or "return error" with comments that explain why the code does that work, or remove them if no extra context is needed.
   - Phrase reviewer-oriented context as maintainer-facing code intent. Do not mention PRs, reviewers, the comment-writing task, or other review process details in committed code comments.
   - Be thorough and explicit enough to capture the needed reason, contract, or operational implication, but keep each comment close to the code it explains and as short as accuracy allows.

5. Preserve behavior.
   - Do not change exported signatures, error behavior, metric names, condition types, logging of sensitive values, resource names, labels, annotations, owner references, CLI output, or Kubernetes apply/prune semantics while adding comments.
   - Treat string literals, raw string contents, struct tags, identifiers, executable statements, kubebuilder markers, and generated CRD schema descriptions as behavior. Restore any non-comment changes unless the user explicitly requested them or the change is an applicable canonical `RollingUpdate.spec.notes` edit made under this skill.
   - A `.spec.notes` edit changes notifier-visible API data even though it does not select a version or target. Keep it limited to operator documentation; do not change discovery, update type, targets, concurrency, names, or any other manifest value during the notes pass. Never introduce committed `.spec.selectedVersion`, which is runtime approval state owned by `fleetctl dependency approve`. If a resource already commits that field, do not modify it under documentation-only authority; report the blocking contract violation and the need for a separately authorized behavior change.
   - Do not hand-edit generated files. Regenerate them from the owning component's tooling only when the user's requested comment pass intentionally changes source comments that drive generated output.
   - Avoid broad refactors, even if comments reveal cleanup opportunities.

## Go Comment Guidance

- Start GoDoc comments for exported identifiers with the identifier name.
- Make package comments begin with `Package <name> ...`.
- Follow GoDoc conventions where they apply; do not force GoDoc-style wording onto ordinary inline comments.
- When working in `starfleet-kit` or other library-like code that may be imported from multiple Go modules, treat exported functions and methods as external APIs whose GoDoc should include usage instructions and examples where applicable.
- Document nil, zero-value, timeout, authorization, and ownership semantics when callers must know them.
- For interfaces, explain the contract and any important method-level behavior.
- For config structs, comment fields whose JSON meaning, defaults, secrecy, or operational impact would not be obvious from the field name.
- For Kubernetes API structs, make field comments accurate for CRD users, not just Go callers.
- Keep inline comments close to the decision they justify.

## YAML Comment Guidance

- Only add comments to canonical YAML manifests, such as `manifests/release/**`, `**/dist/**`, most `**/config/**` YAML, or another YAML file that local guidance or file context clearly identifies as handwritten source.
- Do not add comments to generated YAML output, including `**/rendered_manifests/**`, generated CRDs, generated RBAC manifests, generated portions of `starfleet-controller/config/**` or `squadron-operator/config/**`, or other YAML with generated-file headers.
- When a generated YAML snapshot lacks an important explanation, add the comment to the canonical manifest or to the Go generator or transformer that produces the snapshot.
- Keep YAML comments close to the field, list item, or resource they explain, and focus on operational assumptions, ownership, ordering, security posture, cloud-provider differences, or why a value must not drift.
- For `RollingUpdate` manifests, distinguish YAML comments from `.spec.notes`: comments explain the resource to maintainers, while notes are Markdown delivered to operators when approval is needed. Do not duplicate the same prose in both places.
- Preserve YAML semantics exactly: do not reorder keys, normalize formatting, change anchors, alter document separators, or move comments in a way that changes parser behavior.

## Verification

After edits:

1. Run `gofmt` on touched Go files. For `fleetctl`, `just fmt` is also acceptable from the module root.
2. Run focused tests from the owning Go module, not the repository root. Use the nearest `go.mod` to choose the module root and prefer the component guidance from its `AGENTS.md`.
3. If comments changed Kubernetes API type docs or code-generation markers in `starfleet-controller/api` or `squadron-operator/api`, run the component's appropriate generation target, usually `make manifests` and, when type generation is affected, `make generate`.
4. If canonical YAML comments or `RollingUpdate.spec.notes` changed manifests that feed snapshot tests, run the owning generator, transformer, or snapshot test workflow when it is reasonably discoverable from the surrounding package. For `starfleet-controller/**/rendered_manifests/**`, regenerate from `starfleet-controller/**/dist/**` with the starfleet-controller `make render` target. For `release-manager/**/rendered_manifests/**` and `squadron-operator/**/rendered_manifests/**`, regenerate from `manifests/release/**` with the release-manager `just render` target.
5. If linting, use `$fix-go-lint` guidance and run the narrowest useful module-scoped `golangci-lint run` scope.
6. Run `git diff --check`.
7. Review `git diff` for changed literals, struct tags, identifiers, statements, raw string contents, code-generation markers, generated output, YAML values or ordering, redundant comments, inaccurate comments, or accidental non-comment behavior changes. For a `RollingUpdate` notes pass, confirm `.spec.notes` is the only intentionally changed YAML value and no modified resource commits `.spec.selectedVersion`.
8. Restore incidental formatting-only changes introduced by editing tools, such as adding or removing a final newline from a file that was otherwise intentionally unchanged except for comments.

## Final Response

Summarize the documentation pass by naming the requested scope, the main files or areas touched, any companion `RollingUpdate.spec.notes` changes, the verification commands run, and any warnings or skipped generated files, including generated YAML snapshots such as `**/rendered_manifests/**`.
