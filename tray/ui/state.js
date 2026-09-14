// Explicit application state and pure selectors.
import { plural } from "./format.js";
const state = { actionReview: null, settingsRevision: 0, snap: null, src: null, log: [], settings: null, service: null, view: window.location?.hash === "#settings" ? "settings" : "overview", showAll: false, tabSort: "idle", tabFilter: "", tabProfile: "", sessSort: "rss", sessFilter: "", sessState: "all", tabState: "all", listState: new Map(), settingsBusy: false, update: null, updateBusy: false,
  // Things closed from here that the daemon's snapshot has not caught up with yet.
  gone: { tabs: new Set(), pids: new Set() } };

const armed = {};

const listModels = new Map();



const goneTab = t => state.gone.tabs.has(t.id);

const goneSession = x => state.gone.pids.has(x.pid);


function holders(s) {
  return s.groups.map(g => {
    const browser = s.browsers.find(b => b.name === g.name);
    const sessions = g.kind === "agent" ? s.sessions.filter(x => g.pids.includes(x.pid)) : [];
    const kind = g.kind === "agent" ? "agent" : browser ? "browser" : g.kind === "app" ? "app" : "other";
    let count;
    if (kind === "agent") { const live = sessions.filter(x => !goneSession(x)), stale = live.filter(x => x.state === "stale").length; count = plural(live.length, "session") + (stale ? ` · ${stale} stale` : ""); }
    else if (browser) { const live = browser.tabs.filter(t => !goneTab(t)), stale = live.filter(t => isStale(browser, t)).length; count = live.length ? plural(live.length, "tab") + (stale ? ` · ${stale} stale` : "") : plural(g.procs, "proc"); }
    else count = plural(g.procs, "proc");
    return { key: "g:" + g.name, name: g.name, kind, rss: g.rss, cpu: g.cpu, procs: g.procs, pids: g.pids, app: g.app || null, count, group: g, browser, sessions };
  });
}

const holderByKey = key => holders(state.snap).find(h => h.key === key);



const POLL_MS = 5000; // how often this window reads the daemon's snapshot

const sync = { lastPoll: 0, scans: 0, requestId: 0, acceptedRequest: 0, acceptedFresh: false };

const autoMode = c => !c || !(c.auto_close_sessions || c.auto_stop_servers) ? "off" : c.auto_dry_run ? "preview" : "on";

const autoModeWord = () => autoMode(state.settings) === "off" ? "" : autoMode(state.settings) === "preview" ? "preview only" : "on";

// What auto mode has warned about and is waiting on, with the time left.

const sessionKey = x => `${x.pid}:${x.start_time}`;

const tabKey = (b, t) => JSON.stringify([b.name, t.id, t.profile, t.url]);

const sessionName = x => x.session_name || x.project || "Unnamed session";

const sessionProtection = x => goneSession(x) ? "Closed" : x.is_self ? "This session" : x.engine ? "App engine" : x.state === "active" ? "In use" : "";

function reconcileList(key, rows) {
  let saved = state.listState.get(key);
  if (!saved) { saved = { inspected: null, selected: new Set() }; state.listState.set(key, saved); }
  const eligible = new Set(rows.filter(r => r.eligible).map(r => r.id));
  saved.selected = new Set([...saved.selected].filter(id => eligible.has(id)));
  if (!rows.some(r => r.id === saved.inspected)) saved.inspected = rows[0]?.id ?? null;
  return saved;
}

function isStale(b, t) { return !t.active && !goneTab(t) && t.idle_secs != null && t.idle_secs >= (state.settings?.tab_stale_after_secs ?? 86400); }

// Tabs rolled up by site, worst first, from the tabs given (the live ones),
// so the table follows a close at once instead of waiting for a snapshot.

function sitesOf(b, tabs) {
  const by = new Map();
  for (const t of tabs) {
    const s = by.get(t.site) || { site: t.site, kind: t.kind, tabs: 0, stale_tabs: 0, oldest_idle_secs: null, est_rss: 0 };
    s.tabs++; if (isStale(b, t)) s.stale_tabs++;
    if (!t.active && t.idle_secs != null && (s.oldest_idle_secs == null || t.idle_secs > s.oldest_idle_secs)) s.oldest_idle_secs = t.idle_secs;
    s.est_rss = s.tabs * (b.per_tab_estimate || 0);
    by.set(t.site, s);
  }
  return [...by.values()].sort((a, c) => c.stale_tabs - a.stale_tabs || c.tabs - a.tabs || (c.oldest_idle_secs ?? -1) - (a.oldest_idle_secs ?? -1) || a.site.localeCompare(c.site));
}

// Remember what was closed from here until a snapshot no longer lists it.

const canCloseSession = x => !goneSession(x) && !x.is_self && !x.engine && x.state !== "active";

const canCloseTab = (b, t) => b.can_close_tabs && !goneTab(t) && !t.pinned && !t.active;

function filteredSessions(list) {
  const q = state.sessFilter.trim().toLowerCase();
  return list.filter(x => !goneSession(x) && (state.sessState === "all" || x.state === state.sessState) && (!q || [x.session_name, x.project, x.first_prompt, x.host].join(" ").toLowerCase().includes(q)))
    .sort((a, b) => state.sessSort === "idle" ? (b.idle_secs ?? b.quiet_for_secs ?? -1) - (a.idle_secs ?? a.quiet_for_secs ?? -1) : state.sessSort === "age" ? b.age_secs - a.age_secs : state.sessSort === "name" ? String(a.session_name ?? a.project).localeCompare(String(b.session_name ?? b.project)) : b.rss - a.rss);
}

function filteredTabs(b) {
  const q = state.tabFilter.trim().toLowerCase(), idle = t => t.active ? -1 : t.idle_secs ?? -.5;
  return b.tabs.filter(t => !goneTab(t) && (!state.tabProfile || t.profile === state.tabProfile) && (state.tabState === "all" || state.tabState === "stale" && isStale(b, t) && !t.pinned || state.tabState === "chat" && t.kind === "chat") && (!q || [t.title, t.url, t.site].join(" ").toLowerCase().includes(q)))
    .sort((a, c) => state.tabSort === "site" ? a.site.localeCompare(c.site) || idle(c) - idle(a) : state.tabSort === "title" ? a.title.localeCompare(c.title) : state.tabSort === "window" ? a.window_id - c.window_id || a.index - c.index : idle(c) - idle(a));
}

function autoModeValues(c, mode) {
  if (mode === "off") return { close_sessions: false, stop_servers: false };
  return { close_sessions: c.auto_close_sessions || !c.auto_stop_servers, stop_servers: c.auto_stop_servers, dry_run: mode === "preview" };
}

function actionSucceeded(record) {
  if (record.mode === "dry-run") return false;
  if (record.status && record.status !== "legacy") return record.status === "success";
  return /^(terminated|killed|stopped|closed|already gone|not open any more)/.test(record.result) && !/partial|failed|still running/i.test(record.result);
}

export { state, armed, listModels, goneTab, goneSession, holders, holderByKey, POLL_MS, sync, autoMode, autoModeWord, sessionKey, tabKey, sessionName, sessionProtection, reconcileList, isStale, sitesOf, canCloseSession, canCloseTab, filteredSessions, filteredTabs, autoModeValues, actionSucceeded };
