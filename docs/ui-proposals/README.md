# autoTrim UI proposals

Open `index.html` for three independently reviewable proposals. All runtime styles, scripts, icons, and fictional data are embedded in each proposal. Keep `current.html` beside them for the Current comparison.

- `01-overview.html`: physical RAM separate from swap; clearer advice hierarchy; quieter navigation and background status.
- `02-sessions-tabs.html`: readable names and projects; expandable technical details; search and state/profile filters; batch action scopes follow filtered results.
- `03-actions.html`: explicit target review; readable action history with recovery commands; Off / Preview only / On auto mode.
- `current.html`: the existing `tray/ui/index.html` renderer supplied with fictional sample data. Mutating Tauri commands are blocked.

To review through a browser on a shared origin:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory docs/ui-proposals
```

Open http://127.0.0.1:8765/. Choices and notes save to browser local storage. Export review downloads a Markdown file to share back in the task. The pages do not send decisions to Codex or alter the app. When opened directly with file URLs, browser storage sharing between files varies; localhost is preferred.

## Design direction

This is a refinement of a compact macOS utility. Preserve the existing system typeface, neutral backgrounds, green agent color, blue browser color, largest-first sidebar, and dense information structure.

Palette: canvas #f5f5f3, sidebar #ecece8, surface #ffffff, text #232522, action #2f6f4e, browser #466b9c. Dark appearance is included with brighter foreground accents. Type: the macOS system stack; 21px view titles, 13px interface copy, 11–12px secondary information, tabular figures. Left-align names and explanation; right-align comparable measurements.

Layout: retain the memory header above the sidebar and detail pane. Bring the information needed to decide into the main flow, with technical detail behind disclosure. The proposals keep the existing visual identity instead of introducing a new dashboard style.

```text
RAM use + compressed portion | swap on disk | CPU | freshness
Navigation / holders        | View title + concise context
                            | Relevant actions and readable rows
                            | Supporting detail
```

## Scope and tradeoffs

These files are design prototypes only; no production code, config, or daemon data changed. Preview actions change in-memory fictional records only. Data resets on reload. Most app navigation links lead to the proposal that owns that screen; unchanged screens open in the current UI reference.

The overview header is slightly taller. Lists require an extra click for technical information. Explicit confirmation occupies more space than the existing two-click button. Filter-scoped bulk actions change behavior and require careful implementation review if approved. The list preview focuses on the tab and session rows; implementation would retain site summaries, growth information, and quit/restart controls. Auto mode labels map to existing switches; the three-way control needs to preserve valid combinations when implemented.

The mock memory readings stay fixed during simulated closes; action history demonstrates the interaction outcome. Memory held before close is not reported as actual recovered memory. Per-tab memory is always estimated. Preview only does not close anything. The current renderer and proposed screens share fictional source data, not your live machine state.

Review the recommendations independently. Approval controls save preferences for discussion; they do not apply a patch. No browser analytics, remote fonts, external libraries, or remote assets are used.
