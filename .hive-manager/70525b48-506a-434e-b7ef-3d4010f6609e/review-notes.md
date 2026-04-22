# Worker 5 Review Notes

## Findings

- MAJOR — [src-tauri/src/templates/mod.rs](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src-tauri/src/templates/mod.rs:372), [src-tauri/src/session/controller.rs](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src-tauri/src/session/controller.rs:2440), [src-tauri/src/session/controller.rs](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src-tauri/src/session/controller.rs:2609)
  The live evaluator prompt does not use the same `## Required Protocol` block as the three queen prompt builders. The queens all share `queen_required_protocol()`, but `build_evaluator_prompt()` renders `roles/evaluator`, whose five rules are different. That misses checklist item 3 outright and leaves the runtime evaluator/queen instructions out of sync.

- MINOR — [src-tauri/src/session/controller.rs](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src-tauri/src/session/controller.rs:4058)
  The queen master prompt still says `Always try the curl API first.` The Opus 4.7 hardening pass was supposed to replace remaining soft `try/should/might/consider` wording in queen/evaluator prompts with `MUST`/`MUST NOT` or justify it. This line is still soft guidance.

- MINOR — [src-tauri/src/session/controller.rs](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src-tauri/src/session/controller.rs:9491)
  The new coherence tests only check that prompts contain a few substrings; they do not assert that the required-protocol block is identical across the three queen builders and the live evaluator template, and they do not compare the worker/task scope surfaces for exact equality. The drift above therefore passes the new tests unchanged.

## Verified Clean

- `worktree_boundary_rules()` is wired into `build_fusion_worker_prompt()`, `build_worker_prompt()`, and `write_task_file_with_status()`, so the three worker/task scope surfaces share the same boundary wording.
- `queen_post_workers_protocol()` is shared by `build_queen_master_prompt()`, `build_fusion_queen_prompt()`, and `build_swarm_queen_prompt()`, so the 7-step post-workers flow, evaluator hard rule, and `/loop` ban are verbatim across those three queen prompts.
- The default `opus-4-6` literals at `src-tauri/src/session/controller.rs:673`, `:717`, and `:750` are unchanged.
- [src/lib/components/dashboard/KanbanCard.svelte](/D:/Code%20Projects/hive-manager/.hive-manager/worktrees/70525b48-506a-434e-b7ef-3d4010f6609e/worker-5/src/lib/components/dashboard/KanbanCard.svelte:77) preserves the left accent on hover by changing only `border-top-color`, `border-right-color`, and `border-bottom-color`.
- `git diff --check f2b766c..79c3eaf` is clean.

## Verification

- `cargo check --tests` failed in the environment before project code validation because `PATH` resolves `link.exe` to `C:\Program Files\Git\usr\bin\link.exe`, which rejects Rust's MSVC linker arguments.
- `pnpm run check` failed because `node_modules` is missing in this worktree, so `svelte-kit` is not available.
- The session HTTP API on `localhost:18800` was unavailable from this worktree, so I could not submit the mandatory learnings payload.
