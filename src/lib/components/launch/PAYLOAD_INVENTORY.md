# Launch payload inventory (T10)

Reference: base `14ffe2daa79f94c250ddef7f20b47e4fa774d981`, read-only payload scout plus principal audit. `LaunchDialog.svelte` owns `applyTemplate`, `buildWorkerRole`, and `handleSubmit`; `hiveLaunch.ts` owns the named Hive builder. The composer must reuse these paths rather than translate the backend contract again.

## Shared defaults

`sessionName`, `sessionColor`, `projectPath`, `prompt`, `soloTask` start empty. A nonblank project is required. Name is trimmed and becomes `undefined` when empty; color is `undefined` when empty; project keeps the selected path. Queen = `{cli:'claude',model:'opus',flags:[],label:'Queen'}`. Evaluator = `{cli:'claude',model:'opus',flags:[],label:'Evaluator'}`. QA defaults to UI, API, A11Y workers, each `{specialization,cli:'codex',model:'gpt-6.1-sol',flags:[]}`. Hive/Solo QA are enabled. Planning starts true. Smoke is false unless the Smoke Test action is used. Role/model defaults come from `config/clis.ts:defaultRoles`; no display-model alias is materialized by merely opening a control. Model & Effort defaults to Custom when the exact model/flags do not match a catalogue preset.

## Mode → builder and every emitted field

| Mode | Builder / event | Fields and defaults |
| --- | --- | --- |
| templates | No launch builder/event | Reject with `Choose a template before launching.` A selected template changes to its concrete mode and follows that mode's builder. |
| hive | `buildHiveLaunchConfig` → `launchHive` | `name`, `color`, `project_path`, `queen_config` as above; `workers` = one Coding Principal by default; `execution_policy` detailed below; `work_graph_archetype:null`; `work_graph_parameters:{}`; `prompt: prompt \|\| undefined` (not trimmed); `with_planning:true`; `smoke_test:false`; `with_evaluator:true`; `evaluator_config` = evaluator when enabled, otherwise `undefined`; `qa_workers` = QA roster when enabled, otherwise `undefined`. |
| research | Inline `handleSubmit` → `launchResearch` | `name`, `color`, `project_path`, `queen_config`; `workers` = two researchers; `prompt: prompt \|\| undefined`; `with_planning:false`; `with_evaluator:false`; `smoke_test:false`. No execution policy, evaluator config, QA roster, work graph, or session CLI fields. |
| solo | Inline `handleSubmit` → `launchSolo` | `name`, `color`; **`projectPath`** (camelCase); **`taskDescription: soloTask.trim() \|\| undefined`**; `cli:'claude'`; `model: soloConfig.model \|\| undefined` (initially absent); `flags:[]`; `with_evaluator:true`; `evaluator_config` and `qa_workers` conditional on Solo QA. No Queen, planning, smoke or execution policy. `sessions.launchSolo` later translates this UI event; the composer must retain the UI event shape. |
| fusion | Inline `handleSubmit` → `launchFusion` | `name`, `color`, `project_path`; `variants` = first two of A/B/C/D `{name:'Variant X',cli:'claude',flags:[]}` (model initially absent); `task_description:prompt` (empty string retained); `judge_config:{cli:'claude'}` (initially no flags/model/label); `queen_config`; `with_planning:true` regardless of the planning checkbox. No execution policy/workspace field. |
| debate | Inline `handleSubmit` → `launchDebate` | `name`, `color`, `project_path`; `debaters` = first two of A/B/C/D, `{name:'Debater X',stance:'Proponent'\|'Opponent'\|'Alternative A'\|'Alternative B',cli:'claude',flags:[]}`; `topic:prompt` (empty string retained; the separate `debateTopic` variable is unused); `rounds:3`; `judge_config:{cli:'claude',flags:[],label:'Debate Judge'}`; `queen_config`; `with_planning:true` initially; `default_cli:'claude'`; `default_model:undefined`. |

### Hive nested fields

`workers` maps every coding principal to `{cli,model,flags,label,role}`; default principal = `{cli:'codex',model:'gpt-6.1-sol',flags:[],label:'Coding Principal 1'}`. `role` = `{role_type:'principal',label:'Coding Principal',default_cli:principal.cli,prompt_template:null}` initially. Other roles use predefined labels/prompts; a custom role preserves its ID, title-cases its label and uses a null prompt unless overridden. Form-only `selectedRole`/`promptTemplateOverride` are not emitted as agent fields.

`execution_policy` = `{launch_kind:'hive',workspace_strategy:'shared_cell',queen_delegation:{mode:'auto',max_children:3,max_depth:1},principal_delegation:{mode:'encouraged',max_children:2,max_depth:1},tier_policy:{enabled:false,ceiling_percent:34,review_floor:'high',ladder:{}}}`. Disabled delegation emits only `{mode:'disabled'}`. Enabled limits clamp to at least 1 in the builder; the dialog validates whole-number ranges first. Tier-policy preview cells never enter the payload; ladder is always `{}`. A selected work-graph archetype is trimmed, forces planning true, and allows only trimmed nonempty parameter keys/values. No archetype discards stale overrides.

Research workers map to `{cli:'codex',model:'gpt-6.1-sol',flags:[],label:'Researcher N',role:{role_type:'researcher',label:'Researcher',default_cli:worker.cli,prompt_template:null}}`; explicit labels and prompt overrides survive.

## Every built-in session template fixture

Source: `src-tauri/src/templates/mod.rs:builtin_session_templates` (read-only); frontend contract: `types/domain.ts:SessionTemplate`. Exactly three built-ins exist; there are no Research/Solo/Debate built-ins.

| ID / name | Concrete builder | Overrides relative to defaults |
| --- | --- | --- |
| `bug-fix-hive` / Bug-fix Hive | Hive builder | Name = template name; Queen claude/opus; workers backend and frontend, both codex/gpt-6.1-sol, flags `[]`, labels Backend/Frontend, roles with `prompt_template:'roles/backend'` and `'roles/frontend'`; workspace shared_cell; no graph/parameters. |
| `feature-build-hive` / Feature-build Hive | Hive builder | As Bug-fix plus a coherence worker droid/glm-5.1, label Coherence, `prompt_template:'roles/coherence'`; workspace shared_cell; no graph/parameters. |
| `fusion-compare` / Fusion Compare | Fusion inline builder | Name = template name; candidates A codex/gpt-6.1-sol and B codex/gpt-6-luna, flags `[]`; resolver becomes `{cli:'claude',model:'opus',flags:[],label:'Fusion Resolver'}` judge and never a candidate. Template workspace isolated_cell is stored in form state but is **not** a Fusion payload field. |

`applyTemplate` treats the first case-insensitive Queen role as Queen, all remaining Hive cells as principals; no principals falls back to one default. Fusion uses `templates/templateLaunch.ts:routeFusionTemplateCells`: resolver/judge aliases route to judge, all other cells to variants; no judge preserves the existing judge; no candidates produces an error. Debate templates map all cells to Debater A/B/… with Stance A/B/… and preserve CLI/model. Template routing tests also cover candidates without a resolver and explicit `fusion_judge` aliases. These are routing fixtures, not additional built-in session templates.

Template cells' `prompt_template` values are **agent-role keys**, not task text. Legacy template application sets no project, task, color, QA/planning/delegation/tier config. The new presentation may seed an empty composer from the template's human description as required by handoff §8; for parity, compare the same resulting task input on both surfaces. Existing operator task text must survive template selection.

## Validation traps

Compare actual event details using independent expected fields, including absent versus `undefined` fields, not two aliases for one builder. Exercise nondefault prompt/count/path mappings, every mode, the Templates rejection, all three built-ins, custom Hive graph/roles, Fusion judge aliases, QA-off and advanced edits round-tripped through the composer. Mutation must remove a live composer-to-dialog mapping and fail parity; restore and rerun. Existing LaunchDialog has 7 cases and hiveLaunch has 9 at base; neither count may shrink.

## Composer QA and smoke reachability (#376)

Hive and Solo expose QA directly in the composer, bound to the same Advanced variables. QA remains on by default; off emits `with_evaluator:false` with no evaluator config or QA roster. The summary uses the live roster's count and uppercase specialisations, and its accessible description retains the browser caveat. Hive and Research expose the same secondary Smoke Test action as Advanced (`smoke_test:true`). Fusion, Debate and ordinary Research launch payloads are unchanged.
