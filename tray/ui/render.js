// Dashboard markup and DOM rendering.
import { icon, bytes, dur, esc, plural, AGENT_LABEL, epochNow, nowSecs, compactDuration, signedBytes } from "./format.js";
import { ACTIVITY_WINDOW, cleanupImpact, domainInactivityLabel, state, listModels, goneTab, goneSession, holders, holderByKey, appIconName, POLL_MS, sync, autoMode, autoModeWord, sessionKey, tabKey, sessionName, sessionCount, taskSearch, sessionProtection, reconcileList, isStale, sitesOf, hostnameOfTab, siteHostChoices, canCloseSession, canCloseTab, filteredSessions, filteredTabs } from "./state.js";
import { createActions } from "./actions.js";
const { refresh, navigate, reviewSessions, closeTabs, reviewPorts, wire } = createActions({ renderAll: (...a) => renderAll(...a), renderMain: (...a) => renderMain(...a), renderSide: (...a) => renderSide(...a), renderHeader: (...a) => renderHeader(...a) });
function renderHeader() {
  const sys = state.snap.system, total = sys.total_mem;
  const detail = [sys.wired != null ? `${bytes(sys.wired)} wired` : "", `up ${dur(sys.uptime_secs)}`].filter(Boolean).join(" · ");
  const memoryOpen = document.getElementById("memory-details")?.open;
  const memoryFocused = document.activeElement?.id === "memory-summary";
  document.getElementById("head").innerHTML = `
    <div class="app-brand" aria-label="autoTrim"><img class="brand-mark" src="logo.png" width="28" height="28" alt=""><span>autoTrim</span></div>
    <details class="mem" id="memory-details" ${memoryOpen ? "open" : ""}>
      <summary id="memory-summary" title="Physical RAM used / installed RAM. Includes compressed memory in RAM; excludes swap. Click for breakdown."><span class="muted">RAM used</span><b>${bytes(sys.used_mem)} / ${bytes(total)}</b>
        <span class="memory-chevron" aria-hidden="true"></span>
      </summary>
      <div class="memory-detail"><span><b>${bytes(total)}</b> installed RAM</span><span>${sys.compressed != null ? `Includes <b>${bytes(sys.compressed)}</b> compressed in RAM` : "Physical memory"}</span><span><b>${bytes(Math.max(0, total - sys.used_mem))}</b> outside used (includes cache)</span><span class="memory-system">Swap is stored on disk and is excluded from RAM used.</span><span class="memory-system">${esc(detail)}</span><span class="memory-system">App totals include helper processes. On macOS, footprints include compressed and swapped allocations at their original size; they do not add up to physical RAM used. Units use powers of 1024.</span></div>
    </details>
    <div class="stat" title="${esc(`${bytes(sys.used_swap)} of ${bytes(sys.total_swap)} swap in use on disk; excluded from RAM used`)}"><span>Swap (disk)</span><b>${bytes(sys.used_swap)}</b></div>
    <div class="stat" title="load ${sys.load_one.toFixed(1)} · ${sys.load_five.toFixed(1)} · ${sys.load_fifteen.toFixed(1)}"><span>CPU</span><b>${Math.round(sys.cpu_pct)}%</b></div>
    ${syncMarkup()}`;
  if (memoryFocused) document.getElementById("memory-summary").focus();
  document.getElementById("memory-details").onkeydown = event => {
    if (event.key === "Escape") { event.currentTarget.open = false; document.getElementById("memory-summary").focus(); }
  };
  document.getElementById("sync-btn").onclick = () => refresh({ fresh: true });
}

// ---- the refresh bar: age stays visible beside the manual scan control ----

function syncInfo() {
  const src = state.src, running = !!src.daemon_running;
  const shown = Math.max(0, nowSecs() - state.snap.taken_at);
  const period = Math.max(1, running ? src.interval_secs : POLL_MS / 1000);
  const age = running && src.snapshot_taken_at != null ? Math.max(0, nowSecs() - src.snapshot_taken_at) : shown;
  return { running, shown, period, age, left: Math.max(0, period - age), due: age >= period, busy: sync.scans > 0 };
}


function syncText(i) {
  return `${dur(i.shown)} ago`;
}

function syncTip(i) {
  const cadence = i.running ? `Background monitor samples every ${dur(i.period)}` : `Background monitor stopped; this window scans every ${dur(i.period)}`;
  return `${i.busy ? "Scanning… " : ""}Updated ${dur(i.shown)} ago. ${cadence}.`;
}

function syncMarkup() {
  const i = syncInfo();
  return `<div class="sync" id="sync-status" title="${esc(syncTip(i))}"><span class="txt" id="sync-txt">${esc(syncText(i))}</span><button id="sync-btn" class="${i.busy ? "busy" : ""}" ${i.busy ? "disabled" : ""} aria-label="${i.busy ? "Scanning" : "Refresh now"}" title="${i.busy ? "Scanning…" : "Refresh now"}">${icon("resume")}</button></div>`;
}
// Update the age once a second. Keep polling every second once the daemon's
// next snapshot is due so the age resets as soon as it lands.

function tickSync() {
  if (!state.snap || !state.src) return;
  const i = syncInfo();
  const txt = document.getElementById("sync-txt"), status = document.getElementById("sync-status");
  if (txt) txt.textContent = syncText(i);
  if (status) status.title = syncTip(i);
  const since = Date.now() - sync.lastPoll;
  if (!i.busy && (since >= POLL_MS || (i.running && i.due && since >= 1000))) refresh();
}



function holderIcon(h) {
  const data = state.appIcons.get(appIconName(h));
  const brand = /claude/i.test(h.name) ? 'claude' : /codex/i.test(h.name) ? 'codex' : '';
  return `<span class="app-icon${!data && brand ? ' brand-fallback ' + brand : ''}" aria-hidden="true">${data ? `<img src="${esc(data)}" alt="" width="32" height="32">` : brand ? brand === 'claude' ? '✳' : '⌘' : icon(h.kind)}</span>`;
}

function renderSide() {
  const s = state.snap;
  const focus = state.settings?.focus_areas || [];
  const hs = holders(s).sort((a, b) => Number(focus.includes(b.kind)) - Number(focus.includes(a.kind)) || b.rss - a.rss);
  const sel = k => state.view === k ? " sel" : "";
  const unmanaged = s.ports.filter(p => !p.owner_managed).length;
  let html = `<div class="nav-primary">
    <a class="item plain${sel("overview")}" data-view="overview">${icon("overview")}<span class="name">Overview</span><span class="mem">${s.advice.length ? `<span class="badge">${s.advice.length}</span>` : ""}</span></a>
    <a class="item plain${sel("ports")}" data-view="ports">${icon("ports")}<span class="name">Listening ports</span><span class="mem">${s.ports.length ? `<span class="badge" title="${unmanaged} unmanaged of ${s.ports.length} ports" aria-label="${unmanaged ? `${unmanaged} unmanaged ports` : `${s.ports.length} managed ports`}">${unmanaged || s.ports.length}</span>` : ""}</span></a>
    <a class="item plain${sel("actions")}" data-view="actions">${icon("history")}<span class="name">Actions</span><span class="mem"></span></a>
    <a class="item plain${sel("settings")}" data-view="settings">${icon("settings")}<span class="name">Settings</span><span class="mem">${state.update?.version ? "Update available" : autoModeWord()}</span></a>
    </div><div class="nav-holders"><h3>Apps <span style="font-weight:400;white-space:nowrap" title="Memory footprint, including helpers. On macOS, includes compressed and swapped memory; totals can exceed RAM used.">Footprint</span></h3>`;
  const LIMIT = 20;
  const shown = state.showAll ? hs : hs.slice(0, LIMIT);
  html += shown.map(h => `<a class="item${sel(h.key)}" data-view="${esc(h.key)}" title="${esc(h.name)} · ${bytes(h.rss)} memory footprint · ${plural(h.procs, "process")} including helpers · ${h.cpu.toFixed(1)}% CPU">
      ${holderIcon(h)}<span class="name">${esc(h.kind === "agent" ? h.name.replace(/ sessions$/, "") : h.name)}</span><span class="mem">${bytes(h.rss)}</span></a>`).join("");
  if (hs.length > shown.length) html += `<span class="more" data-more="1">${hs.length - shown.length} more…</span>`;
  else if (state.showAll && hs.length > LIMIT) html += `<span class="more" data-more="0">show fewer</span>`;
  html += `</div><div class="nav-status"><span class="${state.src.daemon_running ? "running" : ""}">${state.src.daemon_running ? '<i class="status-dot" aria-hidden="true"></i>Running in background' : "Background monitor stopped"}</span><a href="#settings" data-view="settings">Auto mode ${esc(autoMode(state.settings).replace("preview", "in preview"))}</a></div>`;
  const nav = document.getElementById("side");
  const scroll = nav.querySelector(".nav-holders")?.scrollTop ?? 0;
  nav.innerHTML = html;
  nav.querySelector(".nav-holders").scrollTop = scroll;
  nav.querySelectorAll("[data-view]").forEach(a => { a.href = "#" + encodeURIComponent(a.dataset.view); if (a.dataset.view === state.view) a.setAttribute("aria-current", "page"); a.onclick = e => { e.preventDefault(); navigate(a.dataset.view); }; });
  nav.querySelectorAll("[data-more]").forEach(a => { a.setAttribute("role", "button"); a.tabIndex = 0; a.onclick = () => { state.showAll = a.dataset.more === "1"; renderSide(); }; a.onkeydown = e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); a.click(); } }; });
}

// ---- views ----

function adviceTarget(a) {
  const s = state.snap;
  if (a.id === "stale_sessions") { const g = s.groups.find(g => g.kind === "agent" && s.sessions.some(x => x.state === "stale" && g.pids.includes(x.pid))); return g ? "g:" + g.name : null; }
  if (a.id === "old_servers") return "ports";
  if (a.id === "conversation_tabs") { const b = s.browsers.find(b => a.title.endsWith(" in " + b.name)); return b ? "g:" + b.name : null; }
  if (a.id.startsWith("page_growth:")) { const pid = +a.id.split(":")[2]; const b = s.browsers.find(b => (b.renderer_procs || []).some(r => r.pid === pid)); return b ? "g:" + b.name : null; }
  if (a.id === "browser_sprawl" || a.id === "heavy_app") { const g = s.groups.find(g => a.title.startsWith(g.name + " ")); return g ? "g:" + g.name : null; }
  if (a.id.startsWith("growth:g:") || a.id.startsWith("cpu:g:")) { const name = a.id.replace(/^(growth|cpu):g:/, ""); return s.groups.some(g => g.name === name) ? "g:" + name : null; }
  if (a.id.startsWith("growth:s:") || a.id.startsWith("cpu:s:")) { const pid = +a.id.split(":")[2]; const g = s.groups.find(g => g.kind === "agent" && g.pids.includes(pid)); return g ? "g:" + g.name : null; }
  return null;
}


function adviceCards(list) {
  if (!list.length) return `<div class="note">Nothing needs attention right now. autoTrim will keep monitoring.</div>`;
  return `<div class="advice-list">${list.map(a => {
    const target = adviceTarget(a);
    const estimate = ["browser_sprawl", "conversation_tabs"].includes(a.id);
    const recovery = a.recovery > 0 ? `<div class="impact"><b>${estimate ? "≈ " : ""}${bytes(a.recovery)}</b><span>${estimate ? "estimated memory" : "memory held"}</span></div>` : "";
    const label = target === "ports" ? "Review servers" : target && holderByKey(target)?.kind === "agent" ? "Review sessions" : target && holderByKey(target)?.browser ? "Review browser" : "View details";
    return `<div class="advice-row ${esc(a.severity)}"><div><div class="t">${esc(a.title)}</div><div class="e">${esc(a.evidence.join("\n"))}</div>${recovery}${!target ? `<div class="a">${esc(a.action)}</div>` : ""}</div>${target ? `<a data-goto="${esc(target)}" title="${esc(a.action)}">${label}</a>` : ""}</div>`;
  }).join("")}</div>`;
}


function observationCards(list) {
  return `<section class="observations" aria-label="Recent observations">${list.map(a => {
    const target = adviceTarget(a), holder = target && holderByKey(target);
    const label = holder?.browser ? "View browser" : holder?.kind === "agent" ? "View sessions" : target === "ports" ? "View servers" : "View details";
    return `<div class="observation">${icon("info")}<div><b>${esc(a.title)}</b><div class="e">${esc(a.evidence.join("\n"))}</div>${!target && a.action ? `<div class="e">${esc(a.action)}</div>` : ""}</div>${target ? `<a data-goto="${esc(target)}" title="${esc(a.action)}">${label}</a>` : ""}</div>`;
  }).join("")}</section>`;
}


function trendsTable(list, title = "Trends", scope = "trends") {
  if (!list.length) return "";
  const rows = list.map(t => ({ id: t.key, title: t.name, context: `${signedBytes(t.growth)} over ${dur(t.span_secs)}`, type: "Memory trend", metric: bytes(t.rss_now), metricLabel: "Memory footprint", facts: [["Change", signedBytes(t.growth)], ["Rate / hour", signedBytes(t.bytes_per_hour)], ["Mean CPU", t.cpu_mean.toFixed(1) + "%"], ["Window", dur(t.span_secs)], ["Samples rising", Math.round(t.rising_frac * 100) + "%"]], note: "Changes compare the start and end of the observation window. Memory growth is advice, never an automatic action." }));
  return `<h2>${esc(title)} <small>over the last ${dur(Math.max(...list.map(t => t.span_secs)))}</small></h2>` + compactList(scope, rows, { label: title, title: "Process / change", metric: "Now", noun: "trend" });
}

// ---- the two switches: auto mode, and running in the background ----

function controlCards() {
  if (!state.settings || !state.service) return `<p class="note">Settings could not be read from the app.</p>`;
  return `<div class="settings-sections">${autoCard()}${backgroundCard()}</div>`;
}

function pendingList(a) {
  const now = epochNow();
  return `<div class="pending">${a.pending.map(p => `<div><span class="l1" title="${esc(p.target + " · " + p.detail)}">${esc(p.target)} <span class="muted">· ${esc(p.detail)} · ${bytes(p.rss)}</span></span><span class="num state stale">${a.dry_run ? "would close" : "closing"} in ${dur(Math.max(0, p.due_at - now))}</span></div>`).join("")}</div>`;
}

// The file's settings are the switches; what the daemon is running with
// (the snapshot's `auto`) says whether they have been picked up yet and
// what is about to be closed.

function autoCard() {
  const c = state.settings, a = state.snap.auto, mode = autoMode(c), busy = state.settingsBusy;
  let html = `<div class="card settings-card"><div class="h">Auto mode <a data-goto="actions">View actions</a></div>`;
  if (c.config_error) html += `<p class="state stale">Settings could not be read; defaults are shown: ${esc(c.config_error)}</p>`;
  html += `<div class="mode-choices" role="group" aria-label="Auto mode">${[["off", "Off"], ["preview", "Preview only"], ["on", "On"]].map(([value, label]) => `<label class="mode-choice"><input type="radio" name="auto-mode" data-auto-mode="${value}" data-list-control="auto-mode-${value}" ${mode === value ? "checked" : ""} ${busy || c.config_error ? "disabled" : ""}><span>${label}</span></label>`).join("")}</div>`;
  html += `<p class="muted">${mode === "off" ? "Monitoring and advice continue. All closing stays manual." : mode === "preview" ? "Preview records what would have closed in Actions. Nothing is closed." : `Warns first, then waits ${dur(c.auto_grace_minutes * 60)}. Anything used during that time is spared.`}</p>`;
  const targets = [c.auto_close_sessions && "Agent sessions", c.auto_stop_servers && "Local servers", c.auto_close_tabs && "Listed websites", mode !== "off" && "Empty New Tabs"].filter(Boolean);
  html += `<details class="settings-disclosure" data-keep-open="cleanup-options"><summary>Cleanup options<span>${esc(targets.join(", ") || "Sessions, servers & Chrome tabs")}</span></summary><div class="settings-disclosure-body">`;
  for (const [key, label, checked, detail] of [["close_sessions", "Close idle agent sessions", c.auto_close_sessions, `Inactive for ${dur(c.stale_after_secs ?? 21600)} or more.`], ["stop_servers", "Stop idle local servers", c.auto_stop_servers, `Quiet dev servers open for ${dur(c.port_stale_after_secs ?? 86400)} or more.`]]) {
    html += `<div class="setting-row"><label><span>${label}<small class="muted" style="display:block">${detail}</small></span><input type="checkbox" data-auto="${key}" data-list-control="auto-${key}" ${checked ? "checked" : ""} ${busy || mode === "off" || c.config_error ? "disabled" : ""}></label></div>`;
  }
  html += domainSettings(c, mode, busy);
  html += `<details class="help" data-keep-open="auto-protection"><summary>What auto mode always keeps open</summary><p>Active sessions, app engines, the newest session in each project, and selected or pinned browser tabs. Only these session hosts are allowed: ${esc((c.auto_hosts || []).join(", ") || "none")}. Activity during the warning period cancels that target’s close.</p></details>`;
  html += `</div></details>`;
  if (!state.src.daemon_running) html += `<p class="note">The background monitor is stopped. Start it below for auto mode to run.</p>`;
  else if (a) {
    const applied = a.close_sessions === c.auto_close_sessions && a.stop_servers === c.auto_stop_servers && a.close_tabs === c.auto_close_tabs && a.dry_run === c.auto_dry_run && a.tab_rules_revision === c.tab_rules_revision;
    if (!applied || busy) html += `<p class="muted" role="status">Applying… the background monitor picks up changes on its next sample.</p>`;
    if (a.pending.length) html += pendingList(a) + (mode !== "off" ? `<div class="row2"><button data-auto-off ${busy ? "disabled" : ""}>Turn off auto mode</button><span class="muted">Cancels pending closes when the change is picked up.</span></div>` : "");
  }
  return html + `</div>`;
}

function domainSettings(c, mode, busy) {
  const rules = c.auto_tab_domains || [];
  const hours = c.auto_tab_inactive_hours ?? 24;
  const presets = [10 / 60, 20 / 60, 30 / 60, 45 / 60, 1, 2, 6, 12, 24, 48, 168];
  const choices = presets.includes(hours) ? presets : [...presets, hours].sort((a, b) => a - b);
  const blocked = busy || !!c.config_error;
  return `<section class="domain-settings" aria-labelledby="domain-settings-title">
    <div class="domain-section-head"><div><h3 id="domain-settings-title">Chrome tabs</h3><p class="muted">Empty New Tab pages are included whenever auto mode is enabled.</p></div></div>
    <div class="setting-row"><label><span>Close tabs from listed websites<small class="muted">Applies to all Chrome profiles.</small></span><input type="checkbox" data-auto-domain-target data-list-control="auto-domains" ${c.auto_close_tabs ? "checked" : ""} ${blocked ? "disabled" : ""}></label></div>
    <div class="domain-controls"><label>Close after<select data-domain-hours data-list-control="domain-hours" aria-label="Domain inactivity" ${blocked ? "disabled" : ""}>${choices.map(value => `<option value="${value}" ${value === hours ? "selected" : ""}>${esc(domainInactivityLabel(value))}${!presets.includes(value) ? " (custom)" : ""}</option>`).join("")}</select></label><span class="muted">of inactivity, plus a ${dur((c.auto_grace_minutes ?? 10) * 60)} warning.</span></div>
    <div class="domain-list" aria-label="Auto-close domains">${rules.length ? rules.map((rule, index) => `<div class="domain-row"><div><b>${esc(rule.domain)}</b><span>${rule.include_subdomains ? "Includes subdomains" : "Exact domain"}</span></div><div><button data-edit-domain="${index}" ${blocked ? "disabled" : ""}>Edit</button><button data-remove-domain="${index}" ${blocked ? "disabled" : ""}>Remove</button></div></div>`).join("") : `<div class="domain-empty"><b>No domains yet</b><span>Add a domain, then review its matching Chrome tabs before enabling cleanup.</span></div>`}</div>
    <div class="domain-actions"><button data-add-domain ${blocked ? "disabled" : ""}>Add domain</button><button class="primary" data-review-domains ${blocked || !rules.length ? "disabled" : ""}>Review matches</button></div>
    <p class="scope">Selected and pinned tabs stay open. Drafts, media, and uploads aren’t detected; unsaved content may be lost.</p>
    ${mode === "off" && rules.length ? `<p class="muted">Saved; Auto mode is off.</p>` : ""}
  </section>`;
}


function backgroundCard() {
  const sv = state.service, src = state.src, c = state.settings;
  let status, buttons = "", maintenance = "";
  if (!sv.supported) status = 'To monitor in the background, run <code>autotrim daemon</code>.';
  else if (sv.installed && sv.running) {
    status = "Starts at login and keeps working when the app is closed.";
    maintenance = `<button data-service="restart">Restart monitor</button><button data-service="uninstall">Stop and disable at login</button>`;
  } else if (sv.installed) {
    status = "Stopped. Start monitoring to resume reminders and auto mode.";
    buttons = `<button class="primary" data-service="restart">Start monitoring</button>`;
    maintenance = `<button data-service="uninstall">Disable at login</button>`;
  } else if (src.daemon_running) {
    status = "Running in a terminal. Enable startup to keep it running at login.";
    buttons = `<button data-service="install">Enable at login</button>`;
  } else {
    status = "Enable monitoring for trends, reminders, and auto mode.";
    buttons = `<button class="primary" data-service="install">Enable monitoring</button>`;
  }
  return `<div class="card settings-card"><div class="settings-line"><div><div class="h">Background monitoring <span class="tag ${src.daemon_running ? "on" : ""}">${src.daemon_running ? "Running" : "Stopped"}</span></div><p class="muted">${status}</p></div>${buttons}</div>
    <div class="setting-row"><label><span>Open window at startup</span><input type="checkbox" data-launch-window ${c.open_window_at_launch ? "checked" : ""}></label></div>
    ${maintenance || sv.binary || sv.pid ? `<details class="help" data-keep-open="service-details"><summary>Background service details</summary>${sv.pid || sv.binary ? `<p class="service-path">${sv.pid ? `PID: ${sv.pid}<br>` : ""}${esc(sv.binary || "")}</p>` : ""}<div class="row2">${maintenance}</div></details>` : ""}</div>`;
}


const isObservation = a => a.severity !== "high" && /^(growth:|cpu:|page_growth:)/.test(a.id);

function cleanupSummary({ linkToActions = false, showDetails = true } = {}) {
  const impact = cleanupImpact();
  const change = value => value === 0 ? "0 B" : signedBytes(value);
  const latest = impact.latest;
  const scope = impact.recorded === 1 ? "Latest action" : `Last ${plural(impact.recorded, "action")}`;
  const memory = impact.measured ? `${impact.estimated ? "≈ " : ""}${bytes(impact.footprint)}` : "—";
  return `<section class="cleanup-summary" aria-label="Recent cleanup">
    <div class="cleanup-heading"><h2>Recent cleanup</h2>${impact.recorded ? `<span>${scope}</span>` : ""}${linkToActions ? `<a data-goto="actions">View actions</a>` : ""}</div>
    ${impact.closed ? `<dl class="cleanup-stats">
      <div class="cleanup-footprint"><dt>Memory before closing</dt><dd>${memory}</dd></div>
      <div><dt>Closed</dt><dd>${impact.closed}</dd></div>
      <div><dt>Auto-closed</dt><dd>${impact.automatic}</dd></div>
    </dl>` : `<p class="cleanup-empty">${impact.recorded ? "No completed closures yet." : "Close an item to see your cleanup activity."}</p>`}
    ${showDetails && (impact.closed || latest) ? `<details class="help cleanup-details" data-keep-open="cleanup-details"><summary>Details</summary>
      ${impact.closed ? `<p class="cleanup-note">Footprints before closing, not measured RAM savings.${impact.estimated ? " Includes tab estimates." : ""}${impact.measured < impact.closed ? ` Memory available for ${impact.measured} of ${impact.closed} items.` : ""} Reopened items may be counted again.</p>` : ""}
      ${latest ? `<div class="cleanup-observed"><span title="${esc(new Date(latest.after.taken_at_ms).toLocaleString())}">Latest measured change${latest.tab_batch ? ` (${plural(latest.attempted_actions, "tab attempt")})` : ""}</span><span>RAM <b>${change(latest.after.used_mem - latest.before.used_mem)}</b></span><span>Swap <b>${change(latest.after.used_swap - latest.before.used_swap)}</b></span><small>Whole machine; includes other activity.</small></div>` : ""}
    </details>` : ""}
  </section>`;
}


function activityCharts() {
  const sys = state.snap.system, end = state.snap.taken_at;
  const history = state.activityHistory.length ? state.activityHistory : [{ taken_at: end, ...sys }];
  const valid = value => Number.isFinite(value) && value >= 0;
  const metrics = [
    { key: "used_mem", label: "RAM used", color: "ram", max: Math.max(1, sys.total_mem), format: bytes },
    { key: "cpu_pct", label: "CPU", color: "cpu", max: 100, format: value => `${value.toFixed(1)}%` },
    { key: "used_swap", label: "Swap used", color: "swap", max: Math.max(1024 ** 3, ...history.map(point => valid(point.used_swap) ? point.used_swap : 0)), format: bytes },
  ];
  return `<section class="overview-activity" aria-labelledby="activity-title">
    <div class="section-heading"><h2 id="activity-title">System activity</h2><span>Last 10 minutes · while open</span></div>
    <div class="activity-charts">${metrics.map(metric => {
      const points = history.filter(point => point.taken_at >= end - ACTIVITY_WINDOW && point.taken_at <= end);
      const segments = []; let segment = [];
      // A sleeping window or missing reading leaves a gap instead of a made-up line.
      const gap = Math.max(90, (state.src?.interval_secs || 30) * 3);
      points.forEach((point, index) => {
        if (!valid(point[metric.key]) || index && point.taken_at - points[index - 1].taken_at > gap) {
          if (segment.length) segments.push(segment);
          segment = [];
        }
        if (valid(point[metric.key])) segment.push([
          (4 + (point.taken_at - end + ACTIVITY_WINDOW) / ACTIVITY_WINDOW * 232).toFixed(2),
          (64 - Math.min(1, point[metric.key] / metric.max) * 56).toFixed(2),
        ]);
      });
      if (segment.length) segments.push(segment);
      const value = valid(sys[metric.key]) ? metric.format(sys[metric.key]) : "Unavailable";
      const description = `${metric.label}: ${value}. Last ten minutes of observed samples. Scale: zero to ${metric.format(metric.max)}.`;
      return `<figure class="activity-chart ${metric.color}"><figcaption><span>${metric.label}</span><b>${value}</b></figcaption>
        <div class="activity-scale">${metric.format(metric.max)}</div>
        <svg viewBox="0 0 240 72" role="img" aria-label="${esc(description)}">
          <path class="activity-grid" d="M4 8H236 M4 36H236 M4 64H236"/>
          ${segments.map(points => {
            const line = points.map(([x, y], index) => `${index ? "L" : "M"}${x} ${y}`).join(" ");
            const first = points[0], last = points.at(-1);
            return `${points.length > 1 ? `<path class="activity-fill" d="${line} L${last[0]} 64 L${first[0]} 64Z"/><path class="activity-line" d="${line}"/>` : ""}<circle class="activity-dot" cx="${last[0]}" cy="${last[1]}" r="2.5"/>`;
          }).join("")}
        </svg><div class="activity-axis"><span>10 min ago</span><span>Latest</span></div>
      </figure>`;
    }).join("")}</div>
    ${history.length < 2 ? `<p class="activity-hint">Collecting samples. Charts fill in as the dashboard refreshes.</p>` : ""}
  </section>`;
}

function viewOverview() {
  const s = state.snap, a = s.auto;
  const observations = s.advice.filter(isObservation), advice = s.advice.filter(a => !isObservation(a));
  const sessions = s.sessions.filter(x => !goneSession(x));
  const tabs = s.browsers.reduce((n, b) => n + b.tabs.filter(t => !goneTab(t)).length, 0);
  return `<div class="overview-heading"><div><h1>Overview</h1><p>Current workload and the few things that may need your attention.</p></div><span class="monitor-status ${state.src.daemon_running ? "on" : ""}">${icon(state.src.daemon_running ? "check" : "info")}${state.src.daemon_running ? "Monitoring" : "Manual scans"}</span></div>
    <dl class="overview-stats" aria-label="Current workload"><div><dt>Agent work</dt><dd>${sessionCount(sessions)}</dd></div><div><dt>Browser activity</dt><dd>${plural(tabs, "open tab")}</dd></div><div><dt>Memory holders</dt><dd>${plural(s.groups.length, "group")}</dd></div></dl>
    ${cleanupSummary({ linkToActions: true })}
    <section class="overview-focus"><div class="section-heading"><h2>Worth a look</h2><span>${advice.length ? plural(advice.length, "item") : "All clear"}</span></div>
      ${a && a.pending.length ? `<div class="card">${pendingList(a)}</div>` : ""}
      ${adviceCards(advice)}${observations.length ? observationCards(observations) : ""}
    </section>
    <details class="overview-memory" data-keep-open="overview-memory"><summary>Memory footprint breakdown</summary>${kindBar()}</details>
    ${activityCharts()}`;
}

// One bar for what the sidebar lists, added up by kind.

function kindBar(compact = false) {
  const sum = {};
  for (const h of holders(state.snap)) sum[h.kind] = (sum[h.kind] || 0) + h.rss;
  const total = Object.values(sum).reduce((n, v) => n + v, 0);
  if (!total) return "";
  const kinds = [["agent", "Agent sessions"], ["browser", "Browsers"], ["app", "Apps"], ["other", "Other processes"]].filter(([k]) => sum[k]);
  return `<section class="memory-mix ${compact ? "compact" : ""}"><div class="section-heading"><h2>${compact ? "Footprint mix" : "Memory footprint by kind"}</h2><span>${plural(state.snap.groups.length, "holder")}</span></div>
    ${compact ? "" : `<p class="help">Grouped process totals, including helpers. On macOS, these include compressed and swapped allocations at their original size and do not add up to physical RAM used.</p>`}
    <div class="kbar">${kinds.map(([k]) => `<i class="${k}" style="width:${(sum[k] * 100 / total).toFixed(1)}%"></i>`).join("")}</div>
    <div class="legend">${kinds.map(([k, l]) => `<span><i class="${k}"></i>${l} <b>${bytes(sum[k])}</b></span>`).join("")}</div><p class="memory-note">Total footprint: ${bytes(total)}. Bar shows each kind’s share of this total, not installed RAM.</p></section>`;
}


function viewSettings() {
  return `<h1>Settings</h1><div class="sub">Changes save automatically.</div><div class="settings-page">${controlCards()}<div class="card settings-card settings-line"><div><div class="h">Preferences</div><p class="muted">Interests, idle thresholds, and notifications.</p></div><button data-setup>Review setup</button></div>${updateCard()}</div>`;
}


function updateCard() {
  const u = state.update;
  if (!u) return '';
  const busy = state.updateBusy || ['checking', 'installing'].includes(u.phase);
  const messages = { idle: 'Checks automatically while the app is open.', checking: 'Checking for updates…', current: 'You’re up to date.', available: `Version ${u.version} is available.`, installing: 'Downloading and installing… autoTrim will restart when ready.', error: 'The update check failed. You can try again.' };
  return `<div class="card settings-card"><div class="settings-line"><div><div class="h">App updates <span class="tag">${esc(u.current_version)}</span></div>
    <p class="muted">${esc(u.enabled ? messages[u.phase] || '' : 'Updates are available in the installed macOS app.')}</p></div>
    ${u.enabled ? `<div class="settings-buttons"><button data-update="check" ${busy ? 'disabled' : ''}>Check for updates</button>${u.version ? `<button class="primary" data-update="install" ${busy ? 'disabled' : ''}>Install and restart</button>` : ''}</div>` : ''}</div>
    ${u.error ? `<p role="alert">${esc(u.error)}</p>` : ''}
    ${u.service_error ? `<p role="alert">${esc(u.service_error)}</p>` : ''}
    ${u.notes ? `<details class="help" data-keep-open="release-notes"><summary>What’s new in ${esc(u.version)}</summary><p style="white-space:pre-wrap">${esc(u.notes)}</p></details>` : ''}
  </div>`;
}


function rowStatus(r) {
  return `<span class="row-state ${esc(r.status || '')}"><i class="state-symbol ${esc(r.status || '')}" aria-hidden="true"></i>${esc(r.statusLabel || "Idle")}</span>`;
}

function compactInspector(r, label) {
  if (!r) return "";
  return `<section class="list-inspector" aria-label="${esc(label)} details">
    <div class="inspector-head"><h3>${esc(r.title)}</h3>${r.statusLabel ? rowStatus(r) : ""}</div>
    <dl><div><dt>${esc(r.metricLabel)}</dt><dd>${esc(r.detailMetric ?? r.metric)}</dd></div>${r.facts.map(([k,v]) => `<div><dt>${esc(k)}</dt><dd>${esc(v)}</dd></div>`).join("")}</dl>
    ${r.note ? `<p class="inspector-note">${esc(r.note)}</p>` : ""}
    ${r.action ? `<footer class="inspector-actions">${r.action}</footer>` : ""}</section>`;
}

function compactList(key, rows, options) {
  key = state.view + ":" + key;
  const saved = reconcileList(key, rows);
  listModels.set(key, { rows, options, saved });
  if (!rows.length) return `<div class="compact-empty">${esc(options.empty || "Nothing to show.")}</div>`;
  const eligible = rows.filter(r => r.eligible), chosen = eligible.filter(r => saved.selected.has(r.id));
  const controls = r => `data-list="${esc(key)}" data-row="${esc(r.id)}"`;
  const selectedMemory = chosen.reduce((n, r) => n + (r.rss || 0), 0);
  const stale = eligible.filter(r => r.status === "stale");
  return `<section class="compact-section" aria-label="${esc(options.label)}"><div class="compact-list"><table class="compact-table"><thead><tr>${options.review ? `<th class="check"><input type="checkbox" data-list-all="${esc(key)}" data-list-control="${esc(key)}:all" aria-label="Select all eligible ${esc(options.plural)}" ${chosen.length && chosen.length === eligible.length ? "checked" : ""} ${eligible.length ? "" : "disabled"}></th>` : ""}<th>${esc(options.title)}</th><th class="num metric">${esc(options.metric)}</th></tr></thead><tbody>${rows.map((r, index) => `<tr class="compact-row ${r.id === saved.inspected ? 'inspected' : ''} ${saved.selected.has(r.id) ? 'selected' : ''}">${options.review ? `<td class="check">${r.eligible ? `<input type="checkbox" data-list-select ${controls(r)} data-list-control="${esc(key + ':select:' + r.id)}" aria-label="Select ${esc(r.title)}" ${saved.selected.has(r.id) ? "checked" : ""}>` : `<span class="row-lock" title="${esc(r.protection)}" aria-label="${esc(r.protection)}">${icon("lock")}</span>`}</td>` : ""}<td><button class="row-title" data-list-inspect ${controls(r)} data-list-control="${esc(key + ':inspect:' + r.id)}" aria-expanded="${r.id === saved.inspected}" ${r.id === saved.inspected ? `aria-controls="${esc(key)}-detail-${index}"` : ""} title="${esc(r.title)}">${esc(r.title)}</button><span class="row-sub">${r.status ? `<i class="state-symbol ${esc(r.status)}" role="img" aria-label="${esc(r.statusLabel)}" title="${esc(r.statusLabel)}"></i>` : ""}${r.status === "stale" && compactDuration(r.idle) ? `<span class="stale-time" title="${esc(r.idleLabel)} ${esc(dur(r.idle))}${r.idleIsDuration ? "" : " ago"}" aria-label="${esc(r.idleLabel)} ${esc(dur(r.idle))}${r.idleIsDuration ? "" : " ago"}">${esc(compactDuration(r.idle))}</span>` : ""}<span class="row-context">${esc(r.context)}</span></span></td><td class="num metric">${esc(r.metric)}</td></tr>${r.id === saved.inspected ? `<tr class="compact-detail" id="${esc(key)}-detail-${index}"><td colspan="${options.review ? 3 : 2}">${compactInspector(r, options.noun)}</td></tr>` : ""}`).join("")}</tbody></table>${options.review ? `<div class="list-footer"><span>${chosen.length ? `${plural(chosen.length, options.noun)} selected${selectedMemory ? ` · ${options.estimated ? '≈ ' : ''}${bytes(selectedMemory)}${options.estimated ? ' estimated' : ''}` : ''}` : 'Click a row for details.'}</span><div class="list-actions">${chosen.length ? `<button data-list-clear="${esc(key)}">Clear</button><button class="primary" data-list-review="${esc(key)}">Review ${plural(chosen.length, options.noun)}</button>` : `${stale.length ? `<button data-list-stale="${esc(key)}">Review ${plural(stale.length, 'stale ' + options.noun)}</button>` : ''}<button data-list-eligible="${esc(key)}" ${eligible.length ? '' : 'disabled'}>Select eligible rows</button>`}</div></div>` : ""}</div></section>`;
}

// A single reading column: website/project groups, with details next to their item.
function resourceList(key, rows, options) {
  key = state.view + ":" + key;
  const saved = reconcileList(key, rows);
  saved.collapsed ??= new Set();
  const groups = new Map();
  for (const row of rows) {
    if (!groups.has(row.group)) groups.set(row.group, { id: row.group, label: row.groupLabel, rows: [] });
    groups.get(row.group).rows.push(row);
  }
  const query = (options.kind === "tab" ? state.tabFilter : state.sessFilter).trim();
  const searching = !!query;
  if (saved.query !== query) { saved.query = query; saved.searchCollapsed = new Set(); }
  saved.activeCollapsed = searching ? saved.searchCollapsed : saved.collapsed;
  const open = group => !saved.activeCollapsed.has(group.id);
  const visible = [...groups.values()].filter(open).flatMap(g => g.rows);
  const eligible = visible.filter(r => r.eligible);
  const eligibleIds = new Set(eligible.map(r => r.id));
  saved.selected = new Set([...saved.selected].filter(id => eligibleIds.has(id)));
  listModels.set(key, { rows: visible, allRows: rows, options, saved });
  const chosen = eligible.filter(r => saved.selected.has(r.id));
  const memory = chosen.reduce((sum, r) => sum + (r.rss || 0), 0);
  const controls = r => `data-list="${esc(key)}" data-row="${esc(r.id)}"`;
  const rowMarkup = (r, index) => {
    const expanded = saved.inspected === r.id, detailId = `${key}:detail:${index}`;
    const activity = r.status === "active" ? options.kind === "tab" ? "Active tab" : "Working now" : r.protection && !r.task ? r.protection : r.idle == null ? "Activity unknown" : `${compactDuration(r.idle)}${r.idleIsDuration ? " quiet" : " ago"}`;
    return `<li class="work-item"><div class="work-row ${saved.selected.has(r.id) ? 'selected' : ''}">
      <div class="work-check">${r.eligible ? `<input type="checkbox" data-list-select ${controls(r)} data-list-control="${esc(key + ':select:' + r.id)}" aria-label="Select ${esc(r.title)}" ${saved.selected.has(r.id) ? 'checked' : ''}>` : `<span class="row-lock" role="img" title="${esc(r.protection)}" aria-label="${esc(r.protection)}">${icon("lock")}</span>`}</div>
      <div class="work-content"><button class="row-title" data-list-inspect ${controls(r)} data-list-control="${esc(key + ':inspect:' + r.id)}" aria-expanded="${expanded}" ${expanded ? `aria-controls="${esc(detailId)}"` : ''} title="${esc(r.title)}">${esc(r.title)}</button><div class="row-context" title="${esc([r.context, r.location].filter(Boolean).join(' · '))}">${esc(r.context)}${r.location ? `<span class="work-location">${esc(r.location)}</span>` : ''}</div></div>
      <span class="work-activity ${esc(r.status)}" title="${esc(r.idleLabel)}${r.idle != null ? ': ' + dur(r.idle) : ''}">${esc(activity)}</span>
      <button class="work-info" data-list-inspect ${controls(r)} data-list-control="${esc(key + ':info:' + r.id)}" aria-label="Details for ${esc(r.title)}" aria-expanded="${expanded}">${icon("info")}</button>
      </div>${expanded ? `<div class="work-detail" id="${esc(detailId)}">${compactInspector(r, options.noun)}</div>` : ''}</li>`;
  };
  const filter = options.kind === "tab" ? state.tabState : state.sessState;
  const choices = options.kind === "tab" ? [["all", "All activity"], ["stale", "Stale tabs"], ["chat", "Chats"]] : [["all", "All activity"], ["stale", "Stale sessions"], ["active", "Active sessions"]];
  const searchOpen = state.searchOpen.has(state.view) || searching;
  const searchAttribute = options.kind === "tab" ? "data-tab-filter" : "data-session-filter";
  return `<section class="resource-list" aria-label="${esc(options.label)}">
    <div class="work-toolbar">${searchOpen ? `<label class="search">${icon("search")}<input type="search" ${searchAttribute} data-list-control="${esc(key)}:search" aria-label="${options.kind === 'tab' ? 'Search tabs' : 'Search sessions'}" placeholder="${options.kind === 'tab' ? 'Find a tab or website' : 'Find a task or project'}" value="${esc(options.kind === 'tab' ? state.tabFilter : state.sessFilter)}"></label>` : `<span class="work-count">${plural(groups.size, options.kind === 'tab' ? 'website' : 'project')} <span>${plural(rows.length, options.plural === 'tabs' ? 'tab' : 'item')}</span></span>`}
      <select class="activity-filter" data-activity-filter="${options.kind}" data-list-control="${esc(key)}:activity" aria-label="Filter by activity">${choices.map(([value, label]) => `<option value="${value}" ${filter === value ? 'selected' : ''}>${label}</option>`).join('')}</select></div>
    <div class="work-groups">${rows.length ? [...groups.values()].map((group, groupIndex) => `<section class="work-group" data-group="${esc(group.id)}"><button class="group-toggle" data-list-group="${esc(group.id)}" data-list="${esc(key)}" data-list-control="${esc(key + ':group:' + group.id)}" aria-expanded="${open(group)}" aria-controls="${esc(key)}:group-body:${groupIndex}" title="${esc(group.id)}"><span class="group-chevron" aria-hidden="true">${open(group) ? '⌄' : '›'}</span><span class="group-name">${esc(group.label)}</span><span class="group-count">${plural(group.rows.length, options.plural === 'tabs' ? 'tab' : 'item')}</span></button><ul class="work-items" id="${esc(key)}:group-body:${groupIndex}" ${open(group) ? '' : 'hidden'}>${open(group) ? group.rows.map((r, index) => rowMarkup(r, `${groupIndex}-${index}`)).join('') : ''}</ul></section>`).join('') : `<div class="compact-empty">${esc(options.empty)}</div>`}</div>
    ${chosen.length ? `<div class="selection-bar has-selection ${options.kind === 'tab' ? 'tab-selection' : ''}"><span role="status"><b>${plural(chosen.length, options.noun)} selected</b>${memory ? ` · ${options.estimated ? '≈ ' : ''}${bytes(memory)}` : ''}</span><div class="list-actions"><button data-list-clear="${esc(key)}" data-list-control="${esc(key)}:clear">Clear</button><button class="primary ${options.kind === 'tab' ? 'close-tabs-button' : ''}" data-list-review="${esc(key)}" data-list-control="${esc(key)}:review">${options.kind === "tab" ? `${icon("close")}Close ${plural(chosen.length, "tab")}` : "Review selected"}</button></div></div>` : ''}
  </section>`;
}

function projectGroup(project) {
  const path = (project || '').replace(/[\\/]+$/, '');
  return { group: path || 'Project unknown', groupLabel: path.split(/[\\/]/).pop() || 'Project unknown' };
}

function sessionRows(list) {
  const rows = [];
  for (const x of filteredSessions(list)) {
    if (x.threads?.length) {
      const q = state.sessFilter.trim().toLowerCase();
      const matchesBackend = [x.session_name, x.project, x.first_prompt, x.host].join(' ').toLowerCase().includes(q);
      // Transcript activity is not proof that a loaded task is active or stale.
      // Activity filters refer to sessions; loaded tasks are visible in All activity.
      if (state.sessState === 'all') for (const t of x.threads.filter(t => !q || matchesBackend || taskSearch(t).includes(q))) {
        const ago = t.last_activity == null || state.snap?.taken_at == null ? null : Math.max(0, state.snap.taken_at - t.last_activity);
        rows.push({ id: `${sessionKey(x)}:task:${t.id || t.transcript}`, ...projectGroup(t.cwd), task: true,
          title: t.name || t.first_prompt || t.id || 'Unnamed task', context: `${t.helper ? 'Helper task' : 'Loaded task'} · ${x.host}`, location: t.cwd || '',
          type: t.helper ? 'Helper task' : 'Loaded task', status: 'idle', statusLabel: 'Loaded', idle: ago, idleLabel: 'Last transcript activity', protection: 'Managed in the host app',
          metric: x.engine ? 'Shared by the backend' : 'Shared by the session', metricLabel: 'Memory / CPU', facts: [['Project', t.cwd || 'Unknown'], ['Task ID', t.id || 'Unknown'], [x.engine ? 'Backend PID' : 'Session PID', x.pid], ['Visibility', 'Transcript held open']],
          note: 'An open transcript shows this task is loaded. Last activity does not prove it is running or finished. Manage this task in its host app.' });
      }
      if (x.engine) continue;
    }
    const idle = x.idle_secs ?? x.quiet_for_secs, protection = sessionProtection(x);
    rows.push({ id: sessionKey(x), data: x, ...projectGroup(x.engine ? 'Shared backends' : x.project || x.cwd),
      title: x.engine ? `${AGENT_LABEL[x.kind] || 'Agent'} backend` : sessionName(x), context: `${x.host || 'Host unknown'}${x.engine ? ' · Shared backend' : ''}`, location: x.project || x.cwd || '',
      type: x.engine ? 'Shared backend' : 'Agent session', status: x.state, statusLabel: protection || (x.state === 'stale' ? 'Stale' : 'Idle'), idle, idleLabel: x.idle_secs != null ? 'Last activity' : 'CPU quiet for', idleIsDuration: x.idle_secs == null,
      metric: bytes(x.rss), metricLabel: x.engine ? 'Shared memory across loaded tasks' : 'Memory footprint', rss: x.rss, eligible: canCloseSession(x), protection,
      facts: [['Host', x.host], ['Project', x.project || x.cwd || 'Unknown'], ['Open', dur(x.age_secs)], ['CPU', (x.cpu_window_mean ?? x.cpu ?? 0).toFixed(1) + '%'], ['PID', x.pid], ['Ports', (x.ports || []).join(', ') || 'None'], [x.idle_secs != null ? 'Last activity' : 'CPU quiet for', x.state === 'active' ? 'Working now' : idle != null ? dur(idle) : 'Unknown']],
      note: x.engine ? `${x.host}'s agent engine serves the app's threads. Task details are unavailable in this snapshot; manage them in the app.` : `${x.first_prompt && x.first_prompt !== sessionName(x) ? x.first_prompt + '\n\n' : ''}The transcript stays on disk. Available resume commands are saved in Actions before closing.`,
      action: protection ? '' : `<button class="primary" data-close-session="${x.pid}">Review close</button>` });
  }
  const labels = new Map();
  for (const r of rows) {
    if (!labels.has(r.groupLabel)) labels.set(r.groupLabel, new Set());
    labels.get(r.groupLabel).add(r.group);
  }
  for (const r of rows) if (labels.get(r.groupLabel).size > 1) r.groupLabel = r.group;
  if (state.sessSort === 'name') rows.sort((a, b) => a.title.localeCompare(b.title));
  else if (state.sessSort === 'idle') rows.sort((a, b) => (b.idle ?? -1) - (a.idle ?? -1));
  else if (state.sessSort === 'rss') rows.sort((a, b) => (b.rss ?? -1) - (a.rss ?? -1));
  if (state.sessReverse && state.sessSort !== 'age') rows.reverse();
  return rows;
}

function sessionTable(list) {
  const rows = sessionRows(list);
  const resources = resourceList('sessions', rows, { kind: 'sess', label: 'Agent work', noun: 'session', plural: 'sessions', review: chosen => reviewSessions(chosen.map(r => r.data)), empty: 'No work matches. Try another search or choose All activity.' });
  const backends = list.filter(x => !goneSession(x) && x.engine && x.threads?.length);
  return resources + (backends.length ? `<details class="backend-resources" data-keep-open="backend-resources"><summary>Shared backend resources <span>${bytes(backends.reduce((sum, x) => sum + x.rss, 0))}</span></summary><p class="help">Loaded tasks share this memory and CPU. Individual task usage is unavailable.</p>${backends.map(x => `<div class="backend-row"><strong>${esc(AGENT_LABEL[x.kind] || 'Agent')} backend</strong><span>${esc(sessionCount([x]))}${x.threads.some(t => t.helper) ? ` · ${plural(x.threads.filter(t => t.helper).length, 'helper')}` : ''}</span><span title="Shared memory across loaded tasks">${bytes(x.rss)}</span></div>`).join('')}</details>` : '');
}

function workOptions(kind, browser) {
  const tab = kind === 'tab', key = state.view + (tab ? ':tabs' : ':sessions');
  const values = tab ? [['idle','Longest untouched'],['site','Website'],['title','Title A–Z'],['window','Window order']] : [['rss','Most memory'],['idle','Longest idle'],['age','Oldest'],['name','Name A–Z']];
  const prefix = tab ? 'tab' : 'sess';
  const profile = browser?.open_profiles.find(p => p.dir === state.tabProfile)?.label || state.tabProfile;
  const searchOpen = state.searchOpen.has(state.view) || !!(tab ? state.tabFilter : state.sessFilter);
  return `<div class="work-heading-actions"><button class="search-toggle" data-search-toggle="${kind}" data-list-control="${esc(key)}:search-toggle" aria-label="${searchOpen ? 'Hide search' : 'Show search'}" aria-expanded="${searchOpen}" title="${searchOpen ? 'Hide search' : 'Search'}">${icon('search')}</button><details class="list-options" data-keep-open="${prefix}-options"><summary data-list-control="${esc(key)}:options" aria-label="View options" title="View options">${icon('settings')}${tab && profile ? `<span>${esc(profile)}</span>` : ''}</summary><div class="list-options-panel">${tab ? `<label>Profile<select data-tab-profile data-list-control="${esc(key)}:profile" aria-label="Filter by profile"><option value="">All profiles</option>${browser.open_profiles.map(p => `<option value="${esc(p.dir)}" ${state.tabProfile === p.dir ? 'selected' : ''}>${esc(p.label)}</option>`).join('')}</select></label>` : ''}<label>Sort by<select ${tab ? 'data-tab-sort' : 'data-sess-sort'} data-list-control="${esc(key)}:sort" aria-label="${tab ? 'Sort tabs' : 'Sort sessions'}">${values.map(([value, label]) => `<option value="${value}" ${state[prefix + 'Sort'] === value ? 'selected' : ''}>${label}</option>`).join('')}</select></label><button data-list-eligible="${esc(key)}">Select visible items</button><button data-list-stale="${esc(key)}">${tab ? 'Close' : 'Review'} visible stale ${tab ? 'tabs' : 'sessions'}</button></div></details></div>`;
}


function viewAgents(h) {
  const s = state.snap, live = h.sessions.filter(x => !goneSession(x));
  const appRss = h.app ? Math.max(0, h.rss - h.sessions.reduce((n, x) => n + x.rss, 0)) : 0;
  const ports = s.ports.filter(p => h.pids.includes(p.pid) && !h.sessions.some(x => (x.pids || []).includes(p.pid)));
  const trend = (s.trends || []).find(t => t.key === h.key && t.span_secs >= 600);
  return `<div class="work-heading">${holderIcon(h)}<div><h1>${esc(h.name.replace(/ sessions$/, ''))}</h1><p>${bytes(h.rss)} footprint <span>/</span> ${sessionCount(live)}</p></div>${workOptions('sess')}</div>
    ${sessionTable(h.sessions)}<p class="help work-help">${live.some(x => !x.engine) ? 'Select checkboxes to review sessions. Shift-click selects a range. Active work stays open.' : 'Loaded tasks share the app backend. Manage these tasks in their host app.'}</p>
    <details class="browser-details" data-keep-open="agent-details"><summary><span>App details &amp; actions</span><small>Memory, trends, servers, and app controls</small></summary><div class="browser-detail-body">
    ${h.app ? `<p class="help">The ${esc(h.app)} app holds ${bytes(appRss)}. The remaining footprint belongs to its sessions, including those running in terminals.</p>` : ''}
    ${holderMemoryHelp(h)}${trend ? trendsTable([trend]) : ''}${ports.length ? `<h2>Listening ports</h2>${portsTable(ports)}` : ''}${quitBlock(h)}</div></details>`;
}


function tabTable(b, tabs) {
  const rows = tabs.map(t => {
    const profile = b.open_profiles.find(p => p.dir === t.profile)?.label || t.profile;
    const protection = t.pinned ? "Pinned" : t.active ? "In use" : !b.can_close_tabs ? "Closing unavailable" : "";
    const status = t.active ? "active" : isStale(b, t) ? "stale" : "idle";
    return { id: tabKey(b, t), data: t, group: t.site || hostnameOfTab(t) || "Browser pages", groupLabel: t.site || hostnameOfTab(t) || "Browser pages", title: t.title.trim() || t.url, context: t.url.replace(/^https?:\/\//, ""), location: `${profile}${t.window_id != null ? ` · Window ${t.window_id}` : ""}${t.index != null ? ` · Tab ${t.index}` : ""}`, type: "Browser tab", status, statusLabel: protection || (status === "stale" ? "Stale" : "Idle"), idle: t.idle_secs, idleLabel: "Last viewed", metric: t.active ? "Now" : t.idle_secs != null ? compactDuration(t.idle_secs) + " ago" : "Not viewed", detailMetric: b.per_tab_estimate ? "≈ " + bytes(b.per_tab_estimate) : "Unknown", metricLabel: "Renderer memory ÷ all tabs (estimate)", rss: b.per_tab_estimate, eligible: canCloseTab(b, t), protection,
      facts: [["Profile", profile], ["Last viewed", t.active ? "Now" : t.idle_secs != null ? dur(t.idle_secs) + " ago" : "Not viewed"], ["Protection", protection || "None"], ["URL", t.url]], note: "Reopen with ⌘⇧T in the same profile. The URL is saved in Actions; unsaved text may not return.",
      action: protection ? `<button disabled>${esc(protection)}${b.can_close_tabs ? ' · kept open' : ''}</button>` : `<button class="primary close-tabs-button" data-close-tab="${t.id}" data-browser="${esc(b.name)}">${icon("close")}Close tab</button>` };
  });
  return resourceList("tabs", rows, { kind: "tab", label: "Browser tabs", title: "Tab / site", metric: "Last viewed", noun: "tab", plural: "tabs", estimated: true, review: chosen => closeTabs(b, chosen.map(r => r.data)), empty: "No tabs match. Try another search, state, or profile." });
}


function viewBrowser(h) {
  const s = state.snap, b = h.browser;
  const threshold = state.settings?.tab_stale_after_secs ?? 86400;
  const live = b.tabs.filter(t => !goneTab(t)), visible = filteredTabs(b);
  let html = `<div class="work-heading">${holderIcon(h)}<div><h1>${esc(h.name)}</h1><p>${bytes(h.rss)} footprint <span>/</span> ${plural(live.length, 'open tab')}</p></div>${workOptions('tab', b)}</div>`;
  html += `${!live.length && b.tabs_note ? `<p class="note">${esc(b.tabs_note)}</p>` : ""}${tabTable(b, visible)}
    <p class="help work-help">Select tabs to close. Pinned and active tabs stay open. Per-tab footprints are estimates.</p>
    <details class="browser-details" data-keep-open="browser-details"><summary><span>Browser details &amp; actions</span><small>Memory, sites, recovery, and app controls</small></summary><div class="browser-detail-body">
    <dl class="detail-metrics" aria-label="Browser summary"><div><dt>Processes</dt><dd>${h.procs}</dd></div><div><dt>CPU</dt><dd>${h.cpu.toFixed(1)}%</dd></div><div><dt>Profiles</dt><dd>${b.open_profiles.length}</dd></div><div><dt>Renderer memory</dt><dd>${bytes(b.renderer_rss)}</dd></div></dl>
    <section class="detail-block"><h2>How memory is counted</h2>${holderMemoryHelp(h)}</section>
    <section class="detail-block"><h2>Sites in these results</h2><p class="help">Stale means not viewed for ${dur(threshold)}. Pinned and active tabs stay open.</p>${sitesTable(b, visible)}</section>`;
  const growing = (s.trends || []).filter(t => t.kind === "renderer" && t.growth > 0 && t.span_secs >= 600 && (b.renderer_procs || []).some(r => `r:${r.pid}:${r.start_time}` === t.key)).sort((x, y) => y.growth - x.growth).slice(0, 5);
  if (growing.length) html += `<section class="detail-block">${trendsTable(growing, "Pages growing", "growing")}<p class="help">Each row is a renderer process; the browser does not identify its tab.</p></section>`;
  html += `<section class="detail-block"><h2>Recovery and estimates</h2><p class="help">Per-tab estimates divide total renderer memory evenly across all open tabs; individual tabs may use more or less. Site estimates multiply that average by the selected tab count. ${bytes(b.renderer_rss)} is held by ${plural(b.renderers, "page renderer")}, plus ${plural(b.extension_renderers, "extension renderer")}. ${b.can_close_tabs ? "Use ⌘⇧T in the same profile to reopen a recently closed tab; its URL is saved in Actions. Unsaved drafts and temporary chats may not be restored." : "Closing tabs is unavailable on this platform."}</p></section>`;
  return html + `<section class="detail-block detail-actions">${quitBlock(h)}</section></div></details>`;
}

function quitBlock(h) {
  const s = state.snap;
  // An agent group quits the app folded into it; anything else quits itself.
  const app = h.kind === "agent" ? h.app : h.kind === "app" || h.kind === "browser" ? h.name : null;
  if (!app) return "";
  const hosting = s.sessions.filter(x => x.host_app === app);
  const self = s.sessions.some(x => x.is_self && x.host_app === app);
  const never = ["Finder", "autoTrim", "autotrim-tray"].includes(app);
  let why = "";
  if (never) why = "this is not something autoTrim quits";
  else if (self) why = "this app is running the session autoTrim is in";
  else if (hosting.length) why = `hosts ${plural(hosting.length, "agent session")}; close those first`;
  const back = h.kind === "browser" ? "with a request to restore its last session; unsaved state may not return" : "with a fresh process; memory savings are not measured";
  return `<h2>Quit or restart</h2><div class="card" style="display:flex;gap:12px;align-items:center;flex-wrap:wrap"><button ${why ? "disabled" : ""} data-quit="${esc(app)}">Quit ${esc(app)}</button><button ${why ? "disabled" : ""} data-restart="${esc(app)}">Restart ${esc(app)}</button><span class="muted">${why ? esc(why) : `Quit asks the app to quit the way ⌘Q would, so it can prompt to save. Restart does the same, waits for it to exit, and opens it again ${back}. Both are logged with the command to reopen it.`}</span></div>`;
}


function viewApp(h) {
  const s = state.snap;
  const hosted = s.sessions.filter(x => x.host_app === h.name);
  const trend = (s.trends || []).find(t => t.key === "g:" + h.name && t.span_secs >= 600);
  const ports = s.ports.filter(p => h.pids.includes(p.pid));
  let html = `<h1>${holderIcon(h)}${esc(h.name)} <span class="pill">${h.kind === "app" ? "app" : "process"}</span></h1>
    <div class="sub"><span><b>${bytes(h.rss)}</b> memory footprint</span><span><b>${h.cpu.toFixed(1)}%</b> cpu</span><span><b>${plural(h.procs, "process")}</b></span>${trend ? `<span>${trend.growth >= 0 ? "grew" : "shrank"} <b>${bytes(Math.abs(trend.growth))}</b> over ${dur(trend.span_secs)}</span>` : ""}</div>`;
  if (hosted.length) {
    html += `<h2>Agent sessions it hosts</h2>${sessionTable(hosted)}`;
  }
  if (ports.length) {
    html += `<h2>Listening</h2>` + portsTable(ports);
  }
  if (h.kind === "other") {
    html += `<p class="note">A bare executable rather than an app bundle: ${plural(h.procs, "process")} named ${esc(h.name)}, ${bytes(h.rss)} between them. autoTrim only stops these when they are a forgotten local server (see Listening ports).</p>`;
  }
  if (!hosted.length && !ports.length && h.kind === "app") {
    html += `<p class="note">${plural(h.procs, "process")} under this app bundle, ${bytes(h.rss)} combined memory footprint.</p>`;
  }
  return html + holderMemoryHelp(h) + quitBlock(h);
}


function sitesTable(b, tabs) {
  const rows = sitesOf(b, tabs).slice(0, 12).map(st => {
    const closable = tabs.filter(t => t.site === st.site && canCloseTab(b, t)), stale = closable.filter(t => isStale(b, t));
    const domains = siteHostChoices(b, st.site, tabs);
    const domainAction = /^(Google )?Chrome$/.test(b.name) && domains.length ? `<button data-auto-domain-site="${esc(st.site)}" data-browser="${esc(b.name)}" ${state.settings?.config_error ? "disabled" : ""}>Auto-close this domain…</button>` : "";
    return { id: st.site, title: st.site, context: `${plural(st.tabs, "tab")} · ${st.stale_tabs} stale`, type: "Site", metric: b.per_tab_estimate ? "≈ " + bytes(st.est_rss) : "—", metricLabel: "Average per tab × selected site tabs (estimate)", facts: [["Tabs", st.tabs], ["Stale", st.stale_tabs], ["Oldest untouched", st.oldest_idle_secs != null ? dur(st.oldest_idle_secs) : "Unknown"]], note: "These counts follow your search and profile filters. Pinned and active tabs stay open.", action: `<button class="primary" ${stale.length ? '' : 'disabled'} data-close-tabs="${stale.map(t => t.id).join(',')}" data-browser="${esc(b.name)}">Close ${plural(stale.length, 'stale tab')}</button><button ${closable.length ? '' : 'disabled'} data-close-tabs="${closable.map(t => t.id).join(',')}" data-browser="${esc(b.name)}">Close all ${plural(closable.length, 'eligible tab')}</button>${domainAction}` };
  });
  return compactList("sites", rows, { label: "Sites in these results", title: "Site / tabs", metric: "≈ Memory", noun: "site" });
}

function portsTable(list) {
  const rows = list.map(p => ({ id: JSON.stringify([p.pid, p.port, p.protocol, p.addr]), data: p, title: p.label || p.process || "Listening process", context: `${p.owner} · ${p.process}`, type: "Listening port", status: "idle", statusLabel: p.owner_managed ? "Managed" : "Unmanaged", metric: String(p.port), metricLabel: "Listening port", eligible: !p.owner_managed, protection: "Managed by its app or the system", facts: [["Owner", p.owner], ["Process", `${p.process} (${p.pid})`], ["Address", p.addr], ["Protocol", p.protocol], ["Open", dur(p.open_for_secs)]], note: `${p.label_source ? p.label_source + '. ' : ''}Stopping the process closes all its listening ports. Managed services stay under their app’s control.`, action: p.owner_managed ? `<button disabled>Managed · kept open</button>` : `<button class="primary" data-review-port="${p.pid}">Review stop</button>` }));
  return compactList("ports", rows, { label: "Listening ports", title: "Server / owner", metric: "Port", noun: "port", plural: "ports", review: chosen => reviewPorts(chosen.map(r => r.data)) });
}


function viewPorts() {
  const s = state.snap;
  return `<h1>Listening ports</h1><div class="sub"><span><b>${s.ports.length}</b> open</span><span><b>${s.ports.filter(p => !p.owner_managed).length}</b> unmanaged: started from a terminal or script, nothing will restart them</span></div>` + (s.ports.length ? portsTable(s.ports) : `<p class="note">Nothing is listening.</p>`);
}


function recoveryDetails(r) {
  const command = r.resume || "";
  // Tab recovery tips are stored as trailing shell comments in existing logs.
  // Only split outside quotes, so a matching phrase inside a URL stays intact.
  if (r.action === "close_tab") {
    const hintPrefix = "   # or ";
    let quote = "";
    for (let i = 0; i < command.length; i++) {
      const c = command[i];
      if (c === "\\" && quote !== "'") { i++; continue; }
      if (quote) { if (c === quote) quote = ""; continue; }
      if (c === "'" || c === '"') { quote = c; continue; }
      if (command.startsWith(hintPrefix + "⌘⇧T in the ", i)) {
        return { command: command.slice(0, i), hint: "Or press " + command.slice(i + hintPrefix.length) };
      }
    }
  }
  return { command, hint: "" };
}


function recoveryControl(r) {
  const { command, hint } = recoveryDetails(r);
  if (!command) return "";
  return `<button type="button" class="copy-command" data-copy-command="${esc(command)}" title="Copy to clipboard" aria-label="Copy recovery command for ${esc(r.target)}: ${esc(command)}">
    <code>${esc(command)}</code><span aria-hidden="true">Copy</span>
  </button>${hint ? `<div class="muted recovery-hint">${esc(hint)}</div>` : ""}`;
}


function holderMemoryHelp(h) {
  const mac = /macos/i.test(state.snap.system.os || "");
  return `<p class="help">Total for ${plural(h.procs, "process")}, including helpers${h.kind === "agent" ? " and sessions" : ""}. ${mac ? "Compare the same processes in Activity Monitor’s Memory column. Compressed and swapped allocations count at their original size, so this is not physical RAM that closing will release." : "Process totals are not a measurement of physical RAM that closing will release."}</p>`;
}

function observedMemory(r) {
  const m = r.memory_observation;
  if (!m) return "";
  const change = (before, after) => after === before ? "0 B" : signedBytes(after - before);
  const elapsed = Math.max(0, m.after.taken_at_ms - m.before.taken_at_ms) / 1000;
  return `<div class="memory-observation"><div class="action-detail-label">Observed whole-machine change${m.tab_batch ? ` · batch of ${plural(m.attempted_actions, "tab attempt")}` : ""}</div><dl class="action-memory-changes">${[["RAM used", m.before.used_mem, m.after.used_mem], ["Swap used", m.before.used_swap, m.after.used_swap]].map(([label, before, after]) => `<div><dt>${label}</dt><dd><span>${bytes(before)} → ${bytes(after)}</span><b>${change(before, after)}</b></dd></div>`).join("")}</dl><p class="action-detail-note">Over ${elapsed.toFixed(1)} seconds. Includes other activity; not attributed savings.</p></div>`;
}

function viewActions() {
  const labels = {close_session: "Close session", stop_server: "Stop server", close_tab: "Close tab", quit_app: "Quit app", restart_app: "Restart app"};
  if (!state.log.length) return `<h1>Actions</h1>${cleanupSummary({ showDetails: false })}<div class="note">No actions yet. When you close something, its result and available recovery instructions will appear here.</div>`;
  const rows = state.log.slice().reverse().map((r, index) => {
    const preview = r.mode === "dry-run";
    const failed = ["failure", "partial"].includes(r.status) || /fail|error|refus|still running|could not/i.test(r.result);
    const skipped = r.status === "skipped" || /^skipped:/i.test(r.result);
    const mode = preview ? "Preview only" : r.mode === "auto" ? "Automatic" : "Manual";
    const succeeded = r.status === "success" || /^(terminated|closed|stopped|quit|already gone|not open any more)\b/i.test(r.result);
    const result = preview ? "Preview" : skipped ? "Skipped" : failed ? (r.status === "partial" ? "Partial" : "Failed") : succeeded ? ({ close_session: "Closed", stop_server: "Stopped", close_tab: "Closed", quit_app: "Quit", restart_app: "Restarted" }[r.action] || "Done") : "Recorded";
    const key = r.id || JSON.stringify([r.ts, r.action, r.pid, r.target]);
    const open = state.expandedAction === key;
    const detailId = `action-detail-${index}`;
    const date = new Date(r.ts * 1000);
    return `<tr class="history-entry${open ? " expanded" : ""}">
      <td><span class="action-label">${esc(labels[r.action] || r.action.replace(/_/g, " "))}</span><span class="action-secondary">${mode}</span></td>
      <td><button type="button" class="action-toggle" data-action-details="${esc(key)}" data-list-control="${esc('action:' + key)}" aria-expanded="${open}" aria-controls="${detailId}" aria-label="Details for ${esc(r.target)}" title="${esc(r.target)}"><span class="action-target">${esc(r.target)}</span><span class="action-chevron" aria-hidden="true"></span></button></td>
      <td><span class="state ${failed ? "stale" : !preview && !skipped && succeeded ? "active" : "idle"}" title="${esc(r.result)}">${result}</span></td>
      <td class="action-memory" title="${r.action === "close_tab" ? "Estimated memory " : "Memory "}${preview ? "held when evaluated" : "held before the action"}${r.action === "close_tab" ? " (estimated)" : ""}">${r.rss ? `${r.action === "close_tab" ? "≈ " : ""}${bytes(r.rss)}` : "—"}</td>
      <td><time title="${esc(date.toLocaleString())}"><span>${esc(date.toLocaleDateString(undefined, { month: "short", day: "numeric" }))}</span><span class="action-secondary">${esc(date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }))}</span></time></td>
    </tr><tr class="action-detail-row" id="${detailId}" data-action-detail-panel ${open ? "" : "hidden"}><td colspan="5"><div class="action-details">
      <div class="action-detail-heading">${esc(r.target)}</div>
      <dl class="action-result"><dt>Result</dt><dd class="${failed ? "state stale" : ""}">${preview ? "Nothing was closed. " : ""}${esc(r.result)}</dd></dl>
      ${!preview ? observedMemory(r) : ""}
      ${r.resume && !preview ? `<div class="action-recovery"><div class="action-detail-label">${r.action === "close_session" ? "Resume command" : "Reopen command"}</div>${recoveryControl(r)}</div>` : ""}
    </div></td></tr>`;
  }).join("");
  return `<h1>Actions</h1>${cleanupSummary({ showDetails: false })}<div class="wrap actions-wrap"><table class="actions-table" aria-label="Action history"><colgroup><col class="action-col"><col><col class="result-col"><col class="memory-col"><col class="time-col"></colgroup><thead><tr><th scope="col">Action</th><th scope="col">Target</th><th scope="col">Result</th><th scope="col">Memory</th><th scope="col">When</th></tr></thead><tbody>${rows}</tbody></table></div><p class="help">Memory held before the action, not recovered. Details include recovery commands and whole-machine changes.</p>`;
}


function renderMain(viewChanged, force = false) {
  const main = document.getElementById("main");
  const active = document.activeElement;
  const selection = window.getSelection?.();
  if (!force && !viewChanged && selection && !selection.isCollapsed && main.contains(selection.anchorNode)) return; // copying text; keep the selection
  if (!force && !viewChanged && active && main.contains(active) && active.matches("input:not([type=checkbox]), select")) return; // typing; leave it alone
  // Keep copy feedback visible through background refreshes.
  if (!viewChanged && state.view === "actions" && main.querySelector(".copy-command.copying, .copy-command.copy-feedback")) return;
  const focusKey = active?.dataset?.listControl;
  const listScroll = new Map([...main.querySelectorAll("[data-list-scroll]")].map(el => [el.dataset.listScroll, [el.scrollTop, el.scrollLeft]]));
  const scroll = viewChanged ? 0 : main.scrollTop;
  listModels.clear();
  let html;
  const v = state.view;
  if (v === "overview") html = viewOverview();
  else if (v === "ports") html = viewPorts();
  else if (v === "actions") html = viewActions();
  else if (v === "settings") html = viewSettings();
  else {
    const h = holderByKey(v);
    if (!h) { state.view = "overview"; html = viewOverview(); renderSide(); }
    else if (h.kind === "agent") html = viewAgents(h);
    else if (h.browser) html = viewBrowser(h);
    else html = viewApp(h);
  }
  const expanded = new Set([...main.querySelectorAll("details[open][data-keep-open]")].map(d => d.dataset.keepOpen));
  main.innerHTML = html;
  if (!viewChanged) main.querySelectorAll("details[data-keep-open]").forEach(d => { d.open = expanded.has(d.dataset.keepOpen); });
  main.scrollTop = scroll;
  if (!viewChanged) main.querySelectorAll("[data-list-scroll]").forEach(el => { const position = listScroll.get(el.dataset.listScroll); if (position) [el.scrollTop, el.scrollLeft] = position; });
  wire(main);
  if (!viewChanged && focusKey) [...main.querySelectorAll("[data-list-control]")].find(el => el.dataset.listControl === focusKey)?.focus({ preventScroll: true });
}


// Keep the pressed element alive until its click fires, even if a poll completes.
let pointerHeld = false, renderPending = false;
document.onpointerdown = e => {
  document.documentElement.dataset.listInput = "pointer";
  if (e.isPrimary && e.button === 0) pointerHeld = true;
};
// WebKit can treat focus restored after a mouse click as keyboard focus.
// Keep the actual focus, but only show its outline again when using the keyboard.
document.onkeydown = e => {
  if (!e.metaKey && !e.ctrlKey && !e.altKey && !["Shift", "Control", "Alt", "Meta"].includes(e.key))
    delete document.documentElement.dataset.listInput;
};
function finishPointer() {
  pointerHeld = false;
  if (renderPending) setTimeout(() => { if (renderPending) renderAll(false); }, 0);
}
document.onpointerup = e => { if (e.isPrimary) finishPointer(); };
document.onpointercancel = finishPointer;
window.onblur = finishPointer;
function renderAll(viewChanged) {
  if (!viewChanged && pointerHeld) { renderPending = true; return; }
  renderPending = false;
  renderHeader(); renderSide(); renderMain(viewChanged);
}

// `fresh` asks the app to scan now instead of reading the daemon's last
// tick. What is on screen is never replaced by something older, so a fresh
// scan taken after an action stays until the daemon catches up with it.

function filterChips(kind, selected, choices) {
  return `<div class="filters" role="group" aria-label="Filter ${kind}s">${choices.map(([value, text]) => `<button data-${kind}-state="${value}" aria-pressed="${selected === value}">${text}</button>`).join("")}</div>`;
}

export { renderHeader, syncInfo, syncText, syncMarkup, tickSync, renderSide, adviceTarget, adviceCards, observationCards, trendsTable, controlCards, pendingList, autoCard, backgroundCard, isObservation, viewOverview, kindBar, viewSettings, updateCard, rowStatus, compactInspector, compactList, sessionTable, viewAgents, tabTable, viewBrowser, quitBlock, viewApp, sitesTable, portsTable, viewPorts, recoveryDetails, recoveryControl, viewActions, renderMain, renderAll, filterChips, refresh };
