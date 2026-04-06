import { invoke } from "@tauri-apps/api/core";

let currentProject = null;
let currentCategory = null;

export async function renderBrowse(container) {
  const projects = await invoke("get_projects");

  container.innerHTML = `
    <div class="dashboard-header">
      <h2>Browse Memories</h2>
      <p>Filter and explore stored memories</p>
    </div>

    <div class="browse-controls">
      <select id="project-filter">
        <option value="">All Projects</option>
        ${projects
          .map(
            (p) =>
              `<option value="${escapeHtml(p.name)}" ${p.name === currentProject ? "selected" : ""}>${escapeHtml(p.name)}</option>`
          )
          .join("")}
      </select>

      <select id="category-filter">
        <option value="">All Categories</option>
        <option value="preference" ${currentCategory === "preference" ? "selected" : ""}>preference</option>
        <option value="entity" ${currentCategory === "entity" ? "selected" : ""}>entity</option>
        <option value="decision" ${currentCategory === "decision" ? "selected" : ""}>decision</option>
        <option value="event" ${currentCategory === "event" ? "selected" : ""}>event</option>
        <option value="case" ${currentCategory === "case" ? "selected" : ""}>case</option>
        <option value="pattern" ${currentCategory === "pattern" ? "selected" : ""}>pattern</option>
      </select>
    </div>

    <div id="browse-results">
      <div class="loading">Loading memories...</div>
    </div>
  `;

  const projectFilter = container.querySelector("#project-filter");
  const categoryFilter = container.querySelector("#category-filter");

  const loadMemories = async () => {
    currentProject = projectFilter.value || null;
    currentCategory = categoryFilter.value || null;

    const resultsDiv = container.querySelector("#browse-results");
    resultsDiv.innerHTML = '<div class="loading">Loading...</div>';

    try {
      const memories = await invoke("get_memories", {
        project: currentProject,
        category: currentCategory,
        limit: 100,
      });

      if (memories.length === 0) {
        resultsDiv.innerHTML =
          '<div class="empty-state"><h3>No memories found</h3><p>Try adjusting filters</p></div>';
        return;
      }

      resultsDiv.innerHTML = `
        <table class="memory-table">
          <thead>
            <tr>
              <th>Category</th>
              <th>Memory</th>
              <th>Project</th>
              <th>Importance</th>
              <th>Accesses</th>
              <th>Created</th>
            </tr>
          </thead>
          <tbody>
            ${memories
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
                    <td class="meta">${m.active_count}</td>
                    <td class="meta">${created}</td>
                  </tr>
                `;
              })
              .join("")}
          </tbody>
        </table>
        <p style="margin-top: 12px; font-size: 12px; color: var(--text-muted);">
          Showing ${memories.length} memories
        </p>
      `;

      // Click to show detail
      resultsDiv.querySelectorAll(".memory-row").forEach((row) => {
        row.addEventListener("click", () => showDetail(row.dataset.id));
      });
    } catch (err) {
      resultsDiv.innerHTML = `<div class="empty-state"><h3>Error</h3><p>${err}</p></div>`;
    }
  };

  projectFilter.addEventListener("change", loadMemories);
  categoryFilter.addEventListener("change", loadMemories);

  await loadMemories();
}

async function showDetail(id) {
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
