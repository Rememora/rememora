import { invoke } from "@tauri-apps/api/core";
import { renderDashboard } from "./views/dashboard.js";
import { renderBrowse } from "./views/browse.js";
import { renderSearch } from "./views/search.js";

const content = document.getElementById("content");
let currentView = "dashboard";

// Navigation
document.querySelectorAll(".nav-link").forEach((link) => {
  link.addEventListener("click", (e) => {
    e.preventDefault();
    const view = link.dataset.view;
    if (view === currentView) return;

    document
      .querySelectorAll(".nav-link")
      .forEach((l) => l.classList.remove("active"));
    link.classList.add("active");
    currentView = view;
    renderView(view);
  });
});

async function renderView(view) {
  content.innerHTML = '<div class="loading">Loading...</div>';

  try {
    switch (view) {
      case "dashboard":
        await renderDashboard(content);
        break;
      case "browse":
        await renderBrowse(content);
        break;
      case "search":
        renderSearch(content);
        break;
      default:
        content.innerHTML =
          '<div class="empty-state"><h3>Unknown view</h3></div>';
    }
  } catch (err) {
    content.innerHTML = `<div class="empty-state"><h3>Error loading view</h3><p>${err}</p></div>`;
    console.error(err);
  }
}

// Initial render
renderView("dashboard");
