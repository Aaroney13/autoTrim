// IPC, confirmation, polling, and event handlers. Renderer callbacks are injected.
import { icon, bytes, dur, esc, plural } from "./format.js";
import { recordActivity, domainInactivityLabel, domainRecommendations, state, armed, listModels, holders, holderByKey, appIconName, sync, autoMode, siteHostChoices, canCloseSession, canCloseTab, filteredSessions, filteredTabs, autoModeValues, actionSucceeded } from "./state.js";

const invoke = (...a) => window.__TAURI__.core.invoke(...a);

export function createActions({ renderAll, renderMain, renderSide, renderHeader }) {
function toast(msg) { const t = document.getElementById("toast"); t.textContent = msg; t.style.display = "block"; clearTimeout(t._h); t._h = setTimeout(() => t.style.display = "none", 8000); }


async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    try { await navigator.clipboard.writeText(text); return; } catch (_) { /* Try the web view fallback below. */ }
  }
  // Some embedded web views do not expose the async Clipboard API.
  const active = document.activeElement;
  const input = document.createElement("textarea");
  input.value = text;
  input.readOnly = true;
  input.style.cssText = "position:fixed;top:0;left:-9999px";
  document.body.append(input);
  try {
    input.select();
    if (!document.execCommand("copy")) throw new Error("Copy failed");
  } finally {
    input.remove();
    active?.focus({ preventScroll: true });
  }
}


async function copyCommand(button) {
  if (button.classList.contains("copying")) return;
  const label = button.querySelector("span");
  clearTimeout(button._copyTimer);
  button.classList.remove("copied", "copy-feedback");
  button.classList.add("copying");
  button.setAttribute("aria-busy", "true");
  button.title = "Copy to clipboard";
  label.textContent = "Copying…";
  try {
    await copyText(button.querySelector("code").textContent);
    button.classList.add("copied");
    button.title = "Copied to clipboard";
    label.textContent = "✓ Copied!";
    toast("Copied to clipboard.");
  } catch (_) {
    label.textContent = "Try again";
    toast("Couldn't copy. Select the command and copy it manually.");
  } finally {
    button.removeAttribute("aria-busy");
    button.classList.remove("copying");
    button.classList.add("copy-feedback");
    button._copyTimer = setTimeout(() => {
      button.classList.remove("copied", "copy-feedback");
      button.title = "Copy to clipboard";
      label.textContent = "Copy";
    }, 2500);
  }
}

// Two-step buttons: the first click arms for five seconds, the second acts.

function twoStep(btn, key, run) {
  const label = btn.dataset.label ?? (btn.dataset.label = btn.textContent);
  const now = Date.now();
  if (!armed[key] || now - armed[key] > 5000) {
    armed[key] = now; btn.textContent = "Confirm"; btn.classList.add("confirm");
    setTimeout(() => { if (armed[key] === now) { btn.textContent = label; btn.classList.remove("confirm"); delete armed[key]; } }, 5000);
    return;
  }
  delete armed[key]; btn.disabled = true; btn.textContent = "…";
  run().catch(e => toast(String(e))).finally(afterAction);
}

// After an action: redraw at once with what is already known (the rows
// just closed say so), then scan for real instead of waiting on the
// daemon's next tick. Browsers write their session file a few seconds
// after a tab closes, so look a second time.

function afterAction() {
  renderAll(false);
  refresh({ fresh: true });
  setTimeout(() => refresh({ fresh: true }), 3500);
}


function report(recs) {
  const list = Array.isArray(recs) ? recs : [recs];
  toast(list.map(r => `${r.action.replace(/_/g, " ")} · ${r.target} · ${r.result}` + (r.resume ? `\n↩ ${r.resume}` : "")).join("\n\n"));
}

// ---- the holders list: everything the sidebar orders by memory ----

async function loadAppIcons() {
  const names = [...new Set((state.snap?.groups || []).map(appIconName))]
    .filter(name => name && !state.appIcons.has(name) && !state.appIconsPending.has(name)).slice(0, 64);
  if (!names.length) return;
  names.forEach(name => state.appIconsPending.add(name));
  try {
    const icons = await invoke("app_icons", { names });
    for (const name of names) {
      const data = icons?.[name];
      state.appIcons.set(name, typeof data === "string" && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(data) ? data : null);
    }
    renderAll(false);
  } catch (_) { /* Keep the fallback and retry on the next snapshot. */ }
  finally { names.forEach(name => state.appIconsPending.delete(name)); }
}

async function refresh(opts = {}) {
  const requestId = ++sync.requestId;
  sync.lastPoll = Date.now();
  if (opts.fresh) { sync.scans++; if (state.snap && state.src) renderHeader(); }
  const revision = state.settingsRevision;
  let snap, src, log, settings, service, update;
  try {
    [snap, src, log, settings, service, update] = await Promise.all([invoke("snapshot", { fresh: !!opts.fresh }), invoke("source_info"), invoke("action_log").catch(() => state.log), invoke("settings").catch(() => null), invoke("service_info").catch(() => null), invoke("update_status").catch(() => null)]);
    if (!state.updateBusy) state.update = update;
  } catch (e) { document.getElementById("head").innerHTML = `<span class="muted">${esc(String(e))}</span>`; return; }
  finally { if (opts.fresh) sync.scans--; }
  const first = !state.snap;
  if (!first && (snap.taken_at < state.snap.taken_at || snap.taken_at === state.snap.taken_at && (sync.acceptedFresh && !opts.fresh || !!opts.fresh === sync.acceptedFresh && requestId < sync.acceptedRequest))) { state.src = src; state.log = log; if (revision === state.settingsRevision && !state.settingsBusy) state.settings = settings; state.service = service; renderAll(false); return; }
  sync.acceptedRequest = requestId; sync.acceptedFresh = !!opts.fresh;
  state.snap = snap; state.src = src; state.log = log; if (revision === state.settingsRevision && !state.settingsBusy) state.settings = settings; state.service = service;
  recordActivity(snap);
  pruneGone(snap);
  renderAll(first);
  void loadAppIcons();
}


function navigate(view) {
  if (state.view !== view) { state.sessFilter = ""; state.sessState = "all"; state.tabFilter = ""; state.tabProfile = ""; state.tabState = "all"; state.listState.clear(); state.searchOpen.clear(); }
  state.view = view; renderSide(); renderMain(true);
}


async function saveAuto(values) {
  if (state.settingsBusy) return;
  state.settingsBusy = true; ++state.settingsRevision; renderMain(false, true);
  try { state.settings = await invoke("set_auto", values); }
  catch (e) { toast(String(e)); }
  finally { state.settingsBusy = false; ++state.settingsRevision; renderSide(); renderMain(false, true); refresh(); }
}

async function saveTabRules(domains, inactiveHours) {
  if (state.settingsBusy) return false;
  state.settingsBusy = true; ++state.settingsRevision; renderMain(false, true);
  try {
    state.settings = await invoke("set_tab_rules", { domains, inactive_hours: inactiveHours, expected_revision: state.settings?.tab_rules_revision ?? "" });
    return true;
  } catch (e) {
    const message = String(e);
    if (state.domainEditor) {
      state.domainEditor.error = message;
      const error = document.getElementById("domain-error");
      if (error) error.textContent = message;
    } else toast(message);
    return false;
  } finally {
    state.settingsBusy = false; ++state.settingsRevision; renderSide(); renderMain(false, true); refresh();
  }
}

async function removeDomainRule(index) {
  const rules = state.settings?.auto_tab_domains || [];
  if (index < 0 || index >= rules.length) return false;
  return saveTabRules(rules.filter((_, i) => i !== index), state.settings?.auto_tab_inactive_hours ?? 24);
}

async function requestTabPreview(domains, inactiveHours) {
  const requestId = ++state.tabPreview.requestId;
  state.tabPreview.busy = true;
  state.tabPreview.error = "";
  try {
    const rows = await invoke("preview_tab_rules", { domains, inactive_hours: inactiveHours });
    if (requestId !== state.tabPreview.requestId) return false;
    state.tabPreview.rows = rows;
    return true;
  } catch (e) {
    if (requestId !== state.tabPreview.requestId) return false;
    state.tabPreview.rows = [];
    state.tabPreview.error = String(e);
    return false;
  } finally {
    if (requestId === state.tabPreview.requestId) state.tabPreview.busy = false;
  }
}

const draftDomain = value => String(value || "").trim().toLowerCase().replace(/\.$/, "");

function existingDomainIndex(domain, except = -1) {
  const canonical = draftDomain(domain);
  return (state.settings?.auto_tab_domains || []).findIndex((rule, index) => index !== except && draftDomain(rule.domain) === canonical);
}

function openDomainEditor({ index = null, domain = "", hostChoices = [] } = {}) {
  if (index == null && domain) {
    const existing = existingDomainIndex(domain);
    if (existing >= 0) index = existing;
  }
  const rule = index == null ? null : state.settings?.auto_tab_domains?.[index];
  state.domainEditor = {
    index,
    domain: rule?.domain ?? domain,
    includeSubdomains: rule?.include_subdomains ?? false,
    revision: state.settings?.tab_rules_revision ?? "",
    hostChoices,
    suggestions: index == null && !hostChoices.length ? domainRecommendations() : null,
    error: "",
  };
  renderDomainEditor();
}

function selectEditorHost(domain) {
  if (!state.domainEditor) return;
  const existing = existingDomainIndex(domain);
  const rule = existing >= 0 ? state.settings.auto_tab_domains[existing] : null;
  Object.assign(state.domainEditor, {
    index: existing >= 0 ? existing : null,
    domain,
    includeSubdomains: rule?.include_subdomains ?? false,
    revision: state.settings?.tab_rules_revision ?? "",
    error: "",
  });
  renderDomainEditor(true);
}

function renderDomainSuggestions() {
  const editor = state.domainEditor, root = document.getElementById("domain-suggestions");
  if (!editor?.suggestions || !root) return;
  const query = draftDomain(editor.domain);
  const matches = editor.suggestions.filter(row => row.domain.includes(query));
  const empty = editor.suggestions.length
    ? "No matching suggestions. You can still add this domain."
    : "No new domains to suggest from your Chrome tabs. You can still enter a domain above.";
  root.innerHTML = `<div class="domain-suggestions-heading"><h3 id="domain-suggestions-title">Suggested domains</h3><span class="muted" role="status">${plural(matches.length, "domain")}</span></div>
    <p>From your latest Chrome scan, with inactive tabs first. Type above to filter.</p>
    ${matches.length ? `<div class="domain-suggestion-list">${matches.map((row, index) => {
      const selected = query === row.domain;
      const detail = row.inactiveCount ? `${row.inactiveCount} inactive · longest ${dur(row.oldestIdleSecs)}` : "None past the timer";
      return `<button type="button" class="domain-suggestion" data-domain-suggestion="${esc(row.domain)}" aria-label="Use ${esc(row.domain)}" aria-describedby="domain-suggestion-detail-${index}" aria-pressed="${selected}"><span><b>${esc(row.domain)}</b><small id="domain-suggestion-detail-${index}">${plural(row.count, "tab")} · ${esc(detail)}</small></span><span class="domain-suggestion-use" aria-hidden="true">${selected ? "Selected" : "Use domain"}</span></button>`;
    }).join("")}</div>` : `<div class="domain-suggestions-empty">${empty}</div>`}`;
  root.querySelectorAll("[data-domain-suggestion]").forEach(button => button.onclick = () => {
    editor.domain = button.dataset.domainSuggestion;
    editor.error = "";
    document.getElementById("domain-input").value = editor.domain;
    document.getElementById("domain-error").textContent = "";
    renderDomainSuggestions();
    document.getElementById("domain-submit").focus();
  });
}

function renderDomainEditor(focusDomain = false) {
  const editor = state.domainEditor, dialog = document.getElementById("domain-editor");
  if (!editor || !dialog) return;
  const editing = editor.index != null;
  const choices = editor.hostChoices.length > 1 ? `<label class="field">Open hostname<select id="domain-host-choice">${editor.hostChoices.map(choice => `<option value="${esc(choice.domain)}" ${choice.domain === editor.domain ? "selected" : ""}>${esc(choice.domain)} (${plural(choice.count, "tab")})</option>`).join("")}</select></label>` : "";
  dialog.innerHTML = `<form method="dialog" class="review-content domain-editor-form" id="domain-editor-form">
    <h2 id="domain-editor-title">${editing ? "Edit auto-close domain" : "Add auto-close domain"}</h2>
    <p id="domain-editor-description">Use a hostname only. It applies to HTTP and HTTPS on every port, across all Chrome profiles.</p>
    ${choices}
    <label class="field">Domain<input id="domain-input" name="domain" type="text" inputmode="url" autocomplete="off" spellcheck="false" value="${esc(editor.domain)}" placeholder="example.com" required></label>
    <label class="check-field"><input id="domain-subdomains" type="checkbox" ${editor.includeSubdomains ? "checked" : ""}>Include subdomains</label>
    ${editor.suggestions ? `<section id="domain-suggestions" aria-labelledby="domain-suggestions-title"></section>` : ""}
    <div class="readonly-setting"><span>Inactive for</span><b>${esc(domainInactivityLabel(state.settings?.auto_tab_inactive_hours ?? 24))}</b></div>
    <p class="muted">The shared timer is managed in Settings. Selected and pinned tabs stay open.</p>
    <p class="field-error" id="domain-error" role="alert">${esc(editor.error)}</p>
    <div class="review-actions"><button type="button" id="domain-cancel">Cancel</button><button type="submit" class="primary" id="domain-submit">${editing ? "Save changes" : "Add domain"}</button></div>
  </form>`;
  dialog.oncancel = () => { state.domainEditor = null; };
  dialog.onclose = () => { state.domainEditor = null; };
  const input = dialog.querySelector("#domain-input"), include = dialog.querySelector("#domain-subdomains");
  input.oninput = () => { editor.domain = input.value; editor.error = ""; dialog.querySelector("#domain-error").textContent = ""; renderDomainSuggestions(); };
  include.onchange = () => { editor.includeSubdomains = include.checked; };
  dialog.querySelector("#domain-host-choice")?.addEventListener("change", event => selectEditorHost(event.target.value));
  dialog.querySelector("#domain-cancel").onclick = () => dialog.close();
  dialog.querySelector("#domain-editor-form").onsubmit = async event => {
    event.preventDefault();
    editor.domain = input.value;
    editor.includeSubdomains = include.checked;
    if (!draftDomain(editor.domain)) { editor.error = "Enter a domain."; dialog.querySelector("#domain-error").textContent = editor.error; input.focus(); return; }
    if (editor.revision !== (state.settings?.tab_rules_revision ?? "")) { editor.error = "Domain rules changed outside this editor. Review the current list and try again."; dialog.querySelector("#domain-error").textContent = editor.error; input.focus(); return; }
    const duplicate = existingDomainIndex(editor.domain, editor.index ?? -1);
    if (duplicate >= 0) { editor.error = "That domain is already in the list."; dialog.querySelector("#domain-error").textContent = editor.error; input.focus(); return; }
    const rules = [...(state.settings?.auto_tab_domains || [])];
    const next = { domain: editor.domain, include_subdomains: editor.includeSubdomains };
    if (editor.index == null) rules.push(next); else rules[editor.index] = next;
    dialog.querySelectorAll("button, input, select").forEach(control => control.disabled = true);
    const ok = await saveTabRules(rules, state.settings?.auto_tab_inactive_hours ?? 24);
    if (!ok) { dialog.querySelectorAll("button, input, select").forEach(control => control.disabled = false); input.focus(); return; }
    dialog.close();
    toast(autoMode(state.settings) === "off" ? "Domain saved. Auto mode is off." : !state.settings.auto_close_tabs ? "Domain saved. Enable domain cleanup in Settings." : "Domain saved.");
  };
  renderDomainSuggestions();
  if (!dialog.open) dialog.showModal();
  (focusDomain ? dialog.querySelector("#domain-input") : dialog.querySelector(editor.hostChoices.length > 1 ? "#domain-host-choice" : "#domain-input"))?.focus();
}

const previewStatus = status => ({ eligible: "Eligible", waiting: "Waiting for inactivity", pinned: "Pinned", selected: "Selected", unknown_activity: "Unknown activity", unavailable: "Unavailable" })[status] || status;

function renderTabPreview() {
  const dialog = document.getElementById("tab-preview"), preview = state.tabPreview;
  if (!dialog) return;
  const rows = preview.rows || [];
  const eligible = rows.filter(row => row.status === "eligible").length;
  const waiting = rows.filter(row => row.status === "waiting").length;
  const protectedCount = rows.length - eligible - waiting;
  dialog.innerHTML = `<div class="review-content preview-content"><h2 id="tab-preview-title">Review matching Chrome tabs</h2><p id="tab-preview-description">Read-only preview. This does not close tabs or change Auto mode.</p>
    ${preview.busy ? `<p class="muted" role="status">Scanning current Chrome session data…</p>` : preview.error ? `<p class="field-error" role="alert">${esc(preview.error)}</p>` : rows.length ? `<div class="preview-summary"><b>${plural(eligible, "eligible tab")}</b><span>${plural(protectedCount, "protected tab")}</span>${waiting ? `<span>${plural(waiting, "waiting tab")}</span>` : ""}</div><div class="preview-table-wrap"><table class="preview-table"><thead><tr><th>Tab / domain</th><th>Profile</th><th>Last selected</th><th>Status</th></tr></thead><tbody>${rows.map(row => `<tr><td><span class="l1">${esc(row.title || "Untitled tab")}</span><span class="l2">${esc(row.domain || "Unknown domain")}</span></td><td>${esc(row.profile || "Unknown")}</td><td>${row.idle_secs == null ? "Unknown" : esc(dur(row.idle_secs)) + " ago"}</td><td><span class="state ${row.status === "eligible" ? "stale" : row.status === "waiting" ? "idle" : "active"}">${esc(previewStatus(row.status))}</span>${row.reason ? `<span class="l2">${esc(row.reason)}</span>` : ""}</td></tr>`).join("")}</tbody></table></div>` : `<div class="note">No open Chrome tabs match the saved domain list.</div>`}
    <div class="review-actions"><button id="tab-preview-close" autofocus>Done</button></div></div>`;
  dialog.querySelector("#tab-preview-close").onclick = () => dialog.close();
}

async function openTabPreview() {
  const dialog = document.getElementById("tab-preview");
  if (!dialog) return;
  state.tabPreview.rows = [];
  state.tabPreview.error = "";
  state.tabPreview.busy = true;
  renderTabPreview();
  if (!dialog.open) dialog.showModal();
  const requestId = state.tabPreview.requestId + 1;
  await requestTabPreview(state.settings?.auto_tab_domains || [], state.settings?.auto_tab_inactive_hours ?? 24);
  if (state.tabPreview.requestId === requestId && dialog.open) renderTabPreview();
}

// The dialog owns a frozen set of reviewed identities. Polling may update
// the app behind it, but never changes the user's targets or checkboxes.

function reviewSessions(list) {
  const targets = list.filter(canCloseSession).map(x => ({ id: x.pid, name: x.session_name || x.project || `Session ${x.pid}`, detail: [x.project, x.host, x.idle_secs != null ? `idle ${dur(x.idle_secs)}` : ""].filter(Boolean).join(" · "), rss: x.rss, startTime: x.start_time }));
  openReview("sessions", targets);
}

async function closeTabs(b, list) {
  if (state.tabsClosing) return;
  // Freeze identities before IPC; the native action rechecks URL, profile,
  // and protection state immediately before closing each tab.
  const targets = list.filter(t => canCloseTab(b, t)).map(t => ({ id: t.id, url: t.url, profile: t.profile }));
  if (!targets.length) return;
  state.tabsClosing = true;
  renderMain(false, true);
  try {
    const records = await invoke("close_tabs", { browser: b.name, expectedTabs: targets });
    markGone("tabs", targets.map(t => t.id), records, actionSucceeded);
    const completed = records.filter(actionSucceeded).length;
    const remaining = targets.length - completed;
    toast(`${plural(completed, "tab")} closed or already gone.${remaining ? ` ${plural(remaining, "tab")} stayed open. Check Actions for details.` : " Reopen with ⌘⇧T. Recovery instructions are in Actions."}`);
  } catch (e) {
    toast(`Could not complete the request: ${e}\nCheck Actions and refresh before trying again.`);
  } finally {
    state.tabsClosing = false;
    afterAction();
  }
}

function reviewPorts(list) {
  const processes = new Map();
  list.filter(p => !p.owner_managed).forEach(p => {
    if (!processes.has(p.pid)) processes.set(p.pid, { id: p.pid, name: p.label || p.process, detail: `${p.owner} · ${p.process} (${p.pid}) · ports ${state.snap.ports.filter(port => port.pid === p.pid).map(port => port.port).join(", ")}`, rss: 0, startTime: p.start_time });
  });
  openReview("ports", [...processes.values()]);
}

const reviewNoun = kind => kind === "ports" ? "server" : "session";

const reviewVerb = kind => kind === "ports" ? "Stop" : "Close";

function openReview(kind, targets) {
  if (state.actionReview || !targets.length) return;
  state.actionReview = { kind, targets, selected: new Set(targets.map(t => t.id)), busy: false, returnView: state.view };
  const dialog = document.getElementById("action-review"), noun = reviewNoun(kind), verb = reviewVerb(kind);
  dialog.innerHTML = `<div class="review-content"><h2 id="review-title">${verb} these ${noun}s?</h2><p id="review-description">Review the exact targets below. Uncheck anything you want to keep.</p><div class="review-targets">${targets.map(t => `<label class="review-target"><input type="checkbox" data-review-target="${t.id}" checked><span><span class="l1">${esc(t.name)}</span><span class="l2" style="display:block">${esc(t.detail)}</span></span><span class="num">${t.rss ? `${bytes(t.rss)}` : ""}</span></label>`).join("")}</div><div class="review-total"><span id="review-count"></span><span id="review-memory"></span></div><div class="recovery-note">${icon("resume")}<span>${kind === "ports" ? "Stopping a server ends its process and closes all ports it owns. Restart it from the terminal or app that launched it." : "Closing stops the session’s processes. Its transcript stays on disk. Available resume commands are saved in Actions."}</span></div><p>${kind === "ports" ? "Each process is checked again before stopping. Services managed by an app or the system are skipped." : "Active sessions, app engines, and sessions whose process has changed are skipped when checked before closing."}</p><p id="review-status" role="status"></p><div class="review-actions"><button id="review-cancel" autofocus>Cancel</button><button class="primary" id="review-submit">${verb} ${plural(targets.length, noun)}</button></div></div>`;
  dialog.oncancel = event => { if (state.actionReview?.busy) event.preventDefault(); };
  dialog.onclose = () => { state.actionReview = null; };
  dialog.querySelectorAll("[data-review-target]").forEach(cb => cb.onchange = () => { cb.checked ? state.actionReview.selected.add(+cb.dataset.reviewTarget) : state.actionReview.selected.delete(+cb.dataset.reviewTarget); updateReviewTotal(); });
  document.getElementById("review-cancel").onclick = () => dialog.close();
  document.getElementById("review-submit").onclick = executeReview;
  updateReviewTotal(); dialog.showModal();
}

function updateReviewTotal() {
  const r = state.actionReview, chosen = r.targets.filter(t => r.selected.has(t.id));
  document.getElementById("review-count").textContent = `${plural(chosen.length, reviewNoun(r.kind))} selected`;
  const rss = chosen.reduce((n, t) => n + t.rss, 0);
  document.getElementById("review-memory").innerHTML = rss ? `<b>${bytes(rss)}</b> held now, not RAM savings` : "";
  const button = document.getElementById("review-submit");
  button.textContent = `${reviewVerb(r.kind)} ${plural(chosen.length, reviewNoun(r.kind))}`;
  button.disabled = !chosen.length || r.busy;
}

async function executeReview() {
  const r = state.actionReview;
  if (!r || r.busy || !r.selected.size) return;
  r.busy = true;
  const dialog = document.getElementById("action-review"), status = document.getElementById("review-status");
  dialog.querySelectorAll("button, input").forEach(el => el.disabled = true);
  status.textContent = `Checking the selected targets and ${r.kind === "ports" ? "stopping" : "closing"}…`;
  const targets = r.targets.filter(t => r.selected.has(t.id)), results = [];
  try {
    for (const target of targets) {
      try {
        const record = r.kind === "ports" ? await invoke("stop_server", { pid: target.id, force: false, expectedStartTime: target.startTime }) : await invoke("close_session", { pid: target.id, expectedStartTime: target.startTime });
        if (r.kind === "sessions" && actionSucceeded(record)) state.gone.pids.add(target.id);
        results.push(record);
      } catch (e) { results.push({ target: target.name, result: `Skipped: ${e}` }); }
    }
    const ok = record => actionSucceeded(record);
    const completed = results.filter(ok).length, failed = results.filter(record => !ok(record));
    status.textContent = `${completed} of ${plural(targets.length, reviewNoun(r.kind))} ${r.kind === "ports" ? "stopped" : "closed"} or already gone.${failed.length ? "\n" + failed.map(record => `${record.target}: ${record.result}`).join("\n") : r.kind === "ports" ? " Results are in Actions." : " Recovery instructions are in Actions."}`;
    document.getElementById("review-submit").textContent = "View actions";
    document.getElementById("review-submit").disabled = false;
    document.getElementById("review-submit").onclick = () => { dialog.close(); navigate("actions"); };
    document.getElementById("review-cancel").textContent = "Done";
  } catch (e) {
    // A lost response can still mean the action ran. Do not offer an
    // automatic retry against stale targets; refresh and review again.
    status.textContent = `Could not complete the request: ${e}\nCheck Actions and refresh before trying again.`;
    document.getElementById("review-submit").textContent = "View actions";
    document.getElementById("review-submit").disabled = false;
    document.getElementById("review-submit").onclick = () => { dialog.close(); navigate("actions"); };
    document.getElementById("review-cancel").textContent = "Done";
  } finally {
    r.busy = false; document.getElementById("review-cancel").disabled = false; afterAction();
  }
}


function markGone(kind, ids, recs, ok) {
  const set = state.gone[kind];
  recs.forEach((r, i) => { if (ok(r) && ids[i] != null) set.add(ids[i]); });
}

function pruneGone(snap) {
  const tabs = new Set(snap.browsers.flatMap(b => b.tabs.map(t => t.id)));
  const pids = new Set(snap.sessions.map(x => x.pid));
  state.gone.tabs.forEach(id => { if (!tabs.has(id)) state.gone.tabs.delete(id); });
  state.gone.pids.forEach(p => { if (!pids.has(p)) state.gone.pids.delete(p); });
}


async function runUpdate(action) {
  if (state.updateBusy || ['checking', 'installing'].includes(state.update?.phase)) return;
  state.updateBusy = true;
  renderMain(true);
  try {
    if (action === 'install') {
      state.update.phase = 'installing';
      renderMain(true);
      await invoke('install_update');
    } else {
      state.update = await invoke('check_updates');
    }
  } catch (e) { toast(String(e)); }
  finally {
    state.updateBusy = false;
    try { state.update = await invoke('update_status'); } catch (_) {}
    renderMain(true);
  }
}


function wire(root) {

  const showActionDetails = key => {
    state.expandedAction = key;
    root.querySelectorAll("[data-action-details]").forEach(button => {
      const open = button.dataset.actionDetails === key;
      button.setAttribute("aria-expanded", String(open));
      button.closest("tr").classList.toggle("expanded", open);
      document.getElementById(button.getAttribute("aria-controls")).hidden = !open;
    });
  };
  root.querySelectorAll("[data-action-details]").forEach(button => {
    button.onclick = () => showActionDetails(state.expandedAction === button.dataset.actionDetails ? null : button.dataset.actionDetails);
    button.onkeydown = event => {
      if (event.key === "Escape") { showActionDetails(null); event.preventDefault(); }
    };
  });
  root.querySelectorAll("[data-action-detail-panel]").forEach(panel => panel.onkeydown = event => {
    if (event.key !== "Escape") return;
    const button = root.querySelector(`[aria-controls="${panel.id}"]`);
    showActionDetails(null);
    button?.focus();
    event.preventDefault();
  });

  root.querySelectorAll("[data-goto]").forEach(a => { a.href = "#" + encodeURIComponent(a.dataset.goto); a.onclick = e => { e.preventDefault(); navigate(a.dataset.goto); }; });
  root.querySelectorAll("[data-close-session]").forEach(b => b.onclick = () => reviewSessions(state.snap.sessions.filter(x => x.pid === +b.dataset.closeSession)));
  root.querySelectorAll("[data-close-stale]").forEach(b => b.onclick = () => { const h = holderByKey("g:" + b.dataset.closeStale); if (h) reviewSessions(filteredSessions(h.sessions).filter(x => x.state === "stale")); });
  root.querySelectorAll("[data-close-tab], [data-close-tabs]").forEach(button => button.onclick = () => {
    const b = state.snap.browsers.find(b => b.name === button.dataset.browser);
    if (!b) return;
    const ids = (button.dataset.closeTabs ?? button.dataset.closeTab).split(",").filter(Boolean).map(Number);
    closeTabs(b, b.tabs.filter(t => ids.includes(t.id)));
  });
  root.querySelectorAll("[data-stop]").forEach(b => b.onclick = () => twoStep(b, "p" + b.dataset.stop, async () => report(await invoke("stop_server", { pid: +b.dataset.stop, force: false }))));
  root.querySelectorAll("[data-quit]").forEach(b => b.onclick = () => twoStep(b, "q" + b.dataset.quit, async () => report(await invoke("quit_app", { name: b.dataset.quit, force: false }))));
  root.querySelectorAll("[data-restart]").forEach(b => b.onclick = () => twoStep(b, "r" + b.dataset.restart, async () => report(await invoke("restart_app", { name: b.dataset.restart, force: false }))));
  // The switches write config.toml; the daemon picks it up on its next tick.
  root.querySelectorAll("[data-auto]").forEach(cb => cb.onchange = () => saveAuto({ [cb.dataset.auto]: cb.checked }));
  root.querySelectorAll("[data-auto-domain-target]").forEach(cb => cb.onchange = () => saveAuto(autoMode(state.settings) === "off" && cb.checked ? { close_tabs: true, dry_run: true } : { close_tabs: cb.checked }));
  root.querySelectorAll("[data-auto-mode]").forEach(r => r.onchange = () => saveAuto(autoModeValues(state.settings, r.dataset.autoMode)));
  root.querySelectorAll("[data-auto-off]").forEach(b => b.onclick = () => saveAuto(autoModeValues(state.settings, "off")));
  root.querySelectorAll("[data-launch-window]").forEach(cb => cb.onchange = async () => { cb.disabled = true; try { state.settings = await invoke("set_open_window_at_launch", { value: cb.checked }); } catch (e) { toast(String(e)); } refresh(); });
  root.querySelectorAll("[data-setup]").forEach(b => b.onclick = () => window.dispatchEvent(new Event("autotrim-setup")));
  root.querySelectorAll("[data-hide]").forEach(b => b.onclick = () => invoke("hide_window").catch(e => toast(String(e))));
  root.querySelectorAll("[data-update]").forEach(b => b.onclick = () => runUpdate(b.dataset.update));
  root.querySelectorAll("[data-add-domain]").forEach(b => b.onclick = () => openDomainEditor());
  root.querySelectorAll("[data-edit-domain]").forEach(b => b.onclick = () => openDomainEditor({ index: +b.dataset.editDomain }));
  root.querySelectorAll("[data-remove-domain]").forEach(b => b.onclick = async () => { b.disabled = true; await removeDomainRule(+b.dataset.removeDomain); });
  root.querySelectorAll("[data-domain-hours]").forEach(select => select.onchange = () => saveTabRules(state.settings?.auto_tab_domains || [], +select.value));
  root.querySelectorAll("[data-review-domains]").forEach(b => b.onclick = () => openTabPreview());
  root.querySelectorAll("[data-auto-domain-site]").forEach(button => button.onclick = () => {
    const browser = state.snap.browsers.find(item => item.name === button.dataset.browser);
    if (!browser) return;
    const choices = siteHostChoices(browser, button.dataset.autoDomainSite, filteredTabs(browser));
    if (choices.length) openDomainEditor({ domain: choices[0].domain, hostChoices: choices });
  });
  root.querySelectorAll("[data-service]").forEach(b => {
    const what = b.dataset.service;
    const run = async () => { state.service = await invoke("service_" + what); toast(what === "install" ? "Installed the login service. The daemon runs now and at every login." : what === "restart" ? "The daemon was restarted." : "Removed the login service; the daemon has stopped. Its data is kept."); };
    if (what === "uninstall") b.onclick = () => twoStep(b, "svc", run);
    else b.onclick = () => { b.disabled = true; b.textContent = "…"; run().catch(e => toast(String(e))).finally(() => refresh()); };
  });

  root.querySelectorAll(".list-options").forEach(options => options.onkeydown = event => {
    if (event.key === "Escape") { options.open = false; options.querySelector("summary").focus(); }
  });
  root.querySelectorAll("[data-search-toggle]").forEach(button => button.onclick = () => {
    const key = button.dataset.searchToggle === 'tab' ? 'tabFilter' : 'sessFilter';
    const open = state.searchOpen.has(state.view) || !!state[key];
    if (open) { state.searchOpen.delete(state.view); state[key] = ''; }
    else state.searchOpen.add(state.view);
    button.focus({ preventScroll: true });
    renderMain(false, true);
    if (!open) document.querySelector('[data-tab-filter], [data-session-filter]')?.focus();
  });
  root.querySelectorAll("[data-tab-profile]").forEach(el => el.onchange = () => { state.tabProfile = el.value; renderMain(false, true); });
  root.querySelectorAll("[data-tab-sort]").forEach(el => el.onchange = () => { state.tabSort = el.value; state.tabReverse = false; renderMain(false, true); });
  root.querySelectorAll("[data-sess-sort]").forEach(el => el.onchange = () => { state.sessSort = el.value; state.sessReverse = false; renderMain(false, true); });
  root.querySelectorAll("[data-activity-filter]").forEach(el => el.onchange = () => {
    state[el.dataset.activityFilter === 'tab' ? 'tabState' : 'sessState'] = el.value;
    renderMain(false, true);
  });
  for (const [attribute, key] of [["data-tab-filter", "tabFilter"], ["data-session-filter", "sessFilter"]]) {
    root.querySelectorAll(`[${attribute}]`).forEach(input => {
      input.oninput = () => {
        state[key] = input.value;
        const start = input.selectionStart, end = input.selectionEnd, direction = input.selectionDirection;
        renderMain(false, true);
        const next = document.querySelector(`[${attribute}]`);
        // Keep edits in the middle of a query in place across a render.
        if (start != null && end != null) {
          try { next.setSelectionRange(start, end, direction); } catch (_) { /* Older web views may not support a search selection. */ }
        }
      };
      input.onkeydown = event => {
        if (event.key === 'Escape') { event.preventDefault(); state[key] = ''; state.searchOpen.delete(state.view); renderMain(false, true); document.querySelector('[data-search-toggle]')?.focus(); }
      };
    });
  }
  root.querySelectorAll("[data-list-inspect]").forEach(b => b.onclick = () => {
    b.focus({ preventScroll: true });
    const saved = listModels.get(b.dataset.list).saved;
    saved.inspected = saved.inspected === b.dataset.row ? null : b.dataset.row;
    renderMain(false, true);
  });
  root.querySelectorAll(".compact-table tbody tr.compact-row, .work-row").forEach(row => row.onclick = e => {
    // The whole highlighted row opens details; embedded controls act on their own.
    if (e.target.closest("button, input, a, select, textarea, label")) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed && row.contains(selection.anchorNode)) return;
    const button = row.querySelector("[data-list-inspect]");
    button.click();
  });
  function selectResource(model, row, checked, range) {
    if (!row?.eligible) return;
    const anchor = model.rows.findIndex(r => r.id === model.saved.anchor), end = model.rows.indexOf(row);
    const targets = range && anchor >= 0 ? model.rows.slice(Math.min(anchor, end), Math.max(anchor, end) + 1) : [row];
    for (const target of targets.filter(r => r.eligible)) checked ? model.saved.selected.add(target.id) : model.saved.selected.delete(target.id);
    model.saved.anchor = row.id;
    renderMain(false, true);
  }
  root.querySelectorAll("[data-list-select]").forEach(cb => {
    cb.onclick = event => { cb._range = event.shiftKey; };
    cb.onchange = () => {
      cb.focus({ preventScroll: true });
      const model = listModels.get(cb.dataset.list), row = model.rows.find(r => r.id === cb.dataset.row);
      selectResource(model, row, cb.checked, cb._range);
    };
  });
  root.querySelectorAll("[data-list-group]").forEach(button => button.onclick = () => {
    button.focus({ preventScroll: true });
    const model = listModels.get(button.dataset.list), group = button.dataset.listGroup;
    const collapsed = model.saved.activeCollapsed || model.saved.collapsed;
    collapsed.has(group) ? collapsed.delete(group) : collapsed.add(group);
    renderMain(false, true);
  });
  root.querySelectorAll("[data-list-all]").forEach(cb => {
    const model = listModels.get(cb.dataset.listAll), count = model.saved.selected.size, total = model.rows.filter(r => r.eligible).length;
    cb.indeterminate = count > 0 && count < total;
    cb.onchange = () => { cb.focus({ preventScroll: true }); model.saved.selected = new Set(cb.checked ? model.rows.filter(r => r.eligible).map(r => r.id) : []); renderMain(false, true); };
  });
  for (const [attribute, action] of [["data-list-clear", m => m.saved.selected.clear()], ["data-list-eligible", m => { m.saved.selected = new Set(m.rows.filter(r => r.eligible).map(r => r.id)); }], ["data-list-review", m => m.options.review(m.rows.filter(r => r.eligible && m.saved.selected.has(r.id)))], ["data-list-stale", m => m.options.review(m.rows.filter(r => r.eligible && r.status === "stale"))]]) {
    root.querySelectorAll(`[${attribute}]`).forEach(b => {
      const initial = listModels.get(b.getAttribute(attribute));
      if (initial && ['data-list-eligible', 'data-list-stale'].includes(attribute)) b.disabled = !initial.rows.some(r => r.eligible && (attribute !== 'data-list-stale' || r.status === 'stale'));
      b.onclick = () => { const model = listModels.get(b.getAttribute(attribute)); if (!model) return; action(model); const options = b.closest(".list-options"); if (options) { options.open = false; options.querySelector("summary").focus({ preventScroll: true }); } if (!state.actionReview) renderMain(false, true); };
    });
  }
  root.querySelectorAll('[data-close-tab], [data-close-tabs], [data-list-review="tabs"], [data-list-stale="tabs"]').forEach(button => {
    if (!state.tabsClosing) return;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    button.textContent = "Closing…";
  });
  root.querySelectorAll("[data-review-port]").forEach(b => b.onclick = () => reviewPorts(state.snap.ports.filter(p => p.pid === +b.dataset.reviewPort)));
  root.querySelectorAll("[data-copy-command]").forEach(b => b.onclick = () => copyCommand(b));

}


return { toast, copyText, copyCommand, twoStep, afterAction, report, refresh, navigate, saveAuto, saveTabRules, removeDomainRule, requestTabPreview, openDomainEditor, openTabPreview, reviewSessions, closeTabs, reviewPorts, reviewNoun, reviewVerb, openReview, updateReviewTotal, executeReview, markGone, pruneGone, runUpdate, wire };
}
