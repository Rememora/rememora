import { invoke } from "../invoke.js";

const CATEGORY_COLORS = {
  preference: "var(--blue)",
  entity: "var(--green)",
  decision: "var(--yellow)",
  event: "var(--orange)",
  case: "var(--red)",
  pattern: "var(--pink)",
};

const BAR_COLORS = [
  "var(--accent)",
  "var(--blue)",
  "var(--green)",
  "var(--yellow)",
  "var(--orange)",
  "var(--pink)",
  "var(--red)",
];

export async function renderDashboard(container) {
  const [stats, recentMemories, sessions] = await Promise.all([
    invoke("get_dashboard_stats"),
    invoke("get_memories", { limit: 10 }),
    invoke("get_sessions", { limit: 5 }),
  ]);

  const maxProjectCount = Math.max(
    ...stats.by_project.map((p) => p.count),
    1
  );

  container.innerHTML = `
    <div class="dashboard-header">
      <h2>Dashboard</h2>
      <p>Overview of your memory system</p>
    </div>

    <div class="stat-cards">
      <div class="stat-card">
        <div class="label">Total Memories</div>
        <div class="value accent">${stats.total_memories}</div>
      </div>
      <div class="stat-card">
        <div class="label">Projects</div>
        <div class="value">${stats.by_project.length}</div>
      </div>
      <div class="stat-card">
        <div class="label">Categories</div>
        <div class="value">${stats.by_category.length}</div>
      </div>
      <div class="stat-card">
        <div class="label">Active Sessions</div>
        <div class="value">${stats.active_sessions}</div>
      </div>
    </div>

    <div class="charts-grid">
      <div class="chart-card">
        <h3>Memories by Project</h3>
        <div class="bar-chart">
          ${
            stats.by_project.length === 0
              ? '<div class="empty-state"><p>No memories yet</p></div>'
              : stats.by_project
                  .map(
                    (p, i) => `
                <div class="bar-row">
                  <span class="bar-label" title="${p.project}">${p.project}</span>
                  <div class="bar-track">
                    <div class="bar-fill" style="width: ${(p.count / maxProjectCount) * 100}%; background: ${BAR_COLORS[i % BAR_COLORS.length]}"></div>
                  </div>
                  <span class="bar-count">${p.count}</span>
                </div>
              `
                  )
                  .join("")
          }
        </div>
      </div>

      <div class="chart-card">
        <h3>Category Distribution</h3>
        <div class="category-grid">
          ${
            stats.by_category.length === 0
              ? '<div class="empty-state"><p>No categories yet</p></div>'
              : stats.by_category
                  .map(
                    (c) => `
                <div class="category-pill">
                  <span class="cat-count cat-${c.category}">${c.count}</span>
                  <span class="cat-name">${c.category}</span>
                </div>
              `
                  )
                  .join("")
          }
        </div>
      </div>
    </div>

    <div class="recent-section">
      <h3>Recent Memories</h3>
      ${
        recentMemories.length === 0
          ? '<div class="empty-state"><p>No memories stored yet. Use <code>rememora save</code> to create your first memory.</p></div>'
          : `
          <table class="memory-table">
            <thead>
              <tr>
                <th>Category</th>
                <th>Memory</th>
                <th>Project</th>
                <th>Importance</th>
                <th>Created</th>
              </tr>
            </thead>
            <tbody>
              ${recentMemories
                .map((m) => {
                  const project = extractProject(m.uri);
                  const cat = m.category || "uncategorized";
                  const created = formatDate(m.created_at);
                  return `
                    <tr data-id="${m.id}" class="memory-row" style="cursor: pointer">
                      <td><span class="badge badge-${cat}">${cat}</span></td>
                      <td class="abstract-text" title="${escapeHtml(m.abstract)}">${escapeHtml(m.abstract)}</td>
                      <td class="meta">${project}</td>
                      <td class="meta">${m.importance.toFixed(1)}</td>
                      <td class="meta">${created}</td>
                    </tr>
                  `;
                })
                .join("")}
            </tbody>
          </table>
        `
      }
    </div>

    <div class="recent-section">
      <h3>Recent Sessions</h3>
      ${
        sessions.length === 0
          ? '<div class="empty-state"><p>No sessions recorded yet.</p></div>'
          : sessions
              .map((s) => {
                const statusClass = `status-${s.status}`;
                return `
                <div class="session-card">
                  <div class="session-info">
                    <span class="session-agent">${escapeHtml(s.agent)}${s.project ? ` / ${escapeHtml(s.project)}` : ""}</span>
                    <span class="session-intent">${escapeHtml(s.intent || s.summary || "No description")}</span>
                  </div>
                  <div class="session-meta">
                    <span class="session-time">${formatDate(s.started_at)}</span>
                    <span class="status-badge ${statusClass}">${s.status}</span>
                  </div>
                </div>
              `;
              })
              .join("")
      }
    </div>
  `;

  // Attach click handlers for memory detail
  container.querySelectorAll(".memory-row").forEach((row) => {
    row.addEventListener("click", () => showMemoryDetail(row.dataset.id));
  });
}

async function showMemoryDetail(id) {
  const existing = document.querySelector(".detail-overlay");
  if (existing) existing.remove();

  const memory = await invoke("get_memory_detail", { id });
  if (!memory) return;

  const overlay = document.createElement("div");
  overlay.className = "detail-overlay";
  overlay.innerHTML = `
    <button class="close-btn">&times;</button>
    <h2>${escapeHtml(memory.name)}</h2>

    <div class="detail-field">
      <div class="field-label">Category</div>
      <div class="field-value"><span class="badge badge-${memory.category || "uncategorized"}">${memory.category || "uncategorized"}</span></div>
    </div>

    <div class="detail-field">
      <div class="field-label">Abstract</div>
      <div class="field-value">${escapeHtml(memory.abstract)}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Overview</div>
      <div class="field-value">${escapeHtml(memory.overview)}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Content</div>
      <div class="field-value mono">${escapeHtml(memory.content)}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">URI</div>
      <div class="field-value mono">${escapeHtml(memory.uri)}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">ID</div>
      <div class="field-value mono">${memory.id}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Importance</div>
      <div class="field-value">${memory.importance}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Access Count</div>
      <div class="field-value">${memory.active_count}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Source Agent</div>
      <div class="field-value">${memory.source_agent || "unknown"}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Created</div>
      <div class="field-value mono">${memory.created_at}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Updated</div>
      <div class="field-value mono">${memory.updated_at}</div>
    </div>
  `;

  overlay.querySelector(".close-btn").addEventListener("click", () => {
    overlay.remove();
  });

  document.body.appendChild(overlay);
}

function extractProject(uri) {
  const match = uri.match(/rememora:\/\/projects\/([^/]+)\//);
  return match ? match[1] : "global";
}

function formatDate(isoString) {
  if (!isoString) return "-";
  try {
    const d = new Date(isoString);
    const now = new Date();
    const diffMs = now - d;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return "just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString();
  } catch {
    return isoString.slice(0, 10);
  }
}

function escapeHtml(str) {
  if (!str) return "";
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
