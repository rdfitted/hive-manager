---
title: Wiki schema
category: meta
last_updated: 2026-09-25
---

# Wiki schema

Use Markdown files with this minimal frontmatter:

```yaml
---
title: Descriptive page title
category: practices
last_updated: YYYY-MM-DD
---
```

`title` names the page, `category` names its section, and `last_updated` is the date its contents were last checked or changed. Use the `meta` category for root-level navigation and maintenance pages. For pages in a category directory, set `category` to that directory name.

## Placement

Put a page in the directory that best matches its subject. `patterns/` holds recurring situations, `practices/` holds procedures, `research/` holds source-backed findings, and `tools/` holds tool guidance. The `engineering/`, `orchestration/`, `planning/`, and `verification/` directories hold the role knowledge templates listed in the index. Create another category only when its purpose is clear.

## Registration and maintenance

Add each page to [index.md](index.md) with a relative link and a useful label. Update or remove its index entry when the page moves or disappears. Keep claims grounded in evidence, identify open questions, and update `last_updated` when you recheck a page. Record noteworthy maintenance in [log.md](log.md).
