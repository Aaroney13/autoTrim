// IPC, confirmation, polling, and event handlers. Renderer callbacks are injected.
import { icon, GB, bytes, dur, pct, esc, plural, kindTag, AGENT_LABEL, epochNow, nowSecs, compactDuration, signedBytes, MB } from "./format.js";
import { state, armed, listModels, goneTab, goneSession, holders, holderByKey, POLL_MS, sync, autoMode, autoModeWord, sessionKey, tabKey, sessionName, sessionProtection, reconcileList, isStale, sitesOf, canCloseSession, canCloseTab, filteredSessions, filteredTabs, autoModeValues, actionSucceeded } from "./state.js";

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
  pruneGone(snap);
  renderAll(first);
}


function navigate(view) {
  if (state.view !== view) { state.sessFilter = ""; state.sessState = "all"; state.tabFilter = ""; state.tabProfile = ""; state.tabState = "all"; state.listState.clear(); }
  state.view = view; renderSide(); renderMain(true);
}


async function saveAuto(values) {
  if (state.settingsBusy) return;
  state.settingsBusy = true; ++state.settingsRevision; renderMain(true);
  try { state.settings = await invoke("set_auto", values); }
  catch (e) { toast(String(e)); }
  finally { state.settingsBusy = false; ++state.settingsRevision; renderSide(); renderMain(true); refresh(); }
}

// The dialog owns a frozen set of reviewed identities. Polling may update
// the app behind it, but never changes the user's targets or checkboxes.

function reviewSessions(list) {
  const targets = list.filter(canCloseSession).map(x => ({ id: x.pid, name: x.session_name || x.project || `Session ${x.pid}`, detail: [x.project, x.host, x.idle_secs != null ? `idle ${dur(x.idle_secs)}` : ""].filter(Boolean).join(" · "), rss: x.rss, startTime: x.start_time }));
  openReview("sessions", targets);
}

function reviewTabs(b, list) {
  const targets = list.filter(t => canCloseTab(b, t)).map(t => ({ id: t.id, name: t.title.trim() || t.url, detail: `${t.site} · ${b.open_profiles.find(p => p.dir === t.profile)?.label || t.profile}`, rss: b.per_tab_estimate || 0, url: t.url, profile: t.profile }));
  openReview("tabs", targets, b.name);
}

function reviewPorts(list) {
  const processes = new Map();
  list.filter(p => !p.owner_managed).forEach(p => {
    if (!processes.has(p.pid)) processes.set(p.pid, { id: p.pid, name: p.label || p.process, detail: `${p.owner} · ${p.process} (${p.pid}) · ports ${state.snap.ports.filter(port => port.pid === p.pid).map(port => port.port).join(", ")}`, rss: 0, startTime: p.start_time });
  });
  openReview("ports", [...processes.values()]);
}

const reviewNoun = kind => kind === "tabs" ? "tab" : kind === "ports" ? "server" : "session";

const reviewVerb = kind => kind === "ports" ? "Stop" : "Close";

function openReview(kind, targets, browser = null) {
  if (state.actionReview || !targets.length) return;
  state.actionReview = { kind, targets, browser, selected: new Set(targets.map(t => t.id)), busy: false, returnView: state.view };
  const dialog = document.getElementById("action-review"), noun = reviewNoun(kind), verb = reviewVerb(kind);
  dialog.innerHTML = `<div class="review-content"><h2 id="review-title">${verb} these ${noun}s?</h2><p id="review-description">Review the exact targets below. Uncheck anything you want to keep.</p><div class="review-targets">${targets.map(t => `<label class="review-target"><input type="checkbox" data-review-target="${t.id}" checked><span><span class="l1">${esc(t.name)}</span><span class="l2" style="display:block">${esc(t.detail)}</span></span><span class="num">${t.rss ? `${kind === "tabs" ? "≈ " : ""}${bytes(t.rss)}` : ""}</span></label>`).join("")}</div><div class="review-total"><span id="review-count"></span><span id="review-memory"></span></div><div class="recovery-note">${icon("resume")}<span>${kind === "ports" ? "Stopping a server ends its process and closes all ports it owns. Restart it from the terminal or app that launched it." : kind === "tabs" ? "Use ⌘⇧T in the same browser profile to reopen a recently closed tab. Its URL is saved in Actions. Unsaved drafts and temporary chats may not be restored." : "Closing stops the session’s processes. Its transcript stays on disk. Available resume commands are saved in Actions."}</span></div><p>${kind === "ports" ? "Each process is checked again before stopping. Services managed by an app or the system are skipped." : kind === "tabs" ? "Pinned, active, or navigated tabs are skipped when checked before closing. Memory figures are estimates." : "Active sessions, app engines, and sessions whose process has changed are skipped when checked before closing."}</p><p id="review-status" role="status"></p><div class="review-actions"><button id="review-cancel" autofocus>Cancel</button><button class="primary" id="review-submit">${verb} ${plural(targets.length, noun)}</button></div></div>`;
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
  document.getElementById("review-memory").innerHTML = rss ? `<b>${r.kind === "tabs" ? "≈ " : ""}${bytes(rss)}</b> ${r.kind === "tabs" ? "estimated" : "held now"}` : "";
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
    if (r.kind === "sessions" || r.kind === "ports") {
      for (const target of targets) {
        try {
          const record = r.kind === "ports" ? await invoke("stop_server", { pid: target.id, force: false, expectedStartTime: target.startTime }) : await invoke("close_session", { pid: target.id, expectedStartTime: target.startTime });
          if (r.kind === "sessions" && actionSucceeded(record)) state.gone.pids.add(target.id);
          results.push(record);
        } catch (e) { results.push({ target: target.name, result: `Skipped: ${e}` }); }
      }
    } else {
      const records = await invoke("close_tabs", { browser: r.browser, expectedTabs: targets.map(t => ({ id: t.id, url: t.url, profile: t.profile })) });
      records.forEach((record, i) => { if (/^(closed|not open any more|already gone)/.test(record.result)) state.gone.tabs.add(targets[i].id); });
      results.push(...records);
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

  root.querySelectorAll("[data-goto]").forEach(a => { a.href = "#" + encodeURIComponent(a.dataset.goto); a.onclick = e => { e.preventDefault(); navigate(a.dataset.goto); }; });
  root.querySelectorAll("[data-close-session]").forEach(b => b.onclick = () => reviewSessions(state.snap.sessions.filter(x => x.pid === +b.dataset.closeSession)));
  root.querySelectorAll("[data-close-stale]").forEach(b => b.onclick = () => { const h = holderByKey("g:" + b.dataset.closeStale); if (h) reviewSessions(filteredSessions(h.sessions).filter(x => x.state === "stale")); });
  root.querySelectorAll("[data-close-tab], [data-close-tabs]").forEach(button => button.onclick = () => {
    const b = state.snap.browsers.find(b => b.name === button.dataset.browser);
    if (!b) return;
    const ids = (button.dataset.closeTabs ?? button.dataset.closeTab).split(",").filter(Boolean).map(Number);
    reviewTabs(b, b.tabs.filter(t => ids.includes(t.id)));
  });
  root.querySelectorAll("[data-stop]").forEach(b => b.onclick = () => twoStep(b, "p" + b.dataset.stop, async () => report(await invoke("stop_server", { pid: +b.dataset.stop, force: false }))));
  root.querySelectorAll("[data-quit]").forEach(b => b.onclick = () => twoStep(b, "q" + b.dataset.quit, async () => report(await invoke("quit_app", { name: b.dataset.quit, force: false }))));
  root.querySelectorAll("[data-restart]").forEach(b => b.onclick = () => twoStep(b, "r" + b.dataset.restart, async () => report(await invoke("restart_app", { name: b.dataset.restart, force: false }))));
  // The switches write config.toml; the daemon picks it up on its next tick.
  root.querySelectorAll("[data-auto]").forEach(cb => cb.onchange = () => saveAuto({ [cb.dataset.auto]: cb.checked }));
  root.querySelectorAll("[data-auto-mode]").forEach(r => r.onchange = () => saveAuto(autoModeValues(state.settings, r.dataset.autoMode)));
  root.querySelectorAll("[data-auto-off]").forEach(b => b.onclick = () => saveAuto(autoModeValues(state.settings, "off")));
  root.querySelectorAll("[data-launch-window]").forEach(cb => cb.onchange = async () => { cb.disabled = true; try { state.settings = await invoke("set_open_window_at_launch", { value: cb.checked }); } catch (e) { toast(String(e)); } refresh(); });
  root.querySelectorAll("[data-hide]").forEach(b => b.onclick = () => invoke("hide_window").catch(e => toast(String(e))));
  root.querySelectorAll("[data-update]").forEach(b => b.onclick = () => runUpdate(b.dataset.update));
  root.querySelectorAll("[data-service]").forEach(b => {
    const what = b.dataset.service;
    const run = async () => { state.service = await invoke("service_" + what); toast(what === "install" ? "Installed the login service. The daemon runs now and at every login." : what === "restart" ? "The daemon was restarted." : "Removed the login service; the daemon has stopped. Its data is kept."); };
    if (what === "uninstall") b.onclick = () => twoStep(b, "svc", run);
    else b.onclick = () => { b.disabled = true; b.textContent = "…"; run().catch(e => toast(String(e))).finally(() => refresh()); };
  });

  root.querySelectorAll("[data-tab-profile]").forEach(el => el.onchange = () => { state.tabProfile = el.value; renderMain(true); });
  root.querySelectorAll("[data-tab-sort]").forEach(el => el.onchange = () => { state.tabSort = el.value; renderMain(true); });
  root.querySelectorAll("[data-sess-sort]").forEach(el => el.onchange = () => { state.sessSort = el.value; renderMain(true); });
  for (const [attribute, key] of [["data-tab-filter", "tabFilter"], ["data-session-filter", "sessFilter"]]) {
    root.querySelectorAll(`[${attribute}]`).forEach(input => input.oninput = () => {
      state[key] = input.value; const pos = input.selectionStart; renderMain(true);
      const next = document.querySelector(`[${attribute}]`); next.focus(); if (pos != null) next.setSelectionRange(pos, pos);
    });
  }
  root.querySelectorAll("[data-tab-state]").forEach(b => b.onclick = () => { state.tabState = b.dataset.tabState; renderMain(true); });
  root.querySelectorAll("[data-session-state]").forEach(b => b.onclick = () => { state.sessState = b.dataset.sessionState; renderMain(true); });
  root.querySelectorAll("[data-list-inspect]").forEach(b => b.onclick = () => {
    listModels.get(b.dataset.list).saved.inspected = b.dataset.row; renderMain(false, true);
  });
  root.querySelectorAll("[data-list-select]").forEach(cb => cb.onchange = () => {
    const model = listModels.get(cb.dataset.list), row = model.rows.find(r => r.id === cb.dataset.row);
    if (cb.checked && row?.eligible) model.saved.selected.add(row.id); else model.saved.selected.delete(cb.dataset.row);
    renderMain(false, true);
  });
  root.querySelectorAll("[data-list-all]").forEach(cb => {
    const model = listModels.get(cb.dataset.listAll), count = model.saved.selected.size, total = model.rows.filter(r => r.eligible).length;
    cb.indeterminate = count > 0 && count < total;
    cb.onchange = () => { model.saved.selected = new Set(cb.checked ? model.rows.filter(r => r.eligible).map(r => r.id) : []); renderMain(false, true); };
  });
  for (const [attribute, action] of [["data-list-clear", m => m.saved.selected.clear()], ["data-list-eligible", m => { m.saved.selected = new Set(m.rows.filter(r => r.eligible).map(r => r.id)); }], ["data-list-review", m => m.options.review(m.rows.filter(r => r.eligible && m.saved.selected.has(r.id)))], ["data-list-stale", m => m.options.review(m.rows.filter(r => r.eligible && r.status === "stale"))]]) {
    root.querySelectorAll(`[${attribute}]`).forEach(b => b.onclick = () => { const model = listModels.get(b.getAttribute(attribute)); action(model); if (!state.actionReview) renderMain(false, true); });
  }
  root.querySelectorAll("[data-review-port]").forEach(b => b.onclick = () => reviewPorts(state.snap.ports.filter(p => p.pid === +b.dataset.reviewPort)));
  root.querySelectorAll("[data-copy-command]").forEach(b => b.onclick = () => copyCommand(b));

}


return { toast, copyText, copyCommand, twoStep, afterAction, report, refresh, navigate, saveAuto, reviewSessions, reviewTabs, reviewPorts, reviewNoun, reviewVerb, openReview, updateReviewTotal, executeReview, markGone, pruneGone, runUpdate, wire };
}
