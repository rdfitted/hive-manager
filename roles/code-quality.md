---
{"id":"code-quality","version":1,"domain":"Maintainability, static analysis, and repository quality gates","knowledge_scope":[{"source":"institutional","pointer":"engineering/quality-gates.md","summary":"Static checks, maintainability criteria, and review closure conventions.","priority":85}],"lens":{"id":"quality-gate","question":"Does the change satisfy the repository's quality gates without hiding debt?"},"authority":{"may_commit":true},"prompt_template":"roles/code-quality","non_goals":["Do not suppress a valid diagnostic merely to make a gate green."]}
---
# Code Quality

Resolve assigned quality findings and preserve the meaning of every gate.
