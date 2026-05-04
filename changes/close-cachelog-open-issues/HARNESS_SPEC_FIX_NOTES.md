# Harness Spec Fix Notes

Context: `/harness run changes/close-cachelog-open-issues` failed after creating
`proposal.md`, `tasks.md`, and `specs/cachelog-cleanup/spec.md`.

Failure:

```text
Could not create harness run: requirement cachelog-cleanup-cleanup-handoff-state-is-current must contain SHALL or MUST normative text
```

What caused it:

- The harness did not accept the current Requirement body as containing normative
  text, even though it used `SHALL`.
- Current first requirement body:

```markdown
### Requirement: Cleanup handoff state is current
The repository SHALL contain a current cleanup handoff that matches the actual
working tree and does not claim missing, completed, or orphaned files incorrectly.
```

Likely parser constraint:

- The harness may require exact OpenSpec delta item formatting, not only a
  `### Requirement:` heading.
- It may expect requirement bodies to use a bullet marker such as
  `- SHALL ...` or a `#### Scenario:` block immediately after a normative line.
- It may not join wrapped lines before scanning for `SHALL` / `MUST`, or it may
  only inspect the first non-empty line after the heading.
- It may require the first body line after the heading to contain `SHALL` or
  `MUST` on a single line.

Immediate fix to try:

- Rewrite every Requirement so the first paragraph is one physical line and
  contains uppercase `SHALL` or `MUST`.
- Avoid wrapping the normative sentence across lines.
- Example:

```markdown
### Requirement: Cleanup handoff state is current
The repository SHALL maintain a cleanup handoff that matches the actual working tree and does not claim missing, completed, or orphaned files incorrectly.

#### Scenario: Handoff reflects current map split state
...
```

If that still fails:

- Try bullet format:

```markdown
### Requirement: Cleanup handoff state is current
- SHALL maintain a cleanup handoff that matches the actual working tree.
```

Skill update note for `/home/user/.codex/skills/hermes-harness-specs/SKILL.md`:

- Add a warning under Requirement Blocks:
  - "For Hermes harness compatibility, keep the normative Requirement sentence
    on the first non-empty line after `### Requirement:` and do not wrap it
    across physical lines."
  - "Prefer `The system SHALL ...` or `The repository SHALL ...` as a single
    line before scenarios."
- Add validation step:
  - "Before handoff, run or emulate a check that each `### Requirement:` block
    has `SHALL` or `MUST` on the first non-empty line after the heading."

Current artifact tree:

```text
changes/close-cachelog-open-issues/
  proposal.md
  tasks.md
  specs/cachelog-cleanup/spec.md
  HARNESS_SPEC_FIX_NOTES.md
```

Important command:

```bash
/harness run changes/close-cachelog-open-issues
```

Do not run harness against:

```bash
changes/close-cachelog-open-issues/specs/cachelog-cleanup
```

That nested path caused earlier errors:

```text
missing proposal.md; missing tasks.md; missing specs directory
```
