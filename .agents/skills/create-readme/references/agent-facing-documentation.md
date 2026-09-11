# Agent-Facing Documentation

Use this guidance when authoring or improving skills, `AGENTS.md` files, and other instructions for agents. Apply the
summary, reference, and reuse patterns to general READMEs too, while retaining what human users and maintainers need.

## Write for the Agent's Task

Include information that changes how an agent selects, performs, or verifies work: scope, actionable instructions,
non-obvious constraints, source locations, relevant commands, and completion criteria. Assume the agent already has
general coding and reasoning abilities. Omit generic tutorials, repeated background, and human-oriented material that
does not help it complete the task; link to that material when it provides useful optional context.

State the condition under which an instruction applies and distinguish requirements from recommendations. Preserve
existing authorization boundaries and instruction scope. Summarizing a requirement must not weaken it or make it appear
optional. Keep essential constraints in the primary document, or require the relevant reference before the affected
action, so an agent cannot reasonably miss them.

## Keep Primary Documents Brief

Make the primary document an entry point: purpose and scope, essential instructions, a short workflow or orientation,
and links that explain when more detail is needed. Keep enough context to choose the next action without opening every
reference. Do not optimize for an arbitrary line limit or remove necessary instructions just to shorten the document.

- In `SKILL.md`, keep selection guidance, the common workflow, and shared constraints. Move substantial mode-specific
  procedures, schemas, examples, and troubleshooting into focused `references/` documents.
- In `AGENTS.md`, keep instructions that apply throughout its scope and links to task-specific guidance. Put detailed
  build, release, architecture, or subsystem procedures in maintained references. Preserve directory-specific scope when
  reorganizing instructions; moving a rule into a reference must not change where it applies.
- In `README.md`, retain a useful overview and common getting-started information. Summarize architecture, operations,
  and specialized workflows, linking to detail as needed. Preserve human usability and the user's requested depth.

Separate substantial topics when agents commonly need them for different tasks. Keep short, tightly related instructions
together when splitting would add navigation without saving meaningful context. The primary document and references
should form a usable path through the task, not require readers to reconstruct instructions from scattered fragments.

## Reuse Existing Sources

Before adding an explanation, search the target, relevant ancestor documents, and existing linked docs for coverage.
Prefer a brief summary and a link to the maintained source over copying instructions, command catalogs, schemas, or
background. Create a new reference only when the information has no suitable home, or move existing detail into one
and replace the original with a summary and link.

Keep each detailed topic in one authoritative location where reasonably possible. A short reminder or prerequisite may
be repeated when necessary to apply an instruction correctly, but avoid parallel versions of the same procedure. If
existing sources disagree, verify the underlying behavior and resolve the discrepancy within scope or report it; do not
silently choose a convenient version. Update affected links when moving content.

## Make References Selective and Discoverable

Link references directly from the primary document or the workflow step that needs them. Use descriptive link text and
state the trigger for reading, such as changing resource mapping, generating CRDs, or preparing a release. Avoid
instructions to read every reference before starting an unrelated task.

Keep each reference focused on a coherent topic, with its own scope and necessary prerequisites. Use paths relative to
the linking document and stable headings for links to specific sections. For long references, provide a short contents
list or useful search terms so agents can locate the needed section. Avoid deep chains of index documents. A reference
may link back for orientation, but following links must not create a mandatory reading or feedback loop.

## Include Self-Improvement Guidance

When creating or updating a skill or `AGENTS.md` within the authorized scope, use the hook guidance in
[improving-skills](../../improving-skills/SKILL.md): its
[skill feedback hook](../../improving-skills/SKILL.md#feedback-hook-for-project-skills) or
[AGENTS.md self-improvement section](../../improving-skills/SKILL.md#self-improvement-section-for-agentsmd).
Preserve equivalent hooks already present. Link to the feedback workflow instead of copying it into each document.

## Review the Result

Check that an agent can identify the applicable instructions and select the needed references from the primary
document alone. Verify relative links and section anchors, retained requirements and scope, and summaries against their
sources. Look for duplicated procedures and references that would force unrelated context into routine tasks. Preserve
accurate, useful information during extraction, and keep edits within the requested documentation scope.
