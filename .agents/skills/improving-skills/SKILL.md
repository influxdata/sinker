---
name: improving-skills
description: Collect feedback after using a project skill from .agents/skills/, when a used skill is unclear, incomplete, or incorrect, or when the user requests a skill review. Propose or apply scoped improvements grounded in the task just completed.
---

> **After completing tasks with this skill:** Invoke `improving-skills` to capture feedback and lessons learned.
> For this skill, include one self-review in the current feedback pass; do not recursively invoke it.

# Improving Skills

Use lessons from completed work to improve skill instructions and supporting resources. Keep feedback concrete and
proportional to the task, preserving guidance that worked well.

## When This Applies

- After completing a task that used a skill from the current project's `.agents/skills/` directory.
- When a skill used during the task was unclear, incomplete, or incorrect.
- When the user explicitly requests feedback on a skill, including one stored outside the repository.

Resolve skills to their actual locations and read applicable `AGENTS.md` instructions. Follow references relevant to the
observed issue. Do not assume that skills from another repository or skill framework are installed here.

## Workflow

1. Finish the user's task and any urgent follow-up first. Capture feedback while the evidence is fresh and include it
   in the task's final response.
2. Review the skills used and the decisions or workarounds they caused. Identify missing guidance, unclear instructions,
   incorrect assumptions, and useful guidance to retain. Ground findings in the completed work and verify proposed paths,
   commands, and behavior against the destination repository or available tools.
3. Check whether the reviewed skills include the feedback hook below. Recommend it for every project skill, including
   this one. Keep edits within the authorized scope; a missing hook elsewhere is a recommendation, not a reason to edit
   every skill in the repository.
4. Apply improvements already covered by the user's request or earlier authorization without asking again. Otherwise,
   prepare a concrete proposal before requesting approval for substantive edits outside that scope. Feedback collection
   alone does not authorize changes to unrelated skills or externally managed bundles.
5. Update affected supporting resources together with `SKILL.md`, preserving unrelated edits and invocation policy.
   Check frontmatter, references, and consistency across the changed bundle. Run focused checks for changed executable
   helpers; distinguish checks actually run from behavior verified by inspection.
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

## Useful Feedback

For each material finding, give the skill name and file, the observed issue or successful guidance, the proposed or applied
change, and a brief rationale. Quote existing text only when needed to make the change understandable. A short paragraph
or small diff is usually enough; combine related findings.

- Report concrete gaps, contradictory instructions, stale dependencies, and verified path or command errors.
- Keep guidance that helped the task, especially constraints tied to an observed failure mode.
- Fix obvious typos within an authorized edit without a separate proposal.
- Skip stylistic preferences and hypothetical future needs. Do not turn one task's details into universal requirements.
- For skills maintained outside the project, respect their ownership and update process. Propose changes unless the
  user's request or existing authorization covers editing that bundle.
