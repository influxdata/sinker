---
name: create-readme
description: Create or update a README for a user-specified repository module, package, or directory by thoroughly reading the code and documenting its purpose, architecture, APIs, behavior, maintenance guidance, and control flow. Use when the user asks to write module documentation, create a README, document a directory, explain how a package works for users and developers, or add a Mermaid diagram to documentation.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.

# Create README

Write detailed README documentation for a module or directory based on the actual code, tests, manifests, and existing docs. The README should serve both users who need to understand how to use the module and developers who need to maintain it.

## Workflow

1. Identify the requested module or directory. If the target is ambiguous, inspect likely paths before asking for clarification.
2. For requests scoped to changes between refs, inspect `git diff --name-status <base>...<head>` and the relevant hunks first, then update only documentation affected by behavior changes while validating against referenced code, API types, manifests, and tests.
3. When changes alter component registration, operational status, supported combinations, or an inventory of modules, controllers, commands, or resources, audit ancestor module READMEs and applicable `AGENTS.md` files for affected summaries or status tables. Update only statements made stale by the scoped changes.
4. For requests scoped to one or more explicit commits, inspect each commit with `git show --name-status --patch <commit>` or `git diff <commit>^!` before choosing README updates.
5. For requests scoped to changes since the target README was last updated, find the last README commit with `git log -1 --format=%H -- <README>`, inspect `git diff <commit>..HEAD -- <target>`, then validate affected changes against current code before editing.
6. After inspecting a commit or diff scope, extract changed identifiers, fields, configuration keys, and behaviors from the hunks. Search the complete target README and applicable ancestor documentation for every occurrence so distant lifecycle, operations, and maintenance statements are included in the audit.
7. Read the code thoroughly enough to explain purpose, responsibilities, public interfaces, data flow, operational behavior, and maintenance concerns.
8. Inspect adjacent tests, examples, generated manifests, existing docs, package metadata, and callers/importers when they clarify real usage.
9. Draft or update the README in the target directory unless the user specifies another output path.
10. Verify that the documentation matches the code and does not invent behavior, commands, APIs, configuration, or dependencies.

## Investigation Guidance

Use repository-native tools first:

- Use `rg --files <target>` to map the target directory.
- Use `rg` to find callers, type definitions, configuration keys, CRD fields, CLI commands, and tests.
- Read package files such as `go.mod`, `Makefile`, `README.md`, `PROJECT`, `config/`, `api/`, `internal/`, and `cmd/` when relevant.
- For Go modules, inspect exported types/functions, controllers, reconcilers, tests, generated API types, and package comments.
- When documenting Go test or validation commands for a package tree, use a recursive package pattern such as `./path/to/package/...` by default so nested subpackages are included. Use a single-package path only when the sample intentionally excludes subpackages, and explain that narrower scope when it is not obvious.
- For Kubernetes controllers or operators, inspect CRDs/API types, reconcile loops, RBAC markers, owned resources, and manifests.

Trace behavior from entrypoints to side effects. Prefer concrete file references and observed code paths over inferred intent.

## README Content

Include sections that fit the module. Do not force every section if it would add empty or speculative content.

- Purpose: what the module does and why it exists.
- Scope: what is inside the directory and what is deliberately handled elsewhere.
- Architecture: main packages, components, controllers, commands, data types, or resources.
- Control flow: how requests, reconciliation, generation, or execution moves through the module.
- Usage: public APIs, CLI commands, configuration fields, examples, or integration points.
- Operations: deployment behavior, runtime assumptions, observability, failure modes, and dependencies.
- Development: how to test, regenerate, validate, or safely extend the module.
- Maintenance notes: invariants, common mistakes, important source-of-truth files, and coupling to other packages.

## Mermaid Diagrams

Include an embedded Mermaid diagram when it clarifies non-trivial control flow, reconciliation, build/generation pipelines, data movement, or resource ownership. Keep diagrams simple enough to maintain.

Use `flowchart TD` for most control flow and resource relationship diagrams. Use `sequenceDiagram` only when the ordering between actors is central to understanding behavior.

## Writing Standards

- Be detailed but concise. Prefer precise explanations over broad marketing language.
- Write for both users and developers; separate usage from implementation details when helpful.
- Name real files, packages, types, commands, CRDs, and configuration fields.
- Mark unknowns explicitly if the code does not answer them.
- Preserve existing README content when updating unless it is obsolete or contradicted by the code.
- Avoid documenting internal guesses as facts.

Before finishing, reread the README against the implementation and report any validation performed.
