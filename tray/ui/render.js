// Dashboard markup and DOM rendering.
import { icon, GB, bytes, dur, esc, plural, AGENT_LABEL, epochNow, nowSecs, compactDuration, signedBytes } from "./format.js";
import { domainInactivityLabel, state, listModels, goneTab, goneSession, holders, holderByKey, appIconName, POLL_MS, sync, autoMode, autoModeWord, sessionKey, tabKey, sessionName, sessionCount, taskSearch, sessionProtection, reconcileList, isStale, sitesOf, siteHostChoices, canCloseSession, canCloseTab, filteredSessions, filteredTabs } from "./state.js";
import { createActions } from "./actions.js";
const { refresh, navigate, reviewSessions, reviewTabs, reviewPorts, wire } = createActions({ renderAll: (...a) => renderAll(...a), renderMain: (...a) => renderMain(...a), renderSide: (...a) => renderSide(...a), renderHeader: (...a) => renderHeader(...a) });
function renderHeader() {
  const sys = state.snap.system, total = sys.total_mem;
  const used = total > 0 ? Math.max(0, Math.min(100, sys.used_mem * 100 / total)) : 0;
  const comp = total > 0 && sys.compressed != null ? Math.max(0, Math.min(used, sys.compressed * 100 / total)) : 0;
  const detail = [sys.wired != null ? `${bytes(sys.wired)} wired` : "", `up ${dur(sys.uptime_secs)}`].filter(Boolean).join(" · ");
  const memoryOpen = document.getElementById("memory-details")?.open;
  const memoryFocused = document.activeElement?.id === "memory-summary";
  document.getElementById("head").innerHTML = `
    <div class="app-brand" aria-label="autoTrim"><span class="brand-mark" aria-hidden="true"></span><span>autoTrim</span></div>
    <details class="mem" id="memory-details" ${memoryOpen ? "open" : ""}>
      <summary id="memory-summary" title="Show memory breakdown"><span class="muted">RAM used</span><b>${total >= GB && sys.used_mem >= GB ? `${(sys.used_mem / GB).toFixed(1)} / ${bytes(total)}` : `${bytes(sys.used_mem)} / ${bytes(total)}`}</b>
        <span class="meter" role="img" aria-label="${esc(`${bytes(sys.used_mem)} of ${bytes(total)} RAM in use`)}"><i class="used" style="width:${(used - comp).toFixed(1)}%"></i><i class="comp" style="width:${comp.toFixed(1)}%"></i></span><span class="memory-chevron" aria-hidden="true"></span>
      </summary>
      <div class="memory-detail"><span>${sys.compressed != null ? `Includes <b>${bytes(sys.compressed)}</b> compressed` : "Physical memory"}</span><span><b>${bytes(Math.max(0, total - sys.used_mem))}</b> outside used (includes cache)</span><span class="memory-system">${esc(detail)}</span><span class="memory-system">App totals include helper processes. On macOS, footprints include compressed and swapped allocations at their original size; they do not add up to physical RAM used. Units use powers of 1024.</span></div>
    </details>
    <div class="stat" title="${esc(`${bytes(sys.used_swap)} of ${bytes(sys.total_swap)} swap in use`)}"><span>Swap</span><b>${bytes(sys.used_swap)}</b></div>
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
  return `<span class="app-icon" aria-hidden="true">${data ? `<img src="${esc(data)}" alt="" width="32" height="32">` : icon(h.kind)}</span>`;
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
    </div><div class="nav-holders"><h3>${focus.length ? "Your focus first" : "Apps by memory"} <span style="font-weight:400;white-space:nowrap">Memory ↓</span></h3>`;
  const LIMIT = 20;
  const shown = state.showAll ? hs : hs.slice(0, LIMIT);
  const largest = Math.max(1, ...hs.map(h => h.rss));
  html += shown.map(h => `<a class="item${sel(h.key)}" data-view="${esc(h.key)}" title="${esc(h.name)} · ${esc(h.count)} · ${plural(h.procs, "process")} · ${h.cpu.toFixed(1)}% CPU">
      ${holderIcon(h)}<span class="name">${esc(h.name)}</span><span class="mem">${bytes(h.rss)}</span><span class="holder-meter" aria-hidden="true"><i style="width:${Math.max(0, h.rss / largest * 100).toFixed(2)}%"></i></span></a>`).join("");
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
  const rows = list.map(t => ({ id: t.key, title: t.name, context: `${signedBytes(t.growth)} over ${dur(t.span_secs)}`, type: "Memory trend", metric: bytes(t.rss_now), metricLabel: "In memory now", facts: [["Change", signedBytes(t.growth)], ["Rate / hour", signedBytes(t.bytes_per_hour)], ["Mean CPU", t.cpu_mean.toFixed(1) + "%"], ["Window", dur(t.span_secs)], ["Samples rising", Math.round(t.rising_frac * 100) + "%"]], note: "Changes compare the start and end of the observation window. Memory growth is advice, never an automatic action." }));
  return `<h2>${esc(title)} <small>over the last ${dur(Math.max(...list.map(t => t.span_secs)))}</small></h2>` + compactList(scope, rows, { label: title, title: "Process / change", metric: "Now", noun: "trend" });
}

// ---- the two switches: auto mode, and running in the background ----

function controlCards() {
  if (!state.settings || !state.service) return `<p class="note">Settings could not be read from the app.</p>`;
  return `<div class="duo">${autoCard()}${backgroundCard()}</div>`;
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
  let html = `<div class="card settings-card"><div class="h">Auto mode <a data-goto="actions">View actions</a></div><div class="muted">Preview what autoTrim would close before turning automatic closing on.</div>`;
  if (c.config_error) html += `<p class="state stale">Settings could not be read; defaults are shown: ${esc(c.config_error)}</p>`;
  html += `<div class="mode-choices" role="group" aria-label="Auto mode">${[["off", "Off", "You close things"], ["preview", "Preview only", "Log, never close"], ["on", "On", "Warn, then close"]].map(([value, label, sub]) => `<label class="mode-choice"><input type="radio" name="auto-mode" data-auto-mode="${value}" ${mode === value ? "checked" : ""} ${busy || c.config_error ? "disabled" : ""}><span><b>${label}</b><small>${sub}</small></span></label>`).join("")}</div>`;
  html += `<p class="muted">${mode === "off" ? "Monitoring and advice continue. All closing stays manual." : mode === "preview" ? "Preview records what would have closed in Actions. Nothing is closed." : `Warns first, then waits ${dur(c.auto_grace_minutes * 60)}. Anything used during that time is spared.`}</p>`;
  for (const [key, label, checked, detail] of [["close_sessions", "Close stale agent sessions", c.auto_close_sessions, `Idle past ${dur(c.stale_after_secs ?? 21600)}, with transcript and CPU evidence.`], ["stop_servers", "Stop old local servers", c.auto_stop_servers, `Quiet dev servers open past ${dur(c.port_stale_after_secs ?? 86400)}.`]]) {
    html += `<div class="setting-row"><label><span>${label}<small class="muted" style="display:block">${detail}</small></span><input type="checkbox" data-auto="${key}" ${checked ? "checked" : ""} ${busy || mode === "off" || c.config_error ? "disabled" : ""}></label></div>`;
  }
  html += domainSettings(c, mode, busy);
  html += `<details class="help" data-keep-open="auto-protection"><summary>What auto mode always keeps open</summary><p>Active sessions, app engines, the newest session in each project, and selected or pinned browser tabs. Only these session hosts are allowed: ${esc((c.auto_hosts || []).join(", ") || "none")}. Activity during the warning period cancels that target’s close.</p></details>`;
  if (!state.src.daemon_running) html += `<p class="note">The background monitor is stopped. Start it below for auto mode to run.</p>`;
  else if (a) {
    const applied = a.close_sessions === c.auto_close_sessions && a.stop_servers === c.auto_stop_servers && a.close_tabs === c.auto_close_tabs && a.dry_run === c.auto_dry_run && a.tab_rules_revision === c.tab_rules_revision;
    if (!applied || busy) html += `<p class="muted" role="status">Applying… the background monitor picks up changes on its next sample.</p>`;
    if (a.pending.length) html += pendingList(a) + (mode !== "off" ? `<div class="row2"><button data-auto-off ${busy ? "disabled" : ""}>Turn off auto mode</button><span class="muted">Cancels pending closes when the change is picked up.</span></div>` : "");
    else if (mode !== "off" && applied) html += `<p class="muted">Nothing is waiting to be closed.</p>`;
  }
  const dry = state.log.filter(r => r.mode === "dry-run");
  if (dry.length) html += `<p class="muted">Preview history: ${plural(dry.length, "would-be close")} among the last ${state.log.length} logged actions.</p>`;
  return html + `</div>`;
}

function domainSettings(c, mode, busy) {
  const rules = c.auto_tab_domains || [];
  const hours = c.auto_tab_inactive_hours ?? 24;
  const presets = [10 / 60, 20 / 60, 30 / 60, 45 / 60, 1, 2, 6, 12, 24, 48, 168];
  const choices = presets.includes(hours) ? presets : [...presets, hours].sort((a, b) => a - b);
  const blocked = busy || !!c.config_error;
  return `<section class="domain-settings" aria-labelledby="domain-settings-title">
    <div class="domain-section-head"><div><h3 id="domain-settings-title">Chrome tab cleanup</h3><p class="muted">Preview your matches first. autoTrim can’t detect drafts, media, uploads, or work inside a page.</p></div><span class="tag">All profiles</span></div>
    <div class="setting-row static-setting"><span>Empty New Tab pages<small class="muted">Included in Auto mode</small></span><span class="muted">Included</span></div>
    <div class="setting-row"><label><span>Close tabs from listed domains<small class="muted">A separate target that can run without session or server cleanup.</small></span><input type="checkbox" data-auto-domain-target ${c.auto_close_tabs ? "checked" : ""} ${blocked ? "disabled" : ""}></label></div>
    <div class="domain-controls"><label>Inactive for<select data-domain-hours aria-label="Domain inactivity" ${blocked ? "disabled" : ""}>${choices.map(value => `<option value="${value}" ${value === hours ? "selected" : ""}>${esc(domainInactivityLabel(value))}${!presets.includes(value) ? " (custom)" : ""}</option>`).join("")}</select></label><span class="muted">Then warn for ${dur((c.auto_grace_minutes ?? 10) * 60)}. This warning period is shared by all automatic cleanup.</span></div>
    <div class="domain-list" aria-label="Auto-close domains">${rules.length ? rules.map((rule, index) => `<div class="domain-row"><div><b>${esc(rule.domain)}</b><span>${rule.include_subdomains ? "Includes subdomains" : "Exact domain"}</span></div><div><button data-edit-domain="${index}" ${blocked ? "disabled" : ""}>Edit</button><button data-remove-domain="${index}" ${blocked ? "disabled" : ""}>Remove</button></div></div>`).join("") : `<div class="domain-empty"><b>No domains yet</b><span>Add a domain, then review its matching Chrome tabs before enabling cleanup.</span></div>`}</div>
    <div class="domain-actions"><button data-add-domain ${blocked ? "disabled" : ""}>Add domain</button><button class="primary" data-review-domains ${blocked || !rules.length ? "disabled" : ""}>Review matches</button></div>
    <p class="scope">HTTP and HTTPS on every port. Selected and pinned tabs stay open. Unsaved page content may be lost.</p>
    ${mode === "off" && rules.length ? `<p class="muted">Saved; Auto mode is off.</p>` : ""}
  </section>`;
}


function backgroundCard() {
  const sv = state.service, src = state.src, c = state.settings;
  let status, buttons = "";
  if (!sv.supported) status = "The login service is macOS only so far; run <code>autotrim daemon</code> under your own supervisor.";
  else if (sv.installed && sv.running) { status = `Starts at login and keeps monitoring when this window is closed.`; buttons = `<button data-service="restart" title="restart the daemon, for example after rebuilding it">Restart</button> <button data-service="uninstall" title="stop the daemon and remove the login service; data is kept">Stop login service</button>`; }
  else if (sv.installed) { status = "The login service is installed but the daemon is not running."; buttons = `<button class="primary" data-service="restart">Start</button> <button data-service="uninstall">Remove login service</button>`; }
  else if (src.daemon_running) { status = "A daemon is running in the foreground, in a terminal. As a login service it would outlive that terminal and start at every login."; buttons = `<button class="primary" data-service="install">Run in background</button>`; }
  else { status = "Nothing is running in the background, so this window scans on its own: no trends, no reminders, no auto mode. Run in background installs the daemon as a login service."; buttons = `<button class="primary" data-service="install">Run in background</button>`; }
  const tag = sv.installed && sv.running ? `<span class="tag on">login service</span>` : src.daemon_running ? `<span class="tag">foreground</span>` : `<span class="tag">not running</span>`;
  return `<div class="card settings-card"><div class="h">Run in background ${tag}</div>
    <div class="muted">${status}</div>
    ${sv.binary || sv.pid ? `<details class="help" data-keep-open="service-details"><summary>Service details</summary><p>${sv.pid ? `PID: ${sv.pid}<br>` : ""}${esc(sv.binary || "")}</p></details>` : ""}
    <div class="row2">${buttons}</div>
    <label style="margin-top:8px" title="off: start without the dashboard; click the Dock icon or Open autoTrim… in the menu bar to open it"><input type="checkbox" data-launch-window ${c.open_window_at_launch ? "checked" : ""}>Open this window when the app starts</label>
    <div class="row2"><button data-hide>Hide window</button><span class="muted">The menu bar item stays. The daemon keeps working with the window closed, and with the app quit.</span></div></div>`;
}



const isObservation = a => a.severity !== "high" && /^(growth:|cpu:|page_growth:)/.test(a.id);


function viewOverview() {
  const s = state.snap, a = s.auto;
  const trends = (s.trends || []).filter(t => t.kind !== "renderer" && t.span_secs >= 600).slice(0, 8);
  const observations = s.advice.filter(isObservation), advice = s.advice.filter(a => !isObservation(a));
  const sessions = s.sessions.filter(x => !goneSession(x));
  const tabs = s.browsers.reduce((n, b) => n + b.tabs.filter(t => !goneTab(t)).length, 0);
  return `<div class="overview-heading"><div><h1>Overview</h1><p>Current workload and the few things that may need your attention.</p></div><span class="monitor-status ${state.src.daemon_running ? "on" : ""}">${icon(state.src.daemon_running ? "check" : "info")}${state.src.daemon_running ? "Monitoring" : "Manual scans"}</span></div>
    <div class="overview-layout"><section class="overview-focus"><div class="section-heading"><h2>Worth a look</h2><span>${advice.length ? plural(advice.length, "item") : "All clear"}</span></div>
      ${a && a.pending.length ? `<div class="card">${pendingList(a)}</div>` : ""}
      ${adviceCards(advice)}${observations.length ? observationCards(observations) : ""}
    </section><aside class="overview-rail" aria-label="Current workload"><section><div class="section-heading"><h2>Current load</h2><span>Now</span></div>
      <dl class="overview-stats"><div><dt>Agent work</dt><dd>${sessionCount(sessions)}</dd></div><div><dt>Browser activity</dt><dd>${plural(tabs, "open tab")}</dd></div><div><dt>Memory holders</dt><dd>${plural(s.groups.length, "group")}</dd></div></dl></section>${kindBar(true)}</aside></div>
    <section class="overview-trends">${trendsTable(trends)}</section>
    ${!state.src.daemon_running ? `<p class="note" style="margin-top:16px">The daemon is not running, so this window is scanning on its own and there are no trends. Start it from Settings.</p>` : (trends.length ? "" : `<p class="note" style="margin-top:16px">Trends appear once the daemon has about ten minutes of history.</p>`)}`;
}

// One bar for what the sidebar lists, added up by kind.

function kindBar(compact = false) {
  const sum = {};
  for (const h of holders(state.snap)) sum[h.kind] = (sum[h.kind] || 0) + h.rss;
  const total = Object.values(sum).reduce((n, v) => n + v, 0);
  if (!total) return "";
  const kinds = [["agent", "Agent sessions"], ["browser", "Browsers"], ["app", "Apps"], ["other", "Other processes"]].filter(([k]) => sum[k]);
  return `<section class="memory-mix ${compact ? "compact" : ""}"><div class="section-heading"><h2>${compact ? "Memory mix" : "Process memory by kind"}</h2><span>${plural(state.snap.groups.length, "holder")}</span></div>
    ${compact ? "" : `<p class="help">Grouped process totals, including helpers. On macOS, these include compressed and swapped allocations at their original size and do not add up to physical RAM used.</p>`}
    <div class="kbar">${kinds.map(([k]) => `<i class="${k}" style="width:${(sum[k] * 100 / total).toFixed(1)}%"></i>`).join("")}</div>
    <div class="legend">${kinds.map(([k, l]) => `<span><i class="${k}"></i>${l} <b>${bytes(sum[k])}</b></span>`).join("")}</div>${compact ? `<p class="memory-note">Process totals, not physical RAM used.</p>` : ""}</section>`;
}


function viewSettings() {
  return `<h1>Settings</h1><div class="sub"><span>Choose how autoTrim runs and when it can act.</span></div>${controlCards()}<div class="card settings-card"><div class="h">Your setup</div><p>Change your interests, idle thresholds, notifications, and startup preferences.</p><button data-setup>Review setup</button></div>${updateCard()}`;
}


function updateCard() {
  const u = state.update;
  if (!u) return '';
  const busy = state.updateBusy || ['checking', 'installing'].includes(u.phase);
  const messages = { idle: 'Checks automatically every six hours while autoTrim is open.', checking: 'Checking for updates…', current: 'You’re up to date.', available: `Version ${u.version} is available.`, installing: 'Downloading and installing… autoTrim will restart when ready.', error: 'The update check failed. You can try again.' };
  return `<div class="card settings-card"><div class="h">App updates <span class="tag">${esc(u.current_version)}</span></div>
    <p>${esc(u.enabled ? messages[u.phase] || '' : 'Updates are available in the installed macOS app.')}</p>
    ${u.error ? `<p role="alert">${esc(u.error)}</p>` : ''}
    ${u.service_error ? `<p role="alert">${esc(u.service_error)}</p>` : ''}
    ${u.notes ? `<details class="help" data-keep-open="release-notes"><summary>What’s new in ${esc(u.version)}</summary><p style="white-space:pre-wrap">${esc(u.notes)}</p></details>` : ''}
    ${u.enabled ? `<button data-update="check" ${busy ? 'disabled' : ''}>Check for updates</button>
    ${u.version ? `<button class="primary" data-update="install" ${busy ? 'disabled' : ''}>Install and restart</button>` : ''}` : ''}
  </div>`;
}


function rowStatus(r) {
  return `<span class="row-state ${esc(r.status || '')}"><i class="state-symbol ${esc(r.status || '')}" aria-hidden="true"></i>${esc(r.statusLabel || "Idle")}</span>`;
}

function compactInspector(r, label) {
  if (!r) return "";
  return `<aside class="list-inspector" aria-label="${esc(label)} details">
    <div class="inspector-head"><div class="inspector-kind"><span>${esc(r.type)}</span>${r.statusLabel ? rowStatus(r) : icon("history")}</div><h3>${esc(r.title)}</h3><p class="context">${esc(r.context)}</p></div>
    <section class="inspector-primary" aria-label="Primary measurement"><span>Primary measurement</span><strong>${esc(r.detailMetric ?? r.metric)}</strong><small>${esc(r.metricLabel)}</small></section>
    <section class="inspector-section" aria-labelledby="${esc(r.id)}-facts"><h4 id="${esc(r.id)}-facts">Details</h4><dl>${r.facts.map(([k,v]) => `<div><dt>${esc(k)}</dt><dd>${esc(v)}</dd></div>`).join("")}</dl></section>
    ${r.note ? `<section class="inspector-note" aria-label="Context">${icon("info")}<div><b>Keep in mind</b><span>${esc(r.note)}</span></div></section>` : ""}
    ${r.action ? `<footer class="inspector-actions">${r.action}</footer>` : ""}</aside>`;
}

function compactList(key, rows, options) {
  key = state.view + ":" + key;
  const saved = reconcileList(key, rows), inspected = rows.find(r => r.id === saved.inspected);
  listModels.set(key, { rows, options, saved });
  if (!rows.length) return `<div class="compact-empty">${esc(options.empty || "Nothing to show.")}</div>`;
  const eligible = rows.filter(r => r.eligible), chosen = eligible.filter(r => saved.selected.has(r.id));
  const controls = r => `data-list="${esc(key)}" data-row="${esc(r.id)}"`;
  const selectedMemory = chosen.reduce((n, r) => n + (r.rss || 0), 0);
  const stale = eligible.filter(r => r.status === "stale");
  return `<section class="compact-section" aria-label="${esc(options.label)}"><div class="compact-layout"><div class="compact-list"><table class="compact-table"><thead><tr>${options.review ? `<th class="check"><input type="checkbox" data-list-all="${esc(key)}" data-list-control="${esc(key)}:all" aria-label="Select all eligible ${esc(options.plural)}" ${chosen.length && chosen.length === eligible.length ? "checked" : ""} ${eligible.length ? "" : "disabled"}></th>` : ""}<th>${esc(options.title)}</th><th class="num metric">${esc(options.metric)}</th></tr></thead><tbody>${rows.map(r => `<tr class="${r.id === saved.inspected ? 'inspected' : ''} ${saved.selected.has(r.id) ? 'selected' : ''}">${options.review ? `<td class="check">${r.eligible ? `<input type="checkbox" data-list-select ${controls(r)} data-list-control="${esc(key + ':select:' + r.id)}" aria-label="Select ${esc(r.title)}" ${saved.selected.has(r.id) ? "checked" : ""}>` : `<span class="row-lock" title="${esc(r.protection)}" aria-label="${esc(r.protection)}">${icon("lock")}</span>`}</td>` : ""}<td><button class="row-title" data-list-inspect ${controls(r)} data-list-control="${esc(key + ':inspect:' + r.id)}" aria-pressed="${r.id === saved.inspected}" title="${esc(r.title)}">${esc(r.title)}</button><span class="row-sub">${r.status ? `<i class="state-symbol ${esc(r.status)}" role="img" aria-label="${esc(r.statusLabel)}" title="${esc(r.statusLabel)}"></i>` : ""}${r.status === "stale" && compactDuration(r.idle) ? `<span class="stale-time" title="${esc(r.idleLabel)} ${esc(dur(r.idle))}${r.idleIsDuration ? "" : " ago"}" aria-label="${esc(r.idleLabel)} ${esc(dur(r.idle))}${r.idleIsDuration ? "" : " ago"}">${esc(compactDuration(r.idle))}</span>` : ""}<span class="row-context">${esc(r.context)}</span></span></td><td class="num metric">${esc(r.metric)}</td></tr>`).join("")}</tbody></table>${options.review ? `<div class="list-footer"><span>${chosen.length ? `${plural(chosen.length, options.noun)} selected${selectedMemory ? ` · ${options.estimated ? '≈ ' : ''}${bytes(selectedMemory)}${options.estimated ? ' estimated' : ''}` : ''}` : 'Click a row for details.'}</span><div class="list-actions">${chosen.length ? `<button data-list-clear="${esc(key)}">Clear</button><button class="primary" data-list-review="${esc(key)}">Review ${plural(chosen.length, options.noun)}</button>` : `${stale.length ? `<button data-list-stale="${esc(key)}">Review ${plural(stale.length, 'stale ' + options.noun)}</button>` : ''}<button data-list-eligible="${esc(key)}" ${eligible.length ? '' : 'disabled'}>Select eligible rows</button>`}</div></div>` : ""}</div>${compactInspector(inspected, options.noun)}</div></section>`;
}

// Tabs and sessions are comparison tables; selecting never opens another pane.
function resourceList(key, rows, options) {
  key = state.view + ":" + key;
  const saved = reconcileList(key, rows);
  saved.collapsed ??= new Set();
  const allRows = rows;
  const groupOrder = state.tabReverse ? ["unknown", "recent", "stale"] : ["stale", "recent", "unknown"];
  const groups = options.grouped ? groupOrder.map(id => ({ id, label: {stale: "Stale", recent: "Recent", unknown: "Not viewed"}[id], rows: allRows.filter(r => r.group === id) })).filter(g => g.rows.length) : [];
  if (options.grouped) rows = groups.filter(g => !saved.collapsed.has(g.id)).flatMap(g => g.rows);
  const visibleEligible = new Set(rows.filter(r => r.eligible).map(r => r.id));
  saved.selected = new Set([...saved.selected].filter(id => visibleEligible.has(id)));
  listModels.set(key, { rows, allRows, options, saved });
  if (!allRows.length) return `<div class="compact-empty">${esc(options.empty)}</div>`;
  const eligible = rows.filter(r => r.eligible), chosen = eligible.filter(r => saved.selected.has(r.id));
  const memory = chosen.reduce((sum, r) => sum + (r.rss || 0), 0);
  const kind = options.noun === "tab" ? "tab" : "sess", sort = state[kind + "Sort"], reverse = state[kind + "Reverse"];
  const sortHead = (label, value, cls = "") => {
    const ascending = ["title", "name", "site", "window"].includes(value) !== reverse;
    return `<th class="${cls}" scope="col" aria-sort="${sort === value ? ascending ? 'ascending' : 'descending' : 'none'}"><button class="column-sort" data-resource-sort="${value}" data-sort-kind="${kind}" data-list-control="${esc(key + ':sort:' + value)}">${label}<span aria-hidden="true">${sort === value ? ascending ? '↑' : '↓' : ''}</span></button></th>`;
  };
  const controls = r => `data-list="${esc(key)}" data-row="${esc(r.id)}"`;
  const rowMarkup = r => `<tr class="resource-row ${saved.selected.has(r.id) ? 'selected' : ''}" ${r.eligible ? 'data-selectable' : ''}>
      <td class="check">${r.eligible ? `<label><input type="checkbox" data-list-select ${controls(r)} data-list-control="${esc(key + ':select:' + r.id)}" aria-label="Select ${esc(r.title)}" ${saved.selected.has(r.id) ? 'checked' : ''}></label>` : `<span class="row-lock" role="img" title="${esc(r.protection)}" aria-label="${esc(r.protection)}">${icon("lock")}</span>`}</td>
      <td>${r.eligible ? `<button class="row-title" data-resource-select ${controls(r)} data-list-control="${esc(key + ':row:' + r.id)}" aria-pressed="${saved.selected.has(r.id)}" title="${esc(r.title)}">${esc(r.title)}</button>` : `<span class="row-title" title="${esc(r.title)}">${esc(r.title)}</span>`}<span class="row-sub row-context" title="${esc([r.context, r.location].filter(Boolean).join(" · "))}">${esc(r.context)}</span></td>
      <td class="num metric" title="${esc(r.metricLabel)}">${esc(options.estimated ? r.detailMetric : r.metric)}</td>
      <td class="activity" data-state="${esc(r.protection && r.status !== "active" ? "protected" : r.status)}" title="${esc(r.idleLabel)}${r.idle != null ? ': ' + dur(r.idle) : ''}">${r.status === 'active' ? 'Now' : r.idle != null ? esc(compactDuration(r.idle)) + (r.idleIsDuration ? ' quiet' : options.estimated ? '' : ' ago') : 'Unknown'}${!options.estimated ? `<small>${esc(r.statusLabel)}</small>` : ""}</td>
      <td class="row-action">${r.eligible ? `<button class="close-row" data-resource-close ${controls(r)} data-list-control="${esc(key + ':close:' + r.id)}" aria-label="Close ${esc(r.title)}" title="Review close">×</button>` : `<span class="muted" title="${esc(r.protection)}">—</span>`}</td></tr>`;
  const body = options.grouped ? groups.map(group => {
    const open = !saved.collapsed.has(group.id), available = group.rows.filter(r => r.eligible);
    const selectedCount = available.filter(r => saved.selected.has(r.id)).length;
    const total = group.rows.reduce((sum, r) => sum + (r.rss || 0), 0);
    return `<tr class="resource-group" data-group="${group.id}"><td class="check"><label><input type="checkbox" data-list-group-all="${group.id}" data-list="${esc(key)}" data-list-control="${esc(key + ':group-all:' + group.id)}" aria-label="Select eligible ${group.label.toLowerCase()} tabs" ${available.length && selectedCount === available.length ? 'checked' : ''} ${!open || !available.length ? 'disabled' : ''}></label></td><td><button class="group-toggle" data-list-group="${group.id}" data-list="${esc(key)}" data-list-control="${esc(key + ':group:' + group.id)}" aria-expanded="${open}"><span class="group-chevron" aria-hidden="true">${open ? '⌄' : '›'}</span>${group.label}<span class="group-count">${group.rows.length}</span></button></td><td class="num metric" title="Estimated footprint of all tabs in this group">${total ? '≈ ' + bytes(total) : 'Unknown'}</td><td colspan="2" class="group-summary">${selectedCount ? `${selectedCount} selected` : ''}</td></tr>${open ? group.rows.map(rowMarkup).join('') : ''}`;
  }).join('') : rows.map(rowMarkup).join('');
  return `<section class="resource-list" aria-label="${esc(options.label)}">
    ${chosen.length ? `<div class="selection-bar has-selection"><span role="status"><b>${plural(chosen.length, options.noun)} selected</b>${memory ? ` · ${options.estimated ? '≈ ' : ''}${bytes(memory)}` : ''}</span><div class="list-actions"><button data-list-clear="${esc(key)}" data-list-control="${esc(key)}:clear">Clear</button><button class="primary" data-list-review="${esc(key)}" data-list-control="${esc(key)}:review">Review ${plural(chosen.length, options.noun)}</button></div></div>` : ''}
    <div class="table-scroll" data-list-scroll="${esc(key)}"><table class="resource-table"><thead><tr>
      <th class="check" scope="col"><label><input type="checkbox" data-list-all="${esc(key)}" data-list-control="${esc(key)}:all" aria-label="Select all eligible ${options.grouped ? "expanded " : ""}${options.plural}" ${chosen.length && chosen.length === eligible.length ? 'checked' : ''} ${eligible.length ? '' : 'disabled'}></label></th>
      ${sortHead(options.noun === "tab" ? "Tab" : "Session", options.noun === "tab" ? "title" : "name")}
      ${options.estimated ? `<th class="num metric" scope="col" title="Renderer memory divided evenly across tabs; individual tab usage is unavailable.">Est. memory</th>` : sortHead("Memory", "rss", "num metric")}
      ${sortHead(options.noun === "tab" ? "Last viewed" : "Last activity", "idle", "activity")}
      <th class="row-action" scope="col" aria-label="Close"></th></tr></thead><tbody>${body}</tbody></table></div>
    <p class="list-hint">${options.estimated ? 'Tab memory is estimated, not guaranteed savings.' : 'Shift-click to select a range.'}</p>
  </section>`;
}

function sessionTable(list) {
  const rows = filteredSessions(list).map(x => {
    const idle = x.idle_secs ?? x.quiet_for_secs, protection = sessionProtection(x);
    const engineNote = `${x.host}'s agent engine serves the app's threads. Close those threads in the app.`;
    return { id: sessionKey(x), data: x, title: x.engine ? `${AGENT_LABEL[x.kind] || "Agent"} backend` : sessionName(x), context: x.engine ? `${sessionCount([x])} · ${x.host}` : [x.project, x.host, x.first_prompt && x.first_prompt !== sessionName(x) ? x.first_prompt : ""].filter(Boolean).join(" · "), type: x.engine ? "Shared backend" : "Agent", status: x.state, statusLabel: protection || (x.state === "stale" ? "Stale" : "Idle"), idle, idleLabel: x.idle_secs != null ? "Last activity" : "CPU quiet for", idleIsDuration: x.idle_secs == null,
      metric: bytes(x.rss), metricLabel: x.engine ? "Shared memory across loaded tasks" : "In memory now", rss: x.rss, eligible: canCloseSession(x), protection,
      facts: [["Host", x.host], ["Open", dur(x.age_secs)], ["CPU", (x.cpu_window_mean ?? x.cpu ?? 0).toFixed(1) + "%"], ["PID", x.pid], ["Ports", (x.ports || []).join(", ") || "None"], [x.idle_secs != null ? "Last activity" : "CPU quiet for", x.state === "active" ? "Working now" : idle != null ? dur(idle) + (x.idle_secs != null ? " ago" : "") : "Unknown"]],
      note: x.engine ? engineNote : `${x.first_prompt && x.first_prompt !== sessionName(x) ? x.first_prompt + '\n\n' : ''}${x.idle_secs != null ? 'The transcript stays on disk. Available resume commands are saved in Actions before closing.' : 'Idle time comes from CPU observations; transcript activity is unavailable. Available resume commands are saved in Actions.'}`,
      action: protection ? `<button disabled>${esc(protection)} · kept open</button>` : `<button class="primary" data-close-session="${x.pid}">Review close</button>` };
  });
  const resourcesTable = resourceList("sessions", rows, { label: "Agent sessions", title: "Session / project", metric: "Memory", noun: "session", plural: "sessions", review: chosen => reviewSessions(chosen.map(r => r.data)), empty: "No sessions match. Try another search or choose All sessions." });
  const live = list.filter(x => !goneSession(x));
  const resources = live.length && live.every(x => x.engine && x.threads?.length)
    ? `<details class="task-details" data-keep-open="backend-resources"><summary>Shared backend resources · ${esc(bytes(live.reduce((sum, x) => sum + x.rss, 0)))}</summary>${resourcesTable}</details>`
    : resourcesTable;
  return resources + live.map(taskTable).join("");
}


function taskTable(x) {
  const threads = x.threads || [];
  if (!threads.length) return x.engine ? `<p class="help">Task details are unavailable in this snapshot. A backend can serve several tasks; its session count is not a task count.</p>` : "";
  const q = state.sessFilter.trim().toLowerCase();
  const matchesBackend = [x.session_name, x.project, x.first_prompt, x.host].join(" ").toLowerCase().includes(q);
  const visible = threads.filter(t => !q || matchesBackend || taskSearch(t).includes(q));
  const rows = visible.map(t => {
    const ago = t.last_activity == null || state.snap?.taken_at == null ? null : Math.max(0, state.snap.taken_at - t.last_activity);
    const activity = ago == null ? "Unknown" : `${dur(ago)} ago`;
    return { id: t.id || t.transcript, title: t.name || t.first_prompt || t.id || "Unnamed task", context: `${t.helper ? "Helper · " : ""}${t.cwd || "Project unknown"}`, type: t.helper ? "Helper task" : "Loaded task",
      metric: activity, metricLabel: "Last transcript activity", facts: [["Project", t.cwd || "Unknown"], ["Task ID", t.id || "Unknown"], ["Backend PID", x.pid], ["Visibility", "Transcript held open"], ["Memory / CPU", "Shared by the backend"]],
      note: "An open transcript shows this task is loaded. Last activity does not prove it is running or finished. Manage this task in its host app." };
  });
  const own = threads.filter(t => !t.helper).length, helpers = threads.length - own;
  return `<details class="task-details" data-keep-open="tasks:${esc(sessionKey(x))}" open><summary>${esc(plural(own, "loaded task"))}${helpers ? ` · ${esc(plural(helpers, "helper"))}` : ""} <span class="muted">· ${esc(x.host)} · PID ${x.pid}</span></summary><p class="help">Tasks with open transcripts appear here. Saved history is not counted. Memory and CPU are shared by the backend above.</p>${compactList("tasks:" + sessionKey(x), rows, { label: "Loaded tasks", title: "Task / project", metric: "Last activity", noun: "task", plural: "tasks", empty: "No loaded tasks match this search." })}</details>`;
}


function viewAgents(h) {
  const s = state.snap, live = h.sessions.filter(x => !goneSession(x)), visible = filteredSessions(h.sessions);
  const appRss = h.app ? Math.max(0, h.rss - h.sessions.reduce((n, x) => n + x.rss, 0)) : 0;
  const ports = s.ports.filter(p => h.pids.includes(p.pid) && !h.sessions.some(x => (x.pids || []).includes(p.pid)));
  const trend = (s.trends || []).find(t => t.key === h.key && t.span_secs >= 600);
  return `<h1>${holderIcon(h)}${esc(h.name)} <span class="pill">Agent</span></h1><div class="sub"><span><b>${bytes(h.rss)}</b> across ${plural(h.procs, "process")}</span><span>${sessionCount(live)}</span><span>${h.cpu.toFixed(1)}% CPU</span></div>
    <div class="list-toolbar"><label class="search">${icon("search")}<input type="search" data-session-filter aria-label="Search sessions" placeholder="Search tasks, sessions or projects" value="${esc(state.sessFilter)}"></label><select data-sess-sort aria-label="Sort sessions">${["rss", "idle", "age", "name"].map(k => `<option value="${k}" ${state.sessSort === k ? "selected" : ""}>${(state.sessReverse && state.sessSort === k ? {rss:"Least memory",idle:"Shortest idle",age:"Newest",name:"Name Z–A"} : {rss:"Most memory",idle:"Longest idle",age:"Oldest",name:"Name A–Z"})[k]}</option>`).join("")}</select></div>
    <div class="filter-row">${filterChips("session", state.sessState, [["all", "All sessions"], ["stale", "Stale"], ["active", "Active"]])}<span class="muted">${visible.length} shown</span></div>
    ${sessionTable(visible)}<p class="help">Select rows to close several sessions together. Active sessions and app engines stay open.</p>
    ${h.app ? `<details class="help" data-keep-open="app-memory"><summary>What is included in ${bytes(h.rss)}?</summary><p>The ${esc(h.app)} app holds ${bytes(appRss)}. The remaining memory belongs to its sessions, including those running in terminals.</p></details>` : ""}
    ${holderMemoryHelp(h)}${trend ? trendsTable([trend]) : ""}${ports.length ? `<h2>Listening ports</h2>${portsTable(ports)}` : ""}${quitBlock(h)}`;
}


function tabTable(b, tabs) {
  const rows = tabs.map(t => {
    const profile = b.open_profiles.find(p => p.dir === t.profile)?.label || t.profile;
    const protection = t.pinned ? "Pinned" : t.active ? "In use" : !b.can_close_tabs ? "Closing unavailable" : "";
    const status = t.active ? "active" : isStale(b, t) ? "stale" : "idle";
    return { id: tabKey(b, t), data: t, group: status === "stale" ? "stale" : t.active || t.idle_secs != null ? "recent" : "unknown", title: t.title.trim() || t.url, context: t.url.replace(/^https?:\/\//, ""), location: `${profile} · Window ${t.window_id}${t.index != null ? ` · Tab ${t.index}` : ""}`, type: "Browser tab", status, statusLabel: protection || (status === "stale" ? "Stale" : "Idle"), idle: t.idle_secs, idleLabel: "Last viewed", metric: t.active ? "Now" : t.idle_secs != null ? compactDuration(t.idle_secs) + " ago" : "Not viewed", detailMetric: b.per_tab_estimate ? "≈ " + bytes(b.per_tab_estimate) : "Unknown", metricLabel: "Renderer memory ÷ all tabs (estimate)", rss: b.per_tab_estimate, eligible: canCloseTab(b, t), protection,
      facts: [["Profile", profile], ["Last viewed", t.active ? "Now" : t.idle_secs != null ? dur(t.idle_secs) + " ago" : "Not viewed"], ["Protection", protection || "None"], ["URL", t.url]], note: "Reopen with ⌘⇧T in the same profile. The URL is saved in Actions; unsaved text may not return.",
      action: protection ? `<button disabled>${esc(protection)}${b.can_close_tabs ? ' · kept open' : ''}</button>` : `<button class="primary" data-close-tab="${t.id}" data-browser="${esc(b.name)}">Review close</button>` };
  });
  return resourceList("tabs", rows, { label: "Browser tabs", title: "Tab / site", metric: "Last viewed", noun: "tab", plural: "tabs", estimated: true, grouped: state.tabSort === "idle" && state.tabState === "all" && !state.tabFilter.trim(), review: chosen => reviewTabs(b, chosen.map(r => r.data)), empty: "No tabs match. Try another search, state, or profile." });
}


function viewBrowser(h) {
  const s = state.snap, b = h.browser, est = b.per_tab_estimate;
  const live = b.tabs.filter(t => !goneTab(t)), visible = filteredTabs(b);
  const matching = filteredTabs(b, "all"), stale = visible.filter(t => isStale(b, t) && canCloseTab(b, t));
  const counts = { all: matching.length, stale: matching.filter(t => isStale(b, t) && !t.pinned).length, chat: matching.filter(t => t.kind === "chat").length };
  const threshold = state.settings?.tab_stale_after_secs ?? 86400;
  const profile = b.open_profiles.find(p => p.dir === state.tabProfile)?.label || state.tabProfile;
  let html = `<div class="browser-heading"><h1>${holderIcon(h)}${esc(h.name)}</h1><span>${bytes(h.rss)} · ${plural(live.length, "tab")}</span></div>`;
  if (!live.length) return html + `<p class="note">No tabs to show. ${esc(b.tabs_note || "")}</p>` + holderMemoryHelp(h) + quitBlock(h);
  if (stale.length) html += `<section class="cleanup-summary" aria-label="Stale tab cleanup"><div><strong>${plural(stale.length, 'tab')} untouched for ${compactDuration(threshold)} or more</strong><p>${est ? `<b>≈ ${bytes(stale.length * est)}</b> estimated footprint` : 'Ready to review'}${state.tabFilter || state.tabProfile || state.tabState !== 'all' ? ' in these results' : ''}</p></div><button class="primary" data-close-tabs="${stale.map(t => t.id).join(',')}" data-browser="${esc(b.name)}">Review ${plural(stale.length, 'stale tab')}</button></section>`;
  html += `<div class="list-toolbar browser-toolbar"><label class="search">${icon("search")}<input type="search" data-tab-filter aria-label="Search tabs" placeholder="Search titles or URLs" value="${esc(state.tabFilter)}"></label>
    ${filterChips("tab", state.tabState, [["all", `All ${counts.all}`], ["stale", `Stale ${counts.stale}`], ["chat", `Chats ${counts.chat}`]])}
    <details class="list-options" data-keep-open="tab-options"><summary aria-label="View options" title="Profile and sort options">${icon("settings")}${profile ? `<span>${esc(profile)}</span>` : ''}</summary><div class="list-options-panel">
    <label>Profile<select data-tab-profile aria-label="Filter by profile"><option value="">All profiles</option>${b.open_profiles.map(p => `<option value="${esc(p.dir)}" ${state.tabProfile === p.dir ? "selected" : ""}>${esc(p.label)}</option>`).join("")}</select></label>
    <label>Sort by<select data-tab-sort aria-label="Sort tabs">${["idle", "site", "title", "window"].map(k => `<option value="${k}" ${state.tabSort === k ? "selected" : ""}>${(state.tabReverse && state.tabSort === k ? {idle:"Recently viewed",site:"Site Z–A",title:"Title Z–A",window:"Reverse window order"} : {idle:"Longest untouched",site:"Site A–Z",title:"Title A–Z",window:"Window order"})[k]}</option>`).join("")}</select></label></div></details></div>
    ${tabTable(b, visible)}
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
    <div class="sub"><span><b>${bytes(h.rss)}</b> process total</span><span><b>${h.cpu.toFixed(1)}%</b> cpu</span><span><b>${plural(h.procs, "process")}</b></span>${trend ? `<span>${trend.growth >= 0 ? "grew" : "shrank"} <b>${bytes(Math.abs(trend.growth))}</b> over ${dur(trend.span_secs)}</span>` : ""}</div>`;
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
    html += `<p class="note">${plural(h.procs, "process")} under this app bundle, ${bytes(h.rss)} in memory between them.</p>`;
  }
  return html + holderMemoryHelp(h) + quitBlock(h);
}


function sitesTable(b, tabs) {
  const rows = sitesOf(b, tabs).slice(0, 12).map(st => {
    const closable = tabs.filter(t => t.site === st.site && canCloseTab(b, t)), stale = closable.filter(t => isStale(b, t));
    const domains = siteHostChoices(b, st.site, tabs);
    const domainAction = /^(Google )?Chrome$/.test(b.name) && domains.length ? `<button data-auto-domain-site="${esc(st.site)}" data-browser="${esc(b.name)}" ${state.settings?.config_error ? "disabled" : ""}>Auto-close this domain…</button>` : "";
    return { id: st.site, title: st.site, context: `${plural(st.tabs, "tab")} · ${st.stale_tabs} stale`, type: "Site", metric: b.per_tab_estimate ? "≈ " + bytes(st.est_rss) : "—", metricLabel: "Average per tab × selected site tabs (estimate)", facts: [["Tabs", st.tabs], ["Stale", st.stale_tabs], ["Oldest untouched", st.oldest_idle_secs != null ? dur(st.oldest_idle_secs) : "Unknown"]], note: "These counts follow your search and profile filters. Pinned and active tabs stay open.", action: `<button class="primary" ${stale.length ? '' : 'disabled'} data-close-tabs="${stale.map(t => t.id).join(',')}" data-browser="${esc(b.name)}">Review ${plural(stale.length, 'stale tab')}</button><button ${closable.length ? '' : 'disabled'} data-close-tabs="${closable.map(t => t.id).join(',')}" data-browser="${esc(b.name)}">Review all ${plural(closable.length, 'eligible tab')}</button>${domainAction}` };
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
  return `<br><button type="button" class="copy-command" data-copy-command="${esc(command)}" title="Copy to clipboard" aria-label="Copy recovery command for ${esc(r.target)}: ${esc(command)}">
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
  return `<div class="memory-observation"><p><b>Observed whole-machine change${m.tab_batch ? ` · batch of ${plural(m.attempted_actions, "tab attempt")}` : ""}</b></p><p>RAM used: ${bytes(m.before.used_mem)} → ${bytes(m.after.used_mem)} (${change(m.before.used_mem, m.after.used_mem)})<br>Swap used: ${bytes(m.before.used_swap)} → ${bytes(m.after.used_swap)} (${change(m.before.used_swap, m.after.used_swap)})</p><p class="help">Over ${elapsed.toFixed(1)} seconds, including a 1-second settling delay. Includes other activity; this is not savings attributable to ${m.tab_batch ? "an individual tab" : "this action"}.</p></div>`;
}

function viewActions() {
  const labels = {close_session: "Close session", stop_server: "Stop server", close_tab: "Close tab", quit_app: "Quit app", restart_app: "Restart app"};
  return `<h1>Actions</h1><div class="sub">What happened, why it happened, and how to get back to it.</div>` + (state.log.length ? state.log.slice().reverse().map(r => {
    const preview = r.mode === "dry-run", failed = /fail|error|refus|still running|could not/i.test(r.result);
    const mode = preview ? "Preview only" : r.mode === "auto" ? "Automatic" : "Manual";
    const key = `${r.ts}:${r.action}:${r.pid}:${r.target}`;
    return `<article class="history-entry"><div class="event-head"><span class="event-icon ${preview ? "neutral" : failed ? "warning" : /terminated|closed|stopped|quit and relaunched/i.test(r.result) ? "" : "neutral"}">${icon(preview || failed ? "info" : /terminated|closed|stopped|quit and relaunched/i.test(r.result) ? "check" : "history")}</span><b>${esc(labels[r.action] || r.action.replace(/_/g, " "))}</b><span class="tag ${preview ? "" : "on"}">${mode}</span><time>${esc(new Date(r.ts * 1000).toLocaleString())}</time></div><div class="event-body"><strong class="result">${esc(r.target)}</strong><dl class="event-facts"><div><dt>Result</dt><dd class="${failed ? "state stale" : ""}">${preview ? "Nothing was closed. " : ""}${esc(r.result)}</dd></div>${r.rss ? `<div><dt>Memory context</dt><dd>${r.action === "close_tab" ? "≈ " : ""}${bytes(r.rss)} ${preview ? "held when evaluated" : "held before the action"}${r.action === "close_tab" ? " (estimated)" : ""}</dd></div>` : ""}</dl>${!preview ? observedMemory(r) : ""}${r.resume && !preview ? `<details data-keep-open="${esc(key)}"><summary>${r.action === "close_session" ? "How to resume" : "How to reopen"}</summary>${recoveryControl(r)}</details>` : ""}</div></article>`;
  }).join("") + `<p class="help">Memory shown is what a target held before the action, not a measurement of memory recovered. Observed RAM and swap changes cover the whole machine, include other activity, and must not be added up as savings.</p>` : `<div class="note">No actions yet. When you close something, its result and available recovery instructions will appear here.</div>`);
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
