// Template engine module - infrastructure for future prompt template features
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use thiserror::Error;

use crate::pty::WorkerRole;
use crate::session::SessionType;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Template not found: {0}")]
    NotFound(String),
    #[error("Invalid template: {0}")]
    Invalid(String),
}

/// Context for rendering prompts
#[derive(Debug, Clone)]
pub struct PromptContext {
    pub session_id: String,
    pub project_path: String,
    pub task: Option<String>,
    pub variables: HashMap<String, String>,
}

impl Default for PromptContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            project_path: String::new(),
            task: None,
            variables: HashMap::new(),
        }
    }
}

/// Information about a worker for prompt rendering
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub id: String,
    pub role_label: String,
    pub role_type: String,
    pub cli: String,
    pub status: String,
    pub current_task: Option<String>,
}

/// Template engine for rendering role and queen prompts
pub struct TemplateEngine {
    templates_dir: PathBuf,
    builtin_templates: HashMap<String, String>,
}

impl TemplateEngine {
    /// Create a new template engine with the given templates directory
    pub fn new(templates_dir: PathBuf) -> Self {
        let mut engine = Self {
            templates_dir,
            builtin_templates: HashMap::new(),
        };
        engine.load_builtin_templates();
        engine
    }

    /// Load built-in templates
    fn load_builtin_templates(&mut self) {
        // Backend worker role template
        self.builtin_templates.insert("roles/backend".to_string(), r#"# Backend Worker Role

You are a Backend Worker in a multi-agent coding session.

## Your Responsibilities
- Implement server-side logic, APIs, and data models
- Work with databases, authentication, and business logic
- Coordinate with Frontend workers on API contracts

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator

## Current Assignment
{{task}}
"#.to_string());

        // Frontend worker role template
        self.builtin_templates.insert("roles/frontend".to_string(), r#"# Frontend Worker Role

You are a Frontend Worker in a multi-agent coding session.

## Your Responsibilities
- Implement UI components and user interactions
- Manage client-side state and data flow
- Coordinate with Backend workers on API contracts

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator

## Current Assignment
{{task}}
"#.to_string());

        // Coherence worker role template
        self.builtin_templates.insert("roles/coherence".to_string(), r#"# Coherence Worker Role

You are a Coherence Worker in a multi-agent coding session.

## Your Responsibilities
- Review code across all workers for consistency
- Ensure API contracts are properly implemented on both sides
- Check for integration issues and type mismatches
- Verify naming conventions and code style consistency

## Communication Protocol
- Review changes from other workers
- Report inconsistencies via coordination.log
- Suggest fixes to maintain coherence

## Current Assignment
{{task}}
"#.to_string());

        // Simplify worker role template
        self.builtin_templates.insert("roles/simplify".to_string(), r#"# Simplify Worker Role

You are a Simplify Worker in a multi-agent coding session.

## Your Responsibilities
- Review code for unnecessary complexity
- Suggest simplifications and refactoring opportunities
- Ensure code is maintainable and readable
- Remove dead code and unused dependencies

## Communication Protocol
- Review changes from other workers
- Report simplification opportunities via coordination.log
- Submit refactoring suggestions

## Current Assignment
{{task}}
"#.to_string());

        // Custom worker role template
        self.builtin_templates.insert("roles/custom".to_string(), r#"# Custom Worker Role

You are a Worker in a multi-agent coding session.

## Your Responsibilities
{{responsibilities}}

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator

## Current Assignment
{{task}}
"#.to_string());

        // Fusion worker prompt template
        self.builtin_templates.insert("fusion-worker".to_string(), r#"You are a Fusion worker implementing variant "{{variant_name}}".
Working directory: {{worktree_path}}
Branch: {{branch}}

## Your Task
{{task}}

## Rules
- Work ONLY within your worktree directory
- Commit all changes to your branch
- Do NOT interact with other variants
- When complete, update your task file status to COMPLETED
"#.to_string());

        // Fusion judge prompt template
        self.builtin_templates.insert("fusion-judge".to_string(), r#"You are the Judge evaluating {{variant_count}} competing implementations.

## Variants
{{variant_list}}

## Evaluation Process
1. For each variant, run: git diff fusion/{{session_id}}/base..fusion/{{session_id}}/[variant]
2. Review code quality, correctness, test coverage, pattern adherence
3. Write comparison report to: {{decision_file}}

## Report Format
# Evaluation Report
## Variant Comparison
| Criterion | Variant A | Variant B | ... |
## Recommendation
Winner: [variant name]
Rationale: [explanation]
"#.to_string());

        // Queen prompt for Hive sessions
        self.builtin_templates.insert("queen-hive".to_string(), r#"# Queen - Hive Session Orchestrator

You are the Queen agent orchestrating a Hive session with direct worker management.

## Your Workers

{{workers_list}}

## Coordination Protocol

1. **Assign tasks**: Send messages to workers via the coordination system
2. **Monitor progress**: Check coordination.log for updates
3. **Add workers**: Request additional workers if needed

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "http://localhost:18800/api/sessions/{{session_id}}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "http://localhost:18800/api/sessions/{{session_id}}/project-dna"
   ```

3. **Curate useful learnings** into `.ai-docs/project-dna.md` (manual edit):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### .ai-docs/ Structure
```
.ai-docs/
├── learnings.jsonl      # Raw learnings from all sessions
├── project-dna.md       # Curated patterns and conventions
├── curation-state.json  # Tracks curation state
└── archive/             # Retired learnings (after 50+ entries)
```

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights into `.ai-docs/project-dna.md`
3. After 50+ learnings, archive to `.ai-docs/archive/`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## Communication Format

To send a message to a worker, use this format:
```
@worker-id: Your task description here
```

The system will route your message to the correct worker.

## Current Task

{{task}}
"#.to_string());

        // Queen prompt for Swarm sessions
        self.builtin_templates.insert("queen-swarm".to_string(), r#"# Queen - Swarm Session Orchestrator

You are the Queen agent orchestrating a Swarm session with hierarchical planning.

## Your Planners

{{planners_list}}

## Coordination Protocol

1. **Delegate to planners**: Assign high-level tasks to domain planners
2. **Monitor progress**: Check coordination.log for updates from planners
3. **Coordinate cross-domain**: Handle dependencies between planner domains

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "http://localhost:18800/api/sessions/{{session_id}}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "http://localhost:18800/api/sessions/{{session_id}}/project-dna"
   ```

3. **Curate useful learnings** into `.ai-docs/project-dna.md` (manual edit):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### .ai-docs/ Structure
```
.ai-docs/
├── learnings.jsonl      # Raw learnings from all sessions
├── project-dna.md       # Curated patterns and conventions
├── curation-state.json  # Tracks curation state
└── archive/             # Retired learnings (after 50+ entries)
```

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights into `.ai-docs/project-dna.md`
3. After 50+ learnings, archive to `.ai-docs/archive/`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## Communication Format

To send a message to a planner, use this format:
```
@planner-id: Your high-level task description here
```

The planners will break down tasks and assign to their workers.

## Current Task

{{task}}
"#.to_string());

        // Planner prompt template
        self.builtin_templates.insert("planner".to_string(), r#"# Planner - {{domain}} Domain

You are a Planner agent managing the {{domain}} domain in a Swarm session.

## Your Workers

{{workers_list}}

## Your Responsibilities
- Break down high-level tasks from the Queen into specific work items
- Assign tasks to your workers based on their capabilities
- Monitor worker progress and report to the Queen
- Handle blockers within your domain

## Communication Protocol

1. **Receive tasks**: Get assignments from the Queen
2. **Assign to workers**: Use @worker-id: format to assign tasks
3. **Report progress**: Update the Queen on domain status

## Current Domain Task

{{task}}
"#.to_string());
    }

    /// Render a worker prompt from a role
    pub fn render_worker_prompt(
        &self,
        role: &WorkerRole,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template_name = format!("roles/{}", role.role_type.to_lowercase());
        let template = self.get_template(&template_name)?;

        let mut rendered = template.clone();

        // Replace task placeholder
        if let Some(ref task) = context.task {
            rendered = rendered.replace("{{task}}", task);
        } else {
            rendered = rendered.replace("{{task}}", "Awaiting task assignment");
        }

        // Replace custom variables
        for (key, value) in &context.variables {
            let placeholder = format!("{{{{{}}}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }

        Ok(rendered)
    }

    /// Render queen prompt for a session
    pub fn render_queen_prompt(
        &self,
        session_type: &SessionType,
        workers: &[WorkerInfo],
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template_name = match session_type {
            SessionType::Hive { .. } => "queen-hive",
            SessionType::Swarm { .. } => "queen-swarm",
            SessionType::Fusion { .. } => "queen-hive", // Use hive template for fusion
            SessionType::Solo { .. } => "queen-hive", // Solo has no queen, keep fallback template for compatibility
        };

        let template = self.get_template(template_name)?;
        let mut rendered = template.clone();

        // Build workers list
        let workers_list = self.format_workers_list(workers);
        rendered = rendered.replace("{{workers_list}}", &workers_list);

        // Also support planners_list for swarm
        rendered = rendered.replace("{{planners_list}}", &workers_list);

        // Replace session_id placeholder
        rendered = rendered.replace("{{session_id}}", &context.session_id);

        // Replace task placeholder
        if let Some(ref task) = context.task {
            rendered = rendered.replace("{{task}}", task);
        } else {
            rendered = rendered.replace("{{task}}", "Awaiting instructions");
        }

        // Replace custom variables
        for (key, value) in &context.variables {
            let placeholder = format!("{{{{{}}}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }

        Ok(rendered)
    }

    /// Render planner prompt
    pub fn render_planner_prompt(
        &self,
        domain: &str,
        workers: &[WorkerInfo],
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("planner")?;
        let mut rendered = template.clone();

        // Replace domain
        rendered = rendered.replace("{{domain}}", domain);

        // Build workers list
        let workers_list = self.format_workers_list(workers);
        rendered = rendered.replace("{{workers_list}}", &workers_list);

        // Replace task placeholder
        if let Some(ref task) = context.task {
            rendered = rendered.replace("{{task}}", task);
        } else {
            rendered = rendered.replace("{{task}}", "Awaiting task assignment from Queen");
        }

        Ok(rendered)
    }

    pub fn render_fusion_worker_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("fusion-worker")?;
        let mut rendered = template.clone();

        rendered = rendered.replace(
            "{{variant_name}}",
            context.variables.get("variant_name").map(String::as_str).unwrap_or("variant"),
        );
        rendered = rendered.replace(
            "{{worktree_path}}",
            context.variables.get("worktree_path").map(String::as_str).unwrap_or("."),
        );
        rendered = rendered.replace(
            "{{branch}}",
            context.variables.get("branch").map(String::as_str).unwrap_or(""),
        );

        if let Some(ref task) = context.task {
            rendered = rendered.replace("{{task}}", task);
        } else {
            rendered = rendered.replace("{{task}}", "Awaiting instructions");
        }

        Ok(rendered)
    }

    pub fn render_fusion_judge_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("fusion-judge")?;
        let mut rendered = template.clone();

        rendered = rendered.replace("{{session_id}}", &context.session_id);
        rendered = rendered.replace(
            "{{variant_count}}",
            context.variables.get("variant_count").map(String::as_str).unwrap_or("0"),
        );
        rendered = rendered.replace(
            "{{variant_list}}",
            context.variables.get("variant_list").map(String::as_str).unwrap_or(""),
        );
        rendered = rendered.replace(
            "{{decision_file}}",
            context.variables.get("decision_file").map(String::as_str).unwrap_or(""),
        );

        Ok(rendered)
    }

    /// Format workers list for prompt
    fn format_workers_list(&self, workers: &[WorkerInfo]) -> String {
        if workers.is_empty() {
            return "No workers assigned yet.".to_string();
        }

        let mut lines = Vec::new();
        for worker in workers {
            let task_str = worker.current_task.as_deref().unwrap_or("-");
            lines.push(format!(
                "- **{}** ({}, {}): {} [{}]",
                worker.id, worker.role_label, worker.cli, worker.status, task_str
            ));
        }
        lines.join("\n")
    }

    /// Get a template by name
    fn get_template(&self, name: &str) -> Result<String, TemplateError> {
        // First check for custom template on disk
        let template_path = self.templates_dir.join(format!("{}.md", name));
        if template_path.exists() {
            return fs::read_to_string(template_path).map_err(TemplateError::from);
        }

        // Fall back to built-in template
        self.builtin_templates.get(name)
            .cloned()
            .ok_or_else(|| TemplateError::NotFound(name.to_string()))
    }

    /// Save a custom template
    pub fn save_template(&self, name: &str, content: &str) -> Result<(), TemplateError> {
        let template_path = self.templates_dir.join(format!("{}.md", name));

        // Ensure parent directory exists
        if let Some(parent) = template_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(template_path, content)?;
        Ok(())
    }

    /// List available templates
    pub fn list_templates(&self) -> Vec<String> {
        let mut templates: Vec<String> = self.builtin_templates.keys().cloned().collect();

        // Add custom templates from disk
        if let Ok(entries) = fs::read_dir(&self.templates_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".md") {
                        let template_name = name.trim_end_matches(".md").to_string();
                        if !templates.contains(&template_name) {
                            templates.push(template_name);
                        }
                    }
                }
            }
        }

        templates.sort();
        templates
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}
