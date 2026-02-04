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

        // Queen prompt for Hive sessions
        self.builtin_templates.insert("queen-hive".to_string(), r#"# Queen - Hive Session Orchestrator

You are the Queen agent orchestrating a Hive session with direct worker management.

## Your Workers

{{workers_list}}

## Coordination Protocol

1. **Assign tasks**: Send messages to workers via the coordination system
2. **Monitor progress**: Check coordination.log for updates
3. **Add workers**: Request additional workers if needed

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
        };

        let template = self.get_template(template_name)?;
        let mut rendered = template.clone();

        // Build workers list
        let workers_list = self.format_workers_list(workers);
        rendered = rendered.replace("{{workers_list}}", &workers_list);

        // Also support planners_list for swarm
        rendered = rendered.replace("{{planners_list}}", &workers_list);

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
