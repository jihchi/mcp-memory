use crate::{db, okf::OkfOptions};
use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex, OnceLock};

pub const INSTRUCTIONS: &str = "\nMCP-Memory Agent Continuity Protocol:\n1. ON SESSION INITIALIZATION: Whenever you start working on a project or open a session, your VERY FIRST ACTION should be to call `memory_get_last` (or retrieve key 'system/last_memory') to inspect where work was last left off.\n2. ON MILESTONE / PROGRESS UPDATE: Whenever you accomplish a milestone, make key structural changes, or pause work, ALWAYS update the last memory checkpoint by calling `memory_update_last` (or storing key 'system/last_memory') with a brief summary and references to relevant memory concepts.\n";

fn default_namespace() -> String {
    "default".into()
}
fn default_limit() -> i64 {
    10
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Content {
    String(String),
    Object(Map<String, Value>),
}
impl From<Content> for Value {
    fn from(value: Content) -> Self {
        match value {
            Content::String(v) => Value::String(v),
            Content::Object(v) => Value::Object(v),
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Verified {
    Object(Map<String, Value>),
    List(Vec<Map<String, Value>>),
}
impl From<Verified> for Value {
    fn from(value: Verified) -> Self {
        match value {
            Verified::Object(v) => Value::Object(v),
            Verified::List(v) => Value::Array(v.into_iter().map(Value::Object).collect()),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectNamespace {
    #[schemars(description = "REQUIRED. Absolute path to active project root directory.")]
    pub project_root: String,
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateLastRequest {
    pub content: Content,
    #[schemars(description = "REQUIRED. Absolute path to active project root directory.")]
    pub project_root: String,
    #[serde(default = "default_namespace")]
    pub namespace: String,
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StoreRequest {
    pub key: String,
    pub content: Content,
    #[schemars(description = "REQUIRED. Absolute path to active project root directory.")]
    pub project_root: String,
    pub tags: Option<Vec<String>>,
    #[serde(default = "default_namespace")]
    pub namespace: String,
    pub concept_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource: Option<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
    pub sources: Option<Vec<Map<String, Value>>>,
    pub usage_window: Option<Map<String, Value>>,
    pub verified: Option<Verified>,
    pub generated_by: Option<String>,
    pub runtime: Option<String>,
    pub parameters: Option<Vec<Map<String, Value>>>,
    pub executor: Option<Map<String, Value>>,
    pub attester: Option<Map<String, Value>>,
    pub computation: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RetrieveRequest {
    pub key: String,
    #[schemars(description = "REQUIRED. Absolute path to active project root directory.")]
    pub project_root: String,
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    #[schemars(description = "REQUIRED. Absolute path to active project root directory.")]
    pub project_root: String,
    pub query: Option<String>,
    pub tags: Option<Vec<String>>,
    pub namespace: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

static MUTATION_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct MemoryServer {
    mutation_lock: Arc<Mutex<()>>,
}

impl Default for MemoryServer {
    fn default() -> Self {
        Self {
            mutation_lock: MUTATION_LOCK
                .get_or_init(|| Arc::new(Mutex::new(())))
                .clone(),
        }
    }
}

fn missing_root(root: &str) -> Option<String> {
    if root.trim().is_empty() {
        Some("Error: 'project_root' is required. Please provide the absolute path to your active project root directory.".into())
    } else {
        None
    }
}
fn maps_to_value(values: Option<Vec<Map<String, Value>>>) -> Option<Value> {
    values.map(|v| Value::Array(v.into_iter().map(Value::Object).collect()))
}

#[tool_router]
impl MemoryServer {
    #[tool(
        description = "AGENT DIRECTIVE - SESSION INITIALIZATION:\nCall this tool FIRST when starting work or opening a project session to retrieve the last recorded session checkpoint ('system/last_memory') and understand where work was left off."
    )]
    fn memory_get_last(&self, Parameters(req): Parameters<ProjectNamespace>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        match db::retrieve_memory("system/last_memory", &req.namespace, Some(&req.project_root)) {
            Ok(Some(record)) => format!("=== LAST SESSION CHECKPOINT (system/last_memory) ===\n\n{}", record.parsed.raw_payload),
            Ok(None) => "No previous 'system/last_memory' checkpoint found for this project.\nAgent Directive: Start fresh, and remember to update 'system/last_memory' via `memory_update_last` when accomplishing milestones.".into(),
            Err(error) => format!("Error retrieving last memory checkpoint: {error}"),
        }
    }

    #[tool(
        description = "AGENT DIRECTIVE - MILESTONE & SESSION CHECKPOINTING:\nCall this tool whenever completing a milestone, making key project changes, or pausing work to update the canonical project checkpoint ('system/last_memory') so future sessions know where work was left off."
    )]
    fn memory_update_last(&self, Parameters(req): Parameters<UpdateLastRequest>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        let _guard = self.mutation_lock.lock().unwrap_or_else(|e| e.into_inner());
        let options = OkfOptions {
            key: "system/last_memory".into(),
            content: req.content.into(),
            tags: vec![
                "last_memory".into(),
                "checkpoint".into(),
                "milestone".into(),
            ],
            namespace: req.namespace,
            concept_type: Some("Checkpoint".into()),
            title: Some("Last Session Checkpoint".into()),
            description: Some(req.summary.unwrap_or_else(|| {
                "Last recorded progress checkpoint and active project state.".into()
            })),
            status: Some("stable".into()),
            ..Default::default()
        };
        match db::store_memory(options, Some(&req.project_root)) {
            Ok(record) => format!(
                "Successfully updated last memory checkpoint ('system/last_memory').\nFile synced to: {}\n\nOKF Record Payload:\n{}",
                record.file_path.display(),
                record.okf_payload
            ),
            Err(error) => format!("Error updating last memory checkpoint: {error}"),
        }
    }

    #[tool(
        description = "Stores or updates a persistent memory record in Open Knowledge Format (OKF v0.2)."
    )]
    fn memory_store(&self, Parameters(req): Parameters<StoreRequest>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        let _guard = self.mutation_lock.lock().unwrap_or_else(|e| e.into_inner());
        let options = OkfOptions {
            key: req.key.clone(),
            content: req.content.into(),
            tags: req.tags.unwrap_or_default(),
            namespace: req.namespace.clone(),
            concept_type: req.concept_type,
            title: req.title,
            description: req.description,
            resource: req.resource,
            status: req.status,
            stale_after: req.stale_after,
            sources: maps_to_value(req.sources),
            usage_window: req.usage_window.map(Value::Object),
            verified: req.verified.map(Into::into),
            generated_by: req.generated_by,
            runtime: req.runtime,
            parameters: maps_to_value(req.parameters),
            executor: req.executor.map(Value::Object),
            attester: req.attester.map(Value::Object),
            computation: req.computation,
            ..Default::default()
        };
        match db::store_memory(options, Some(&req.project_root)) {
            Ok(record) => format!(
                "Successfully stored memory for key '{}' in namespace '{}'.\nFile synced to: {}\n\nOKF Record Payload:\n{}",
                record.key,
                record.namespace,
                record.file_path.display(),
                record.okf_payload
            ),
            Err(error) => format!("Error storing memory: {error}"),
        }
    }

    #[tool(description = "Retrieves a specific memory record by key.")]
    fn memory_retrieve(&self, Parameters(req): Parameters<RetrieveRequest>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        match db::retrieve_memory(&req.key, &req.namespace, Some(&req.project_root)) {
            Ok(Some(record)) => record.parsed.raw_payload,
            Ok(None) => format!(
                "Memory record with key '{}' not found in namespace '{}'.",
                req.key, req.namespace
            ),
            Err(error) => format!("Error retrieving memory: {error}"),
        }
    }

    #[tool(
        description = "Searches persistent memories matching keywords, tags, or namespace patterns."
    )]
    fn memory_search(&self, Parameters(req): Parameters<SearchRequest>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        match db::search_memories(
            req.query.as_deref(),
            req.tags.as_deref(),
            req.namespace.as_deref(),
            req.limit,
            Some(&req.project_root),
        ) {
            Ok(results) if results.is_empty() => "No matching memories found.".into(),
            Ok(results) => {
                let mut lines = vec![format!("Found {} memory record(s):\n", results.len())];
                for (index, result) in results.iter().enumerate() {
                    let tags = if result.tags.is_empty() {
                        "none".into()
                    } else {
                        result.tags.join(", ")
                    };
                    lines.push(format!("### {}. [{}] (Namespace: {})\n- **Tags**: {}\n- **Updated**: {}\n```markdown\n{}\n```\n", index + 1, result.key, result.namespace, tags, result.updated_at, result.okf_payload));
                }
                lines.join("\n")
            }
            Err(error) => format!("Error searching memories: {error}"),
        }
    }

    #[tool(description = "Removes a specific memory entry.")]
    fn memory_delete(&self, Parameters(req): Parameters<RetrieveRequest>) -> String {
        if let Some(error) = missing_root(&req.project_root) {
            return error;
        }
        let _guard = self.mutation_lock.lock().unwrap_or_else(|e| e.into_inner());
        match db::delete_memory(&req.key, &req.namespace, Some(&req.project_root)) {
            Ok(true) => format!(
                "Successfully deleted memory key '{}' from namespace '{}'.",
                req.key, req.namespace
            ),
            Ok(false) => format!(
                "Memory key '{}' not found in namespace '{}'.",
                req.key, req.namespace
            ),
            Err(error) => format!("Error deleting memory: {error}"),
        }
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("MCP-Memory", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
