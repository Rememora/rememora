use rusqlite::Connection;

use rememora::models::context::{self, InsertContext};
use rememora::models::session;

// ---------------------------------------------------------------------------
// MemoryBuilder
// ---------------------------------------------------------------------------

pub struct MemoryBuilder {
    name: String,
    project: Option<String>,
    category: String,
    importance: f64,
    content: String,
    abstract_text: String,
    overview: String,
    tags: Vec<String>,
    agent: String,
    accessed_days_ago: Option<i64>,
    access_count: Option<i64>,
}

/// Shorthand constructor: `memory("chose Zustand")` → MemoryBuilder with defaults.
pub fn memory(name: &str) -> MemoryBuilder {
    MemoryBuilder::new(name)
}

impl MemoryBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            project: None,
            category: "decision".to_string(),
            importance: 0.5,
            content: format!("Content for {name}"),
            abstract_text: format!("Abstract for {name}"),
            overview: format!("Overview for {name}"),
            tags: vec![],
            agent: "claude-code".to_string(),
            accessed_days_ago: None,
            access_count: None,
        }
    }

    pub fn project(mut self, project: &str) -> Self {
        self.project = Some(project.to_string());
        self
    }

    pub fn category(mut self, category: &str) -> Self {
        self.category = category.to_string();
        self
    }

    pub fn importance(mut self, importance: f64) -> Self {
        self.importance = importance;
        self
    }

    pub fn content(mut self, content: &str) -> Self {
        self.content = content.to_string();
        self
    }

    pub fn abstract_text(mut self, text: &str) -> Self {
        self.abstract_text = text.to_string();
        self
    }

    pub fn overview(mut self, text: &str) -> Self {
        self.overview = text.to_string();
        self
    }

    pub fn tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|t| t.to_string()).collect();
        self
    }

    pub fn agent(mut self, agent: &str) -> Self {
        self.agent = agent.to_string();
        self
    }

    /// Set updated_at to N days in the past (for hotness/decay testing).
    pub fn accessed_days_ago(mut self, days: i64) -> Self {
        self.accessed_days_ago = Some(days);
        self
    }

    /// Set the active_count (number of times this memory has been accessed).
    pub fn access_count(mut self, count: i64) -> Self {
        self.access_count = Some(count);
        self
    }

    /// Which project this memory belongs to (for auto-creation in given:: helpers).
    pub fn project_name(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// Insert into the database and return the generated ID.
    pub fn insert(&self, conn: &Connection) -> String {
        let slug = slug::slugify(&self.name);
        let (uri, parent_uri) = match &self.project {
            Some(proj) => (
                format!("rememora://projects/{proj}/memories/{}/{slug}", self.category),
                Some(format!(
                    "rememora://projects/{proj}/memories/{}",
                    self.category
                )),
            ),
            None => (
                format!("rememora://global/memories/{}/{slug}", self.category),
                Some(format!("rememora://global/memories/{}", self.category)),
            ),
        };

        let tags_json = serde_json::to_string(&self.tags).unwrap();

        let id = context::insert(
            conn,
            &InsertContext {
                uri,
                parent_uri,
                context_type: "memory".to_string(),
                category: Some(self.category.clone()),
                name: self.name.clone(),
                abstract_text: self.abstract_text.clone(),
                overview: self.overview.clone(),
                content: self.content.clone(),
                tags: tags_json,
                source_agent: Some(self.agent.clone()),
                source_session: None,
                importance: self.importance,
            },
        )
        .expect("Failed to insert test memory");

        // Post-insert: backdate updated_at for decay/hotness scenarios
        if let Some(days) = self.accessed_days_ago {
            let past = chrono::Utc::now() - chrono::Duration::days(days);
            conn.execute(
                "UPDATE contexts SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![past.to_rfc3339(), id],
            )
            .unwrap();
        }

        // Post-insert: override active_count
        if let Some(count) = self.access_count {
            conn.execute(
                "UPDATE contexts SET active_count = ?1 WHERE id = ?2",
                rusqlite::params![count, id],
            )
            .unwrap();
        }

        id
    }
}

// ---------------------------------------------------------------------------
// SessionBuilder
// ---------------------------------------------------------------------------

pub struct SessionBuilder {
    intent: String,
    agent: String,
    project: Option<String>,
    cwd: Option<String>,
    parent: Option<String>,
    summary: Option<String>,
    working_state: Option<String>,
    end_status: Option<String>,
}

/// Shorthand constructor: `session("implement auth")` → SessionBuilder with defaults.
pub fn session(intent: &str) -> SessionBuilder {
    SessionBuilder::new(intent)
}

impl SessionBuilder {
    pub fn new(intent: &str) -> Self {
        Self {
            intent: intent.to_string(),
            agent: "claude-code".to_string(),
            project: None,
            cwd: None,
            parent: None,
            summary: None,
            working_state: None,
            end_status: None,
        }
    }

    pub fn agent(mut self, agent: &str) -> Self {
        self.agent = agent.to_string();
        self
    }

    pub fn project(mut self, project: &str) -> Self {
        self.project = Some(project.to_string());
        self
    }

    pub fn cwd(mut self, cwd: &str) -> Self {
        self.cwd = Some(cwd.to_string());
        self
    }

    pub fn parent(mut self, parent_id: &str) -> Self {
        self.parent = Some(parent_id.to_string());
        self
    }

    pub fn ended(mut self) -> Self {
        self.end_status = Some("ended".to_string());
        if self.summary.is_none() {
            self.summary = Some("Completed".to_string());
        }
        self
    }

    pub fn transferred(mut self) -> Self {
        self.end_status = Some("transferred".to_string());
        if self.summary.is_none() {
            self.summary = Some("Transferred".to_string());
        }
        self
    }

    pub fn summary(mut self, summary: &str) -> Self {
        self.summary = Some(summary.to_string());
        self
    }

    pub fn working_state(mut self, state: &str) -> Self {
        self.working_state = Some(state.to_string());
        self
    }

    /// Insert into the database and return the session ID.
    pub fn insert(&self, conn: &Connection) -> String {
        let id = session::start(
            conn,
            &self.agent,
            self.project.as_deref(),
            self.cwd.as_deref(),
            &self.intent,
            self.parent.as_deref(),
        )
        .expect("Failed to start test session");

        if let Some(ref status) = self.end_status {
            session::end(
                conn,
                &id,
                self.summary.as_deref().unwrap_or("Completed"),
                self.working_state.as_deref(),
                Some(status.as_str()),
            )
            .expect("Failed to end test session");
        }

        id
    }
}
