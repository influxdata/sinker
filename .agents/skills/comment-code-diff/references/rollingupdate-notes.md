# RollingUpdate Notes

Read this reference when the requested scope contains a supported third-party dependency deployed to a `ManagementCluster`, `ObservabilityCluster`, or `RegionalCluster`, or contains its committed `RollingUpdate` under `starfleet-controller/dist/manifests/management-cluster/config/rollingupdates/`.

## Scope and update-system contract

Begin from dependencies or `RollingUpdate` resources in the user's requested scope. Do not turn the task into a fleet-wide notes audit. For branch, multi-commit range, directory, module, or manifest-tree scopes, the corresponding canonical `RollingUpdate` is an eligible synchronization file even when it was not already changed. A request for one exact commit remains limited to files changed by that commit, and an explicit-file request remains limited to the named files; report an applicable note outside either scope instead of editing it.

Before writing notes, read:

- [Third-Party Dependency RollingUpdate Coverage](../../code-review/references/repository-review-contracts.md#third-party-dependency-rollingupdate-coverage) for the complete resource-coverage contract;
- `starfleet-controller/api/v1alpha1/rollingupdate_types.go` for admitted update, discovery, and target combinations and field semantics; and
- `starfleet-controller/internal/controller/rollingupdate/README.md` for discovery identities, approval behavior, notifier delivery, target mutation, and progress semantics.

Use the coverage contract to identify the applicable committed resource; do not create or repair a `RollingUpdate` as part of a documentation-only pass. Report non-note synchronization defects unless the user separately authorizes their repair. In particular:

- `RollingUpdate` manages supported dependencies maintained outside InfluxData, not internal dependencies maintained by InfluxData.
- A dependency normally has one resource selecting the complete union of applicable target types. Provider-specific regional dependencies use the admission-owned provider label; dependencies deployed to every provider use an unfiltered `RegionalCluster` target.
- Thanos is the sole one-resource-per-deployed-HelmRelease exception. Each document in `thanos.yaml` maps to one rendered `thanos-<key>` HelmRelease. Notes must be accurate for the particular component; repeat shared guidance only when every sentence applies to every document.
- Resource names and update types can be operational identities: Flux package and progress lookup use the `RollingUpdate` name, CAPI providers have fixed identities, k0smotron represents paired bootstrap and control-plane providers, and Kubernetes must be named `kubernetes`.
- Never introduce committed `.spec.selectedVersion`. It is runtime approval state changed by `fleetctl dependency approve`, not documentation or a manifest default. If the field is already committed, modifying the resource would trigger the review contract's requirement to remove it; do not make that behavior change under documentation-only authority. Report the blocker and request separate authorization before changing the resource.

## When notes are applicable

`.spec.notes` is optional Markdown passed to every notifier. The GitHub notifier appends it beneath its own `## Notes` heading in the approval issue; Slack links to that issue rather than copying the text.

Add notes when an operator needs dependency-specific information beyond the notifier's ordinary version, discovery, target, and concurrency context to decide whether or how to approve safely. Typical triggers include:

- required update ordering, prerequisites, supported version or skew constraints, or intermediate upgrades;
- CRD, API, configuration, storage, schema, or data migrations;
- breaking changes, removed features, irreversible steps, or a constrained rollback path;
- a reason to approve only an exact version or a narrower automatic-approval range;
- manual validation, soak time, or data-plane checks not represented by the configured progress adapter; or
- a documented incident, known incompatibility, temporary prerelease exception, security tradeoff, or exit condition that materially affects approval.

Update existing notes whenever scoped code, manifests, targets, update mechanics, incidents, or verified upstream requirements make them incomplete, stale, misleading, or applicable to the wrong dependency. Remove an obsolete statement when evidence shows it no longer applies. Omit `.spec.notes` when there is no durable, actionable guidance; generic filler is less useful than no note.

## Research the actual dependency

Trace the dependency from its `RollingUpdate` to every canonical deployment source and provider variant selected by its targets. Inspect relevant values, transformers, controllers, tests, component documentation, and `AGENTS.md` files. Search `docs/incidents/` by dependency name, resource name, alert, and failure signature; established contributing factors, rejected mitigations, recovery steps, and open follow-ups take precedence over generic advice.

Consult the dependency's official release notes, upgrade guide, compatibility or version-skew policy, and security notices when repository evidence does not fully establish the approval requirements. Prefer primary upstream sources, link directly to stable authoritative guidance when the link will help the operator, and do not turn an unverified assumption into an instruction. Verify every copied or shared sentence against the named dependency; nearby resources may have different upgrade ordering, version limits, targets, or failure modes.

Consider the dependency family without substituting family-wide boilerplate for specific evidence:

- For Kubernetes and k0s, check control-plane and worker skew, adjacent-version requirements,
  k0s revision handling, and the repository's comparable `X.Y.Z-k0s.N` approval identity. Trace
  rollout ordering through downstream controllers and their readiness gates, not only the
  `RollingUpdate` target mutation and progress adapters. Before asserting that sequencing is
  absent or blocking approval, distinguish what `RollingUpdate` orchestrates from what CAPI,
  k0smotron, or another target subsystem enforces, and corroborate the conclusion with tests,
  authoritative documentation, or user-provided operational evidence. Revision-sensitive
  back-pins should use an exact version because range discovery can over-select tags whose k0s
  revision is build metadata upstream.
- For CAPI Operator, core Cluster API, CAPA, CAPZ, and k0smotron, establish the supported compatibility matrix and required update order. State which resource the note governs. Remember that the k0smotron bootstrap and control-plane providers are one logical dependency updated together; do not paste core-provider instructions into an infrastructure provider or an unrelated chart without proving they apply.
- For networking, ingress, DNS, certificate, identity, and storage dependencies, examine availability blast radius, CRD or controller/node-agent sequencing, migration requirements, rollback behavior, and the health signal that proves the data plane still works. A `FluxHelmRelease` can report completion before a chart's data-plane rollout finishes, so document a separate check or soak only when the dependency's implementation or an incident establishes that need.
- For observability dependencies, determine whether the update affects collection, remote write, querying, alerting, or retained data and whether losing the component can hide the rollout's own failure. Keep per-component Thanos guidance aligned with the rendered HelmRelease it controls.
- For a temporary prerelease, pin, workaround, or known-bad-version exclusion, record why it exists, the evidence-backed condition for removing it, and what must change when that condition is met. Avoid time-relative wording that will silently become false.

## Write for the approver

Use a YAML block scalar and concise Markdown. The API limit is 32,768 characters, but useful notes should normally be much shorter. Do not add a `## Notes` heading because the GitHub notifier supplies it.

Make each instruction self-contained and actionable:

- name the affected dependency, target, or related component rather than relying on pronouns;
- state the required order or compatibility boundary and what evidence the operator should check;
- use the current command name, `fleetctl dependency approve <name>`, when approval mechanics are relevant;
- link a repository checklist, incident, or official upstream guide instead of copying a long procedure; and
- distinguish a mandatory safety gate from a recommendation or observation.

Do not merely restate `.spec.type`, discovery URLs or ranges, target selectors, or `maxConcurrentUpdates`; the approval issue already has that context. Do not mention the PR, reviewer, or comment-writing task. Do not include credentials, internal tokens, or other sensitive data. Keep maintainer-only explanations of rendering, naming, or target selection in nearby YAML `#` comments rather than notifier-facing notes.

## Verification

After changing canonical notes:

1. Run `make render` from `starfleet-controller/` and inspect the corresponding generated `RollingUpdate` snapshots; never hand-edit those snapshots.
2. Run `VALIDATE_YAML_CONCURRENCY=4 ./scripts/validate-yaml.sh --changed-only` from the repository root.
3. Run `git diff --check` and inspect the diff. Confirm the Markdown remains readable after YAML rendering, every factual instruction is supported, `.spec.notes` is the only intentional canonical YAML value changed by the documentation pass, and no modified resource commits `.spec.selectedVersion`.
