---
name: create-readme
description: Create or update repository, module, package, or directory READMEs and agent-facing documentation such as skills and AGENTS.md files. Ground guidance in sources, keep primary documents concise, and link to task-specific details.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.

# Create README

Write documentation based on the actual implementation, tests, configuration, and existing docs. Tailor it to the
intended audience, with detail proportional to the target's complexity and the user's requested scope.

## Agent-Facing Documentation and Context

For all documentation, including general `README.md` files, keep primary documents concise: summarize essentials,
link to existing sources, and separate substantial task-specific detail into references that readers can load when needed.
Preserve the information needed by the intended audience and the constraints needed to act correctly.

When authoring or improving skills or `AGENTS.md` files, read
[agent-facing documentation guidance](references/agent-facing-documentation.md) for audience selection, reference
structure, avoiding duplication, and self-improvement hooks. For README restructuring, use its guidance on summaries
and references while preserving human-facing usage information.

## Workflow

1. Identify the requested target, audience, output path, and applicable `AGENTS.md` instructions. If the target is ambiguous,
   inspect likely paths before asking for clarification. Resolve module names to actual files; a Rust module may be a
   single `.rs` file rather than a directory. For README tasks, use the containing directory's README or existing coverage
   unless the user specifies another location.
2. Establish any requested change scope before choosing edits:
    - Between refs: inspect `git diff --name-status <base>...<head>` and the relevant hunks for branch changes since the
      merge base; use `<base>..<head>` for a direct comparison of the two trees.
    - Explicit commits: inspect each with `git show --name-status --patch <commit>`.
    - Since the README was last updated: find its last commit with `git log -1 --format=%H -- <README>`, then inspect
      `git diff <commit>..HEAD -- <target>`. If it has no history, inspect the current implementation directly.
3. Read the sources needed to substantiate the requested documentation. For implementation docs, trace responsibilities,
   public interfaces, data flow, side effects, and maintenance concerns through relevant code, tests, and configuration.
   For agent instructions, verify workflows, commands, and constraints against applicable instructions and available tools.
   Follow references when relevant to the task. For historical docs, validate against the requested revision.
4. For scoped updates, extract changed identifiers, configuration fields, and behaviors from the diff. Search the
   complete target document and relevant ancestor or linked documentation for affected usage examples, lifecycle
   descriptions, and summaries. Include related files outside the target when needed to verify behavior, but keep
   documentation edits tied to the requested scope.
5. Draft or update the document, preserving accurate existing content and the user's chosen structure. Summarize and
   link to existing coverage before creating new references; keep each detailed topic in one maintained location.
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

For Sinker implementation, development commands, or CRD generation, read the relevant parts of the
[Sinker source guide](references/sinker-source-guide.md).

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
- Link to source files and related documentation using paths relative to the document's location. Explain when each
  reference is useful so agents can select relevant context without reading every linked document.
- Treat existing docs and example fixtures as evidence to check, not proof that a behavior or deployment is supported.
  Mark consequential unknowns explicitly.
- Reread prose, tables, examples, and diagrams together against the implementation so a scoped update leaves no
  contradictory descriptions.
- Run checks that substantiate the documentation changes; distinguish commands verified by inspection from checks
  actually executed. Report implementation or manifest discrepancies without expanding a documentation task into
  unrelated repairs.
