---
name: md-lint
description: >
  Lint markdown documents using markdownlint rules. This skill should run
  automatically every time you create or edit a markdown file. Invoke explicitly
  when the user asks to "lint markdown", "check markdown", "fix markdown",
  or "validate docs".
---

# Markdown Lint

Lint markdown files after every create or edit using markdownlint rules, respecting the project's `.github/.markdownlint.json` configuration.

## When to Run

Run this skill automatically:

- After creating any `.md` file
- After editing any `.md` file
- When the user explicitly asks to lint or validate markdown

## Steps

### 1. Read the project configuration

Read `.github/.markdownlint.json` to know which rules are disabled for this project. Current config:

```json
{
  "default": true,
  "MD013": false,
  "MD033": false,
  "MD041": false
}
```

This means:

- **MD013 disabled** — No line length limit enforced
- **MD033 disabled** — Inline HTML is allowed (tables, GitHub alerts, etc.)
- **MD041 disabled** — First line does not need to be a top-level heading

All other rules are **enabled by default**.

### 2. Check the document against enabled rules

For every markdown file you just created or edited, verify compliance with these rules (grouped by category):

#### Headings

- **MD001** — Heading levels increment by one (no skipping from `#` to `###`)
- **MD003** — Consistent heading style (use ATX `#` style throughout)
- **MD018** — Space after `#` in headings (`# Heading`, not `#Heading`)
- **MD019** — Single space after `#` (not `#  Heading`)
- **MD022** — Blank line before and after headings
- **MD024** — No duplicate heading text within the same nesting level
- **MD025** — Single top-level heading (`# Title`) per document (when MD041 is enabled — currently disabled)

#### Lists

- **MD004** — Consistent unordered list style (use `-` throughout)
- **MD005** — Consistent indentation for list items at the same level
- **MD007** — Unordered list indentation (2 spaces)
- **MD030** — Spaces after list markers (single space after `-`, `*`, or `1.`)
- **MD032** — Blank line before and after lists

#### Whitespace

- **MD009** — No trailing spaces at end of lines
- **MD010** — No hard tabs (use spaces)
- **MD012** — No multiple consecutive blank lines
- **MD027** — No multiple spaces after blockquote marker `>`
- **MD047** — Files should end with a single newline character

#### Links and Images

- **MD011** — No reversed link syntax (`[text](url)` not `(text)[url]`)
- **MD034** — No bare URLs (use `<url>` or `[text](url)`)
- **MD042** — No empty links (`[text]()` is invalid)
- **MD045** — Images should have alt text (`![alt](image.png)`)
- **MD053** — Link and image reference definitions must be used (no orphaned references)

#### Code

- **MD014** — Don't prefix all shell commands with `$` unless showing output
- **MD031** — Blank line before and after fenced code blocks
- **MD038** — No spaces inside code spans (`` `code` `` not `` ` code ` ``)
- **MD040** — Fenced code blocks should specify a language (` ```json ` not ` ``` `)
- **MD046** — Consistent code block style (use fenced ` ``` `, not indented)
- **MD048** — Consistent fenced code block style (use backticks `` ` ``, not tildes `~`)

#### Structure

- **MD028** — No blank line inside a blockquote (looks like separate blockquotes)
- **MD036** — Don't use emphasis instead of a heading (`**Not a heading**` on its own line)

#### Tables

- **MD055** — Consistent table pipe style (leading and trailing pipes)
- **MD056** — Table column count should be consistent across rows

### 3. Fix violations

When you find violations:

1. Fix them directly in the same edit pass — do not just report them
2. If a fix would change the meaning or intent of the content, note it to the user instead of auto-fixing
3. Common auto-fixes:
   - Add missing blank lines around headings, lists, and code blocks
   - Remove trailing whitespace
   - Remove consecutive blank lines (keep one)
   - Add language identifiers to fenced code blocks
   - Fix heading level increments
   - Ensure file ends with a single newline

### 4. Do NOT flag these (disabled rules)

- Long lines (MD013 is off) — do not wrap or warn about long lines
- Inline HTML (MD033 is off) — `<details>`, `<summary>`, GitHub `> [!NOTE]` alerts are all fine
- First line not being a heading (MD041 is off) — files can start with anything
