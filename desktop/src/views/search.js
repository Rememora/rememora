import { invoke } from "../invoke.js";

export function renderSearch(container) {
  container.innerHTML = `
    <div class="dashboard-header">
      <h2>Search Memories</h2>
      <p>Full-text search across all stored memories</p>
    </div>

    <div class="search-box">
      <input type="text" id="search-input" placeholder="Search memories..." autofocus />
      <button id="search-btn">Search</button>
    </div>

    <div id="search-results">
      <div class="empty-state">
        <h3>Type a query to search</h3>
        <p>Uses FTS5/BM25 ranking for relevance</p>
      </div>
    </div>
  `;

  const input = container.querySelector("#search-input");
  const btn = container.querySelector("#search-btn");
  const resultsDiv = container.querySelector("#search-results");

  const doSearch = async () => {
    const query = input.value.trim();
    if (!query) return;

    resultsDiv.innerHTML = '<div class="loading">Searching...</div>';

    try {
      const results = await invoke("search_memories", {
        query,
        limit: 30,
      });

      if (results.length === 0) {
        resultsDiv.innerHTML =
          '<div class="empty-state"><h3>No results found</h3><p>Try different keywords</p></div>';
        return;
      }

      resultsDiv.innerHTML = results
        .map((r) => {
          const cat = r.context.category || "uncategorized";
          const project = extractProject(r.context.uri);
          return `
            <div class="search-result" data-id="${r.context.id}" style="cursor: pointer">
              <div class="result-header">
                <span class="result-name">
                  <span class="badge badge-${cat}">${cat}</span>
                  ${escapeHtml(r.context.name)}
                </span>
                <span class="result-rank">rank: ${r.rank.toFixed(4)} | ${project}</span>
              </div>
              <div class="result-abstract">${escapeHtml(r.context.abstract)}</div>
            </div>
          `;
        })
        .join("");

      resultsDiv.innerHTML += `
        <p style="margin-top: 12px; font-size: 12px; color: var(--text-muted);">
          ${results.length} results for "${escapeHtml(query)}"
        </p>
      `;

      // Click to show detail
      resultsDiv.querySelectorAll(".search-result").forEach((el) => {
        el.addEventListener("click", () => showDetail(el.dataset.id));
      });
    } catch (err) {
      resultsDiv.innerHTML = `<div class="empty-state"><h3>Search error</h3><p>${err}</p></div>`;
    }
  };

  btn.addEventListener("click", doSearch);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") doSearch();
  });
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
      <div class="field-label">Importance / Accesses</div>
      <div class="field-value">${memory.importance} / ${memory.active_count}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Source Agent</div>
      <div class="field-value">${memory.source_agent || "unknown"}</div>
    </div>

    <div class="detail-field">
      <div class="field-label">Created</div>
      <div class="field-value mono">${memory.created_at}</div>
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

function escapeHtml(str) {
  if (!str) return "";
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
