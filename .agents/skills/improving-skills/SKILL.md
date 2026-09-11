---
name: improving-skills
description: Review and improve skills and AGENTS.md instructions using task feedback. Use after a project skill or AGENTS.md feedback hook, when instructions caused an issue, or when the user requests a review or improvement. Keep changes scoped and grounded in observed work.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.
> For this skill, include one self-review in the current feedback pass; do not recursively invoke it.

# Improving Skills

Use lessons from completed work to improve skills, `AGENTS.md` instructions, and supporting resources. Keep feedback
concrete and proportional to the task, preserving guidance that worked well.

## When This Applies

- After completing a task that used a skill from the current project's `.agents/skills/` directory.
- When an applicable `AGENTS.md` self-improvement section calls for feedback after a task.
- When a skill or `AGENTS.md` instruction used during the task was unclear, incomplete, or incorrect.
- When the user explicitly requests review or improvement of a skill or `AGENTS.md`, including one outside the repository.

Resolve skills to their actual locations and read applicable `AGENTS.md` instructions. Follow references relevant to the
observed issue. Do not assume that skills from another repository or skill framework are installed here.

## Documentation Guidance

When improving skills or `AGENTS.md` files, use [create-readme](../create-readme/SKILL.md) for writing and verification
guidance and its [agent-facing documentation reference](../create-readme/references/agent-facing-documentation.md)
for audience, concise primary documents, selective references, and reuse of existing sources. Read only the supporting
topics relevant to the improvement. Using this guidance belongs to the current feedback pass; it does not start another
pass through the skills' completion hooks.

## Workflow

1. Finish the user's task and any urgent follow-up first. Capture feedback while the evidence is fresh and include it
   in the task's final response. An explicitly requested documentation improvement is itself the task to complete.
2. Review the skills and applicable `AGENTS.md` instructions used and the decisions or workarounds they caused. Identify
   missing guidance, unclear instructions, incorrect assumptions, and useful guidance to retain. Ground findings in the
   completed work and verify proposed paths, commands, and behavior against the destination repository or available tools.
3. Check the reviewed documents for the appropriate feedback hook below. Recommend it for project skills and
   `AGENTS.md` files. Keep edits within the authorized scope; a missing hook elsewhere is a recommendation, not a reason
   to edit every instruction file in the repository.
4. Apply improvements already covered by the user's request or earlier authorization without asking again. Otherwise,
   prepare a concrete proposal before requesting approval for substantive edits outside that scope. Feedback collection
   alone does not authorize changes to unrelated instruction files or externally managed bundles.
5. Update affected supporting resources together with `SKILL.md` or `AGENTS.md`, preserving unrelated edits and invocation
   policy. Check frontmatter, references, and consistency across the changed bundle. Run focused checks for changed
   executable helpers; distinguish checks actually run from behavior verified by inspection.
6. Report meaningful changes, remaining proposals, and validation briefly. If no change is warranted, mention what worked
   well without inventing an improvement. Include one review of `improving-skills` itself in this pass. That review
   satisfies its own feedback hook; finish without starting another feedback cycle solely because this skill ran.

## Feedback Hook for Project Skills

Recommend that all project skills, including `improving-skills` itself, include this instruction after the frontmatter
or in an equivalent completion section:

```markdown
> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.
```

Preserve equivalent existing instructions rather than duplicating them. When creating, migrating, or updating a project
skill within the user's requested scope, add the hook if missing. For `improving-skills`, keep the single-pass self-review
qualification shown above so the hook terminates.

## Self-Improvement Section for AGENTS.md

When creating or updating an `AGENTS.md` file within the user's requested scope, add a self-improvement section if no
equivalent instruction already applies. Use the following pattern, adjusting the link relative to that `AGENTS.md`:

```markdown
## Self-Improvement

After completing a task governed by this file, use
[improving-skills](.agents/skills/improving-skills/SKILL.md) to review the skills and AGENTS.md instructions used and
capture concrete feedback. Apply improvements within the authorized scope; propose changes outside it. Combine feedback
into one pass, including improving-skills' self-review, without recursively invoking completion hooks.
```

The example path is for a repository-root `AGENTS.md` with this project skill installed. Verify the actual location
before adding the link; do not create a broken dependency when the skill is unavailable. Preserve equivalent inherited
guidance without duplicating it in nested files. A missing section outside the requested scope is a recommendation.

## Useful Feedback

For each material finding, identify the skill or `AGENTS.md` file, the observed issue or successful guidance, the proposed
or applied change, and a brief rationale. Quote existing text only when needed to make the change understandable. A short
paragraph or small diff is usually enough; combine related findings.

- Report concrete gaps, contradictory instructions, stale dependencies, and verified path or command errors.
- Keep guidance that helped the task, especially constraints tied to an observed failure mode.
- Fix obvious typos within an authorized edit without a separate proposal.
- Skip stylistic preferences and hypothetical future needs. Do not turn one task's details into universal requirements.
- For skills maintained outside the project, respect their ownership and update process. Propose changes unless the
  user's request or existing authorization covers editing that bundle.
