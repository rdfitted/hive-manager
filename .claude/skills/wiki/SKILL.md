---
name: wiki
description: Read, create, and maintain a shared wiki using its index and schema.
---

# Work with a shared wiki

Use this skill for a shared wiki question or an authorized wiki edit. Resolve the wiki root in this order: nonempty `HIVE_WIKI_ROOT` for the current process, the saved `global_wiki_path` setting, then `~/.ai-docs/wiki`. Expand a leading `~` to the current user's home. Read `<root>/index.md` first and `<root>/schema.md` when present. A directory without `index.md` is not an initialized wiki.

## Start or navigate

- To start a new wiki, copy `docs/wiki-starter/` from the Hive Manager repository to the chosen root using the host file API or Node's built-in `node:fs` module. Preserve any existing files. The starter has `index.md`, `schema.md`, `log.md`, four category READMEs, and generic role knowledge templates.
- To answer a question, use the index to select a small set of relevant pages. Follow their links and source references. Verify stale or consequential claims against current evidence and state uncertainty plainly.
- Keep a project's `.ai-docs/` files in that project. A project `project-dna.md` pointer is not a page in the shared starter.

## Add or revise a page

1. Confirm the page's scope and locate any existing entry in `index.md` to avoid duplicates.
2. Read `schema.md`. If it is absent, use frontmatter with `title`, `category`, and `last_updated` in `YYYY-MM-DD` form. Put the page in the directory matching its category. The starter uses `patterns/`, `practices/`, `research/`, and `tools/`, plus role knowledge directories.
3. Write only claims supported by current sources. Separate observation, inference, and open questions. Keep project-specific details in the project layer.
4. Add or update the relative link in `index.md`; update the category README where it lists pages. Record a meaningful maintenance change in `log.md`.
5. Check frontmatter, links, and cited sources. Follow the wiki repository's existing branch and review workflow when it is versioned.

## Availability and privacy

An indexed local directory is sufficient for reading and local edits. Git enables local commits; a Git worktree with an `origin` remote and working `gh auth status` enables a remote pull request flow. If a prerequisite is missing, report the local result and do not claim a remote change was delivered.

If the wiki contains restricted material, configure `knowledge_wiki_folders` as an explicit allowlist of folders safe for Atlas to index. `null` means all discoverable folders and `[]` means none. This setting controls indexing; it does not replace filesystem access controls. Do not import restricted content into the starter or another project.
