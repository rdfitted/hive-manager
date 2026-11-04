//! Label-scoped eligibility for institutional wiki references (wiki schema.md §11).
//!
//! Client pages never surface outside their own scope (Ryan, 2026-10-28). The
//! Claude prompt hook (`~/.claude/hooks/user_prompt_submit.py`) enforces the same
//! rule; `testdata/wiki_scope_parity.json` is the shared fixture that keeps the
//! two implementations from drifting.
//!
//! Eligibility for a project: `universal` + the scopes of the FIRST `repos[]`
//! rule whose `match` globs fit the repo (case-insensitive), then `oversight`
//! expansion, one-way: an office sees what it manages.
//!
//! A page's scope, strict order, first match wins (Ryan, 2026-11-04):
//! 1. an entity path rule (`clients/`, `partners/`, `pipeline/`,
//!    `agents/hunter/prospects/`, honouring `except`), whatever `pages{}` says;
//! 2. the label map's `pages{}` entry;
//! 3. the page's own frontmatter `scope:` (a page added after the last map build);
//! 4. otherwise unknown, and the caller excludes it.
//!
//! A missing or corrupt map keeps rules 1 (built-in entity patterns) and 3, with
//! universal-only eligibility: fail closed on client data, open on the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

/// Generated label map, relative to the institutional wiki root.
pub const LABEL_MAP_RELATIVE_PATH: &str = "derived/labels.json";
pub const UNIVERSAL_SCOPE: &str = "universal";
/// The hook's sentinel for rule 4; never an eligible scope.
pub const UNKNOWN_SCOPE: &str = "unknown";
/// Bytes of a page read for its frontmatter scope (the hook reads 8192).
pub const FRONTMATTER_HEAD_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathRule {
    pattern: String,
    scope: String,
    except: Vec<String>,
}

/// The entity path rules (the hook's `DEFAULT_PATH_RULES`), used when the map
/// is unavailable or carries no `path_rules`.
fn builtin_path_rules() -> Vec<PathRule> {
    let rule = |pattern: &str, scope: &str, except: &[&str]| PathRule {
        pattern: pattern.to_string(),
        scope: scope.to_string(),
        except: except.iter().map(|name| name.to_string()).collect(),
    };
    vec![
        rule("clients/*.md", "client:{stem}", &[]),
        rule("partners/*.md", "partner:{stem}", &[]),
        rule("pipeline/*.md", "pipeline:{stem}", &[]),
        rule(
            "agents/hunter/prospects/*.md",
            "pipeline:{stem}",
            &["README.md", "_TEMPLATE.md"],
        ),
    ]
}

/// The entity scope a page has by path alone (rule 1 with the built-in rules),
/// for callers with no label map at hand.
pub fn builtin_entity_scope(page: &str) -> Option<String> {
    WikiScope::without_map("").indexed_scope(page)
}

/// Eligibility filter for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiScope {
    /// `None` when the label map was missing or corrupt.
    page_scopes: Option<BTreeMap<String, String>>,
    path_rules: Vec<PathRule>,
    eligible: BTreeSet<String>,
    repo: String,
}

impl WikiScope {
    /// Load the map under `wiki_root` for `project_path`. Never fails: an
    /// unusable map yields the degraded scope plus the reason, for the caller
    /// to record.
    pub fn load(wiki_root: &Path, project_path: &Path) -> (Self, Option<String>) {
        let repo = repo_of(project_path);
        let path = wiki_root.join(LABEL_MAP_RELATIVE_PATH);
        let parsed = fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|content| {
                serde_json::from_str::<Value>(&content).map_err(|error| error.to_string())
            })
            .and_then(|map| Self::from_label_map(&map, &repo));
        match parsed {
            Ok(scope) => (scope, None),
            Err(error) => (
                Self::without_map(&repo),
                Some(format!("{}: {error}", path.display())),
            ),
        }
    }

    /// The degraded scope for a missing or corrupt map: built-in entity path
    /// rules, frontmatter scope, universal-only eligibility.
    pub fn without_map(repo: &str) -> Self {
        Self {
            page_scopes: None,
            path_rules: builtin_path_rules(),
            eligible: BTreeSet::from([UNIVERSAL_SCOPE.to_string()]),
            repo: repo.to_string(),
        }
    }

    /// Build eligibility from an already-parsed map for a repo name (the
    /// directory directly under a `Code Projects` root, or `""`).
    pub fn from_label_map(map: &Value, repo: &str) -> Result<Self, String> {
        let object = map.as_object().ok_or("label map is not an object")?;
        let pages = object
            .get("pages")
            .and_then(Value::as_object)
            .ok_or("label map has no 'pages' object")?;

        validate_structure(object)?;
        let page_scopes = pages
            .iter()
            .filter_map(|(page, entry)| {
                let scope = entry.get("scope")?.as_str()?;
                (!scope.is_empty()).then(|| (page.clone(), scope.to_string()))
            })
            .collect();

        // `label_map.get("path_rules") or DEFAULT_PATH_RULES`, as in the hook.
        let map_rules: Vec<PathRule> = object
            .get("path_rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|rule| PathRule {
                pattern: rule
                    .get("match")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                scope: rule
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or(UNKNOWN_SCOPE)
                    .to_string(),
                except: string_list(rule.get("except")),
            })
            .collect();
        let path_rules = if map_rules.is_empty() {
            builtin_path_rules()
        } else {
            map_rules
        };

        let mut eligible = BTreeSet::from([UNIVERSAL_SCOPE.to_string()]);
        if !repo.is_empty() {
            let repo_lower = repo.to_lowercase();
            for rule in object
                .get("repos")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let matched = string_list(rule.get("match"))
                    .iter()
                    .any(|glob| fnmatch(&glob.to_lowercase(), &repo_lower));
                if matched {
                    eligible.extend(string_list(rule.get("scopes")));
                    break;
                }
            }
        }
        // One pass in map order, like the hook. The generated map is sorted,
        // so serde_json's sorted object order is the file order.
        if let Some(oversight) = object.get("oversight").and_then(Value::as_object) {
            for (office, managed) in oversight {
                if eligible.contains(office) {
                    eligible.extend(string_list(Some(managed)));
                }
            }
        }

        Ok(Self {
            page_scopes: Some(page_scopes),
            path_rules,
            eligible,
            repo: repo.to_string(),
        })
    }

    /// Rules 1 and 2: the scope decided without opening the page.
    pub fn indexed_scope(&self, page: &str) -> Option<String> {
        let page = normalize_page(page);
        let name = page.rsplit('/').next().unwrap_or(page);
        for rule in &self.path_rules {
            if fnmatch(&rule.pattern, page) {
                if rule.except.iter().any(|except| except == name) {
                    // Rollups and templates are not entity pages: fall through
                    // to the remaining rules rather than assuming a scope.
                    break;
                }
                let chars: Vec<char> = name.chars().collect();
                let stem: String = chars[..chars.len().saturating_sub(3)].iter().collect();
                return Some(rule.scope.replace("{stem}", &stem));
            }
        }
        self.page_scopes.as_ref()?.get(page).cloned()
    }

    /// Rules 1-3 against a wiki on disk, reading the page's frontmatter only
    /// when the path rules and the map are silent (the hook's `page_scope`).
    #[cfg(test)]
    pub fn page_scope_in(&self, wiki_root: &Path, page: &str) -> Option<String> {
        if let Some(scope) = self.indexed_scope(page) {
            return known(scope);
        }
        let root = wiki_root.canonicalize().ok()?;
        let path = root.join(normalize_page(page)).canonicalize().ok()?;
        if !path.starts_with(&root) || path == root || !path.is_file() {
            return None;
        }
        let bytes = fs::read(path).ok()?;
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(FRONTMATTER_HEAD_BYTES)]);
        frontmatter_scope(&head)
    }

    /// Rules 1-3. `None` is rule 4: unknown, so the caller excludes the page.
    pub fn page_scope(&self, page: &str, frontmatter_scope: Option<&str>) -> Option<String> {
        match self.indexed_scope(page) {
            Some(scope) => known(scope),
            None => frontmatter_scope
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
                .and_then(known),
        }
    }

    pub fn is_eligible(&self, scope: &str) -> bool {
        self.eligible.contains(scope)
    }

    pub fn map_available(&self) -> bool {
        self.page_scopes.is_some()
    }

    #[cfg(test)]
    pub fn eligible_scopes(&self) -> &BTreeSet<String> {
        &self.eligible
    }

    #[cfg(test)]
    pub fn repo(&self) -> &str {
        &self.repo
    }
}

/// The final decision for one page, as the parity fixture records it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDecision {
    Eligible,
    ScopeExcluded,
    ScopeUnknown,
}

#[cfg(test)]
impl PageDecision {
    pub fn decide(scope: &WikiScope, page: &str, frontmatter_scope: Option<&str>) -> Self {
        match scope.page_scope(page, frontmatter_scope) {
            None => Self::ScopeUnknown,
            Some(page_scope) if scope.is_eligible(&page_scope) => Self::Eligible,
            Some(_) => Self::ScopeExcluded,
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::ScopeExcluded => "scope_excluded",
            Self::ScopeUnknown => "scope_unknown",
        }
    }
}

/// A map whose trusted tables are malformed is corrupt, not partially
/// usable: the caller degrades to built-in rules and universal-only
/// eligibility rather than trusting what it could parse.
fn validate_structure(object: &serde_json::Map<String, Value>) -> Result<(), String> {
    let is_string_list = |value: Option<&Value>| {
        value.map_or(true, |value| {
            value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
        })
    };
    let is_optional_string =
        |value: Option<&Value>| value.map_or(true, Value::is_string);
    if let Some(pages) = object.get("pages").and_then(Value::as_object) {
        if !pages
            .values()
            .all(|entry| entry.is_object() && is_optional_string(entry.get("scope")))
        {
            return Err("label map 'pages' has a malformed entry".to_string());
        }
    }
    if let Some(rules) = object.get("path_rules") {
        let valid = rules.as_array().is_some_and(|rules| {
            rules.iter().all(|rule| {
                rule.get("match").is_some_and(Value::is_string)
                    && is_optional_string(rule.get("scope"))
                    && is_string_list(rule.get("except"))
            })
        });
        if !valid {
            return Err("label map 'path_rules' is malformed".to_string());
        }
    }
    if let Some(repos) = object.get("repos") {
        let valid = repos.as_array().is_some_and(|repos| {
            repos.iter().all(|rule| {
                rule.get("match").is_some_and(Value::is_array)
                    && is_string_list(rule.get("match"))
                    && is_string_list(rule.get("scopes"))
            })
        });
        if !valid {
            return Err("label map 'repos' is malformed".to_string());
        }
    }
    if let Some(oversight) = object.get("oversight") {
        let valid = oversight
            .as_object()
            .is_some_and(|offices| offices.values().all(|managed| is_string_list(Some(managed))));
        if !valid {
            return Err("label map 'oversight' is malformed".to_string());
        }
    }
    Ok(())
}

fn known(scope: String) -> Option<String> {
    (scope != UNKNOWN_SCOPE).then_some(scope)
}

/// The hook's `page.split("#", 1)[0].lstrip("./")`.
pub fn normalize_page(page: &str) -> &str {
    page.split('#')
        .next()
        .unwrap_or_default()
        .trim_start_matches(['.', '/'])
}

/// The hook's `_frontmatter_scope` on a page's opening text: frontmatter
/// opened by `---`, closed by the next `\n---`, and the first line matching
/// `^scope:\s*['"]?([^'"\s#]+)`.
pub fn frontmatter_scope(text: &str) -> Option<String> {
    static SCOPE_LINE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let pattern = SCOPE_LINE.get_or_init(|| {
        regex::Regex::new(r#"(?m)^scope:\s*['"]?([^'"\s#]+)"#).expect("scope regex")
    });
    pattern
        .captures(&rest[..end])
        .map(|captures| captures[1].trim().to_string())
        .filter(|scope| !scope.is_empty())
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The directory directly under a `Code Projects` root, else `""`.
pub fn repo_of(project_path: &Path) -> String {
    // Split the string on both separators rather than using
    // `Path::components`: Hive Manager runs on Windows, but a Windows path
    // must resolve the same way on a Linux host (CI) too. `.` and `..` are
    // normalized like Python's `os.path.abspath`.
    let absolute =
        std::path::absolute(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let text = absolute.to_string_lossy();
    let mut parts: Vec<&str> = Vec::new();
    for segment in text.split(['\\', '/']) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment => parts.push(segment),
        }
    }
    for (index, part) in parts.iter().enumerate() {
        if part.to_lowercase() == "code projects" {
            if let Some(repo) = parts.get(index + 1) {
                return (*repo).to_string();
            }
        }
    }
    String::new()
}

/// Case-sensitive Python `fnmatch.fnmatchcase`: `*` and `?` also match `/`.
/// Callers lowercase both sides for the case-insensitive repo match.
pub(crate) fn fnmatch(pattern: &str, text: &str) -> bool {
    let mut regex = String::from(r"(?s)\A");
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        index += 1;
        match character {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' => {
                let mut end = index;
                if end < chars.len() && chars[end] == '!' {
                    end += 1;
                }
                if end < chars.len() && chars[end] == ']' {
                    end += 1;
                }
                while end < chars.len() && chars[end] != ']' {
                    end += 1;
                }
                if end >= chars.len() {
                    regex.push_str(r"\[");
                    continue;
                }
                let mut class: String = chars[index..end].iter().collect();
                index = end + 1;
                class = class.replace('\\', r"\\");
                for special in ['[', '&', '~', '|'] {
                    class = class.replace(special, &format!("\\{special}"));
                }
                if let Some(rest) = class.strip_prefix('!') {
                    class = format!("^{rest}");
                } else if class.starts_with('^') {
                    class = format!("\\{class}");
                }
                regex.push('[');
                regex.push_str(&class);
                regex.push(']');
            }
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push_str(r"\z");
    regex::Regex::new(&regex).is_ok_and(|compiled| compiled.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARITY: &str = include_str!("testdata/wiki_scope_parity.json");

    fn parity() -> Value {
        serde_json::from_str(PARITY).expect("parity fixture parses")
    }

    fn fixture_scope(repo: &str) -> WikiScope {
        WikiScope::from_label_map(&parity()["label_map"], repo).expect("fixture map")
    }

    /// Every case in the shared fixture, decided by this implementation. The
    /// prompt hook's tests read the same file.
    #[test]
    fn parity_fixture_decisions_match() {
        let fixture = parity();
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let repo = case["repo"].as_str().unwrap();
            let scope = if case["map"].as_str() == Some("missing") {
                WikiScope::without_map(repo)
            } else {
                fixture_scope(repo)
            };
            let decision = PageDecision::decide(
                &scope,
                case["page"].as_str().unwrap(),
                case["frontmatter_scope"].as_str(),
            );
            assert_eq!(decision.tag(), case["expect"].as_str().unwrap(), "case {name}");
        }
    }

    #[test]
    fn fnmatch_mirrors_python_semantics() {
        assert!(fnmatch("clients/*.md", "clients/tog.md"));
        assert!(fnmatch("clients/*.md", "clients/nested/tog.md"));
        assert!(!fnmatch("clients/*.md", "Clients/tog.md"));
        assert!(fnmatch("gvb-wt?", "gvb-wt1"));
        assert!(fnmatch("[!a]bc", "xbc"));
        assert!(!fnmatch("[!a]bc", "abc"));
        assert!(fnmatch("a[", "a["));
        assert!(fnmatch("a.b", "a.b"));
        assert!(!fnmatch("a.b", "axb"));
    }

    #[test]
    fn repo_is_the_directory_under_code_projects() {
        assert_eq!(repo_of(Path::new(r"D:\Code Projects\TOG Portal\src")), "TOG Portal");
        assert_eq!(repo_of(Path::new(r"C:\Users\x\code projects\hive-manager")), "hive-manager");
        assert_eq!(repo_of(Path::new(r"C:\Users\x\.t3\worktrees\hive-manager\a")), "");
        assert_eq!(repo_of(Path::new(r"D:\Code Projects")), "");
        assert_eq!(repo_of(Path::new("/home/ci/Code Projects/acme-portal/src")), "acme-portal");
        assert_eq!(repo_of(Path::new("D:/Code Projects/TOG Portal")), "TOG Portal");
        assert_eq!(
            repo_of(Path::new(r"D:\Code Projects\x\..\hive-manager\src")),
            "hive-manager"
        );
    }

    #[test]
    fn first_repo_rule_wins_case_insensitively() {
        let scope = fixture_scope("tog-second");
        assert!(scope.is_eligible("client:tog"));
        assert!(!scope.is_eligible("client:never-reached"));
        assert!(fixture_scope("OGMAINBRAIN").is_eligible("client:tog"));
    }

    #[test]
    fn path_rules_beat_a_mislabelled_map_entry() {
        let scope = fixture_scope("hive-manager");
        assert_eq!(scope.indexed_scope("clients/mislabelled.md").as_deref(), Some("client:mislabelled"));
    }

    #[test]
    fn missing_or_corrupt_maps_degrade_to_builtin_rules() {
        assert!(WikiScope::from_label_map(&serde_json::json!([]), "x").is_err());
        assert!(WikiScope::from_label_map(&serde_json::json!({"pages": []}), "x").is_err());
        for corrupt in [
            serde_json::json!({"pages": {}, "path_rules": [null]}),
            serde_json::json!({"pages": {}, "path_rules": [{"scope": "client:{stem}"}]}),
            serde_json::json!({"pages": {"clients/a.md": "client:a"}}),
            serde_json::json!({"pages": {}, "repos": [{"match": "TOG*", "scopes": ["client:tog"]}]}),
            serde_json::json!({"pages": {}, "oversight": {"client:a": "client:b"}}),
        ] {
            assert!(WikiScope::from_label_map(&corrupt, "TOG Portal").is_err(), "{corrupt}");
        }
        let missing = tempfile::tempdir().unwrap();
        let (scope, error) = WikiScope::load(missing.path(), missing.path());
        assert!(error.is_some());
        assert!(!scope.map_available());
        assert_eq!(scope, WikiScope::without_map(""));
        assert_eq!(scope.indexed_scope("partners/acme.md").as_deref(), Some("partner:acme"));
        assert_eq!(scope.indexed_scope("patterns/guide.md"), None);
    }

    /// Port of rdfitted/Claude-Code-Setup#114 `hooks/tests/test_wiki_scope_strict.py`:
    /// the same temp wiki, the same assertions, against this implementation.
    mod hook_strict_port {
        use super::super::*;
        use std::path::PathBuf;
        use tempfile::TempDir;

        fn page(wiki: &Path, rel: &str, scope: Option<&str>) {
            let path = wiki.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut frontmatter = vec![format!("title: {rel}"), "description: d".to_string()];
            if let Some(scope) = scope {
                frontmatter.push(format!("scope: {scope}"));
            }
            fs::write(path, format!("---\n{}\n---\n\nbody\n", frontmatter.join("\n"))).unwrap();
        }

        fn wiki() -> (TempDir, PathBuf) {
            let dir = TempDir::new().unwrap();
            let w = dir.path().join("wiki");
            page(&w, "practices/known.md", Some("universal"));
            page(&w, "clients/acme.md", Some("client:acme"));
            page(&w, "clients/globex.md", Some("client:globex"));
            page(&w, "practices/new-universal.md", Some("universal"));
            page(&w, "practices/new-client-note.md", Some("client:globex"));
            page(&w, "agents/hunter/prospects/README.md", Some("universal"));
            fs::create_dir_all(w.join("harness")).unwrap();
            fs::write(w.join("harness/notes.md"), "# infra, no frontmatter\n").unwrap();
            fs::write(w.join("index.md"), "# index\n").unwrap();
            let labels = serde_json::json!({
                "version": 1,
                "path_rules": [
                    {"match": "clients/*.md", "scope": "client:{stem}"},
                    {"match": "partners/*.md", "scope": "partner:{stem}"},
                    {"match": "pipeline/*.md", "scope": "pipeline:{stem}"},
                    {"match": "agents/hunter/prospects/*.md", "scope": "pipeline:{stem}",
                     "except": ["README.md", "_TEMPLATE.md"]}
                ],
                "repos": [{"match": ["acme*"], "scopes": ["client:acme"]}],
                "oversight": {},
                "pages": {
                    "practices/known.md": {"scope": "universal"},
                    "clients/acme.md": {"scope": "client:acme"},
                    "agents/hunter/prospects/README.md": {"scope": "universal"}
                }
            });
            fs::create_dir_all(w.join("derived")).unwrap();
            fs::write(w.join("derived/labels.json"), labels.to_string()).unwrap();
            (dir, w)
        }

        fn scope(w: &Path) -> (WikiScope, Option<String>) {
            WikiScope::load(w, Path::new(r"D:\Code Projects\acme-portal\src"))
        }

        fn page_ok(scope: &WikiScope, w: &Path, page: &str) -> bool {
            scope
                .page_scope_in(w, page)
                .is_some_and(|page_scope| scope.is_eligible(&page_scope))
        }

        #[test]
        fn own_client_page_kept_other_client_excluded() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert!(page_ok(&ws, &w, "clients/acme.md"));
            assert!(!page_ok(&ws, &w, "clients/globex.md"));
            assert!(page_ok(&ws, &w, "practices/known.md"));
        }

        #[test]
        fn stale_map_entity_page_still_isolated_by_path() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert_eq!(ws.page_scope_in(&w, "clients/globex.md").as_deref(), Some("client:globex"));
            assert!(!page_ok(&ws, &w, "clients/globex.md"));
        }

        #[test]
        fn unmapped_page_uses_frontmatter_scope() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert_eq!(ws.page_scope_in(&w, "practices/new-universal.md").as_deref(), Some("universal"));
            assert!(page_ok(&ws, &w, "practices/new-universal.md"));
            assert_eq!(
                ws.page_scope_in(&w, "practices/new-client-note.md").as_deref(),
                Some("client:globex")
            );
            assert!(!page_ok(&ws, &w, "practices/new-client-note.md"));
        }

        #[test]
        fn unknown_infra_page_excluded() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert_eq!(ws.page_scope_in(&w, "harness/notes.md"), None);
            assert!(!page_ok(&ws, &w, "harness/notes.md"));
            assert!(!page_ok(&ws, &w, "practices/does-not-exist.md"));
        }

        #[test]
        fn path_rule_except_falls_through_to_map() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert_eq!(
                ws.page_scope_in(&w, "agents/hunter/prospects/README.md").as_deref(),
                Some("universal")
            );
        }

        #[test]
        fn anchor_and_dot_prefix_normalized() {
            let (_dir, w) = wiki();
            let (ws, _) = scope(&w);
            assert_eq!(
                ws.page_scope_in(&w, "./clients/globex.md#history").as_deref(),
                Some("client:globex")
            );
        }

        #[test]
        fn missing_map_degrades_closed_on_client_data() {
            let (_dir, w) = wiki();
            fs::remove_file(w.join("derived/labels.json")).unwrap();
            let (ws, error) = scope(&w);
            assert!(error.is_some() && !ws.map_available());
            assert!(page_ok(&ws, &w, "practices/known.md"));
            assert!(!page_ok(&ws, &w, "clients/acme.md"));
            assert!(!page_ok(&ws, &w, "practices/new-client-note.md"));
            assert!(!page_ok(&ws, &w, "harness/notes.md"));
        }

        #[test]
        fn corrupt_map_degrades() {
            let (_dir, w) = wiki();
            fs::write(w.join("derived/labels.json"), "{not json").unwrap();
            let (ws, error) = scope(&w);
            assert!(error.is_some() && !ws.map_available());
            assert!(!page_ok(&ws, &w, "clients/globex.md"));
        }

        #[test]
        fn frontmatter_scope_mirrors_the_hook_regex() {
            assert_eq!(frontmatter_scope("---\nscope: universal # note\n---\n").as_deref(), Some("universal"));
            assert_eq!(frontmatter_scope("---\nscope: 'client:acme'\n---\n").as_deref(), Some("client:acme"));
            assert_eq!(frontmatter_scope("---\n  scope: universal\n---\n"), None);
            assert_eq!(frontmatter_scope("# no frontmatter\nscope: universal\n"), None);
            assert_eq!(frontmatter_scope("---\nscope: universal\n"), None);
        }
    }
}
