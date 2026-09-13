# Table redesign mockups

Open `index.html` to compare three directions, then open an individual HTML file.
The shared CSS and JavaScript are local; no dependencies or network services are needed.
The existing review server exposes this folder at http://127.0.0.1:8765/table-redesign/.

- **A — Quiet rows:** subtle dividers, readable columns, quiet row actions, inline details.
- **B — Grouped rows:** project/site/kind sections, subtotals, selection followed by one review action.
- **C — Compact list + details:** consistent compact rows and a selected-item inspector; stacked on narrow windows.

Every direction includes sessions, browser tabs, ports, and memory trends using the same
invented data. Search, sorting, filtering, checkboxes, details, and simulated closing work.
Protected rows are excluded from simulated close actions. Reset sample restores removed rows.
All actions run entirely in the preview; there is no Tauri bridge or connection to autoTrim.

Light/dark appearance, review decisions, and notes are stored locally in the browser.
Export review downloads Markdown for sharing decisions back to the task.
Production application source and the earlier approved proposals are unchanged by these mockups.
