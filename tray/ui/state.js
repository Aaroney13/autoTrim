// Explicit application state and pure selectors.
import { plural } from "./format.js";
const state = { actionReview: null, tabsClosing: false, domainEditor: null, tabPreview: { requestId: 0, busy: false, rows: [], error: "" }, settingsRevision: 0, snap: null, src: null, log: [], settings: null, service: null, view: window.location?.hash === "#settings" ? "settings" : "overview", showAll: false, tabSort: "idle", tabReverse: false, tabFilter: "", tabProfile: "", sessSort: "rss", sessReverse: false, sessFilter: "", sessState: "all", tabState: "all", listState: new Map(), searchOpen: new Set(), settingsBusy: false, update: null, updateBusy: false,
  // Things closed from here that the daemon's snapshot has not caught up with yet.
  gone: { tabs: new Set(), pids: new Set() } };

const armed = {};
state.expandedAction = null;

const ACTIVITY_WINDOW = 600;
state.activityHistory = [];

// Keep only observed samples; polling the same snapshot must not invent history.
function recordActivity(snap) {
  if (!Number.isFinite(snap?.taken_at) || !snap.system) return;
  const history = state.activityHistory, last = history.at(-1);
  if (last && snap.taken_at < last.taken_at) return;
  const sample = { taken_at: snap.taken_at, used_mem: snap.system.used_mem,
    cpu_pct: snap.system.cpu_pct, used_swap: snap.system.used_swap };
  if (last?.taken_at === sample.taken_at) history[history.length - 1] = sample;
  else history.push(sample);
  state.activityHistory = history.filter(point => point.taken_at >= sample.taken_at - ACTIVITY_WINDOW).slice(-601);
}

// Summarize the bounded action history, never cumulative physical RAM savings.
function cleanupImpact(log = state.log) {
  const unique = new Map();
  log.forEach((record, index) => unique.set(record.id || index, record));
  const records = [...unique.values()];
  const impact = { recorded: records.length, closed: 0, automatic: 0, footprint: 0, measured: 0, estimated: false, latest: null };
  const closing = new Set(["close_session", "close_tab", "stop_server", "quit_app"]);
  for (const record of records) {
    if (record.mode === "dry-run" || ["dry_run", "intent", "skipped"].includes(record.status)) continue;
    const observation = record.memory_observation;
    const validSample = sample => sample && [sample.taken_at_ms, sample.used_mem, sample.used_swap].every(value => Number.isFinite(value) && value >= 0);
    if (validSample(observation?.before) && validSample(observation?.after)
      && observation.after.taken_at_ms >= observation.before.taken_at_ms
      && (!impact.latest || observation.after.taken_at_ms > impact.latest.after.taken_at_ms)) {
      impact.latest = observation;
    }
    // Already-absent targets and restarted apps did not leave a closed workload.
    const legacy = !record.status || record.status === "legacy";
    const success = record.status === "success" || legacy && /^(terminated|closed|stopped|quit)\b/i.test(record.result);
    if (!closing.has(record.action) || !success || /already gone|not open any more|fail|error|refus|still running|could not/i.test(record.result)) continue;
    const processes = record.termination?.processes;
    if (processes && !processes.some(process => ["graceful", "forced"].includes(process.exit))) continue;
    impact.closed++;
    if (record.mode === "auto") impact.automatic++;
    if (Number.isFinite(record.rss) && record.rss > 0) {
      impact.footprint += record.rss;
      impact.measured++;
      if (record.action === "close_tab") impact.estimated = true;
    }
  }
  return impact;
}

state.appIcons = new Map();
state.appIconsPending = new Set();

function appIconName(holder) {
  if (holder.kind === "other") return "";
  return holder.app || (holder.kind === "agent" ? holder.name.replace(/ sessions$/, "") : holder.name);
}

const listModels = new Map();



const goneTab = t => state.gone.tabs.has(t.id);

const goneSession = x => state.gone.pids.has(x.pid);


function holders(s) {
  return s.groups.map(g => {
    const browser = s.browsers.find(b => b.name === g.name);
    const sessions = g.kind === "agent" ? s.sessions.filter(x => g.pids.includes(x.pid)) : [];
    const kind = g.kind === "agent" ? "agent" : browser ? "browser" : g.kind === "app" ? "app" : "other";
    let count;
    if (kind === "agent") { const live = sessions.filter(x => !goneSession(x)), stale = live.filter(x => x.state === "stale").length; count = sessionCount(live) + (stale ? ` · ${stale} stale` : ""); }
    else if (browser) { const live = browser.tabs.filter(t => !goneTab(t)), stale = live.filter(t => isStale(browser, t)).length; count = live.length ? plural(live.length, "tab") + (stale ? ` · ${stale} stale` : "") : plural(g.procs, "proc"); }
    else count = plural(g.procs, "proc");
    return { key: "g:" + g.name, name: g.name, kind, rss: g.rss, cpu: g.cpu, procs: g.procs, pids: g.pids, app: g.app || null, count, group: g, browser, sessions };
  });
}

const holderByKey = key => holders(state.snap).find(h => h.key === key);



const POLL_MS = 5000; // how often this window reads the daemon's snapshot

const sync = { lastPoll: 0, scans: 0, requestId: 0, acceptedRequest: 0, acceptedFresh: false };

const autoMode = c => !c || !(c.auto_close_sessions || c.auto_stop_servers || c.auto_close_tabs) ? "off" : c.auto_dry_run ? "preview" : "on";

const autoModeWord = () => autoMode(state.settings) === "off" ? "" : autoMode(state.settings) === "preview" ? "preview only" : "on";

// What auto mode has warned about and is waiting on, with the time left.

const sessionKey = x => `${x.pid}:${x.start_time}`;

const tabKey = (b, t) => JSON.stringify([b.name, t.id, t.profile, t.url]);

const sessionName = x => x.session_name || x.project || "Unnamed session";

function sessionCount(list) {
  const engines = list.filter(x => x.engine).length, standalone = list.length - engines;
  const tasks = list.flatMap(x => x.threads || []).filter(t => !t.helper).length;
  return [engines ? plural(engines, "backend") : "", standalone ? plural(standalone, "session") : "", tasks ? plural(tasks, "loaded task") : ""].filter(Boolean).join(" · ") || "0 sessions";
}

const taskSearch = t => [t.name, t.first_prompt, t.cwd, t.id].join(" ").toLowerCase();

const sessionProtection = x => goneSession(x) ? "Closed" : x.is_self ? "This session" : x.engine ? "App engine" : x.state === "active" ? "In use" : "";

function reconcileList(key, rows) {
  let saved = state.listState.get(key);
  if (!saved) { saved = { inspected: null, selected: new Set() }; state.listState.set(key, saved); }
  const eligible = new Set(rows.filter(r => r.eligible).map(r => r.id));
  saved.selected = new Set([...saved.selected].filter(id => eligible.has(id)));
  if (!rows.some(r => r.id === saved.inspected)) saved.inspected = null;
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

function hostnameOfTab(tab) {
  try {
    const source = String(tab.url || "");
    const authority = source.match(/^https?:\/\/([^/?#]*)/i)?.[1];
    if (!authority || authority.includes("%")) return null;
    const url = new URL(source);
    return url.protocol === "http:" || url.protocol === "https:" ? url.hostname.toLowerCase().replace(/\.$/, "") : null;
  } catch (_) { return null; }
}

function siteHostChoices(browser, site, tabs = browser.tabs) {
  const counts = new Map();
  for (const tab of tabs) {
    if (tab.site !== site) continue;
    const domain = hostnameOfTab(tab);
    if (domain) counts.set(domain, (counts.get(domain) || 0) + 1);
  }
  return [...counts].map(([domain, count]) => ({ domain, count })).sort((a, b) => b.count - a.count || a.domain.localeCompare(b.domain));
}

// Snapshot-based suggestions; the native preview rechecks matching tabs.
function domainRecommendations() {
  const byDomain = new Map(), rules = state.settings?.auto_tab_domains || [];
  const inactiveSecs = Math.round((state.settings?.auto_tab_inactive_hours ?? 24) * 3600);
  for (const browser of state.snap?.browsers || []) {
    if (!['Google Chrome', 'Chrome'].includes(browser.name) || !browser.can_close_tabs || browser.tabs_note) continue;
    for (const tab of browser.tabs) {
      if (goneTab(tab)) continue;
      const domain = hostnameOfTab(tab);
      // Only suggest DNS names accepted by the rule editor, never IPs or local labels.
      if (!domain || domain.length > 253 || !domain.includes('.') || /^\d+(\.\d+){3}$/.test(domain)
        || !domain.split('.').every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))) continue;
      if (rules.some(rule => {
        const listed = rule.domain.toLowerCase().replace(/\.$/, '');
        return domain === listed || rule.include_subdomains && domain.endsWith('.' + listed);
      })) continue;
      const row = byDomain.get(domain) || { domain, count: 0, inactiveCount: 0, oldestIdleSecs: null };
      row.count++;
      if (!tab.pinned && !tab.active && tab.last_active > 0 && tab.last_active <= state.snap.taken_at) {
        const idle = state.snap.taken_at - tab.last_active;
        row.oldestIdleSecs = Math.max(row.oldestIdleSecs ?? 0, idle);
        if (idle >= inactiveSecs) row.inactiveCount++;
      }
      byDomain.set(domain, row);
    }
  }
  return [...byDomain.values()].sort((a, b) => b.inactiveCount - a.inactiveCount
    || (b.oldestIdleSecs ?? -1) - (a.oldestIdleSecs ?? -1)
    || b.count - a.count || a.domain.localeCompare(b.domain));
}

// Remember what was closed from here until a snapshot no longer lists it.

const canCloseSession = x => !goneSession(x) && !x.is_self && !x.engine && x.state !== "active";

const canCloseTab = (b, t) => b.can_close_tabs && !goneTab(t) && !t.pinned && !t.active;

function filteredSessions(list) {
  const q = state.sessFilter.trim().toLowerCase();
  const result = list.filter(x => !goneSession(x) && (state.sessState === "all" || x.state === state.sessState) && (!q || [x.session_name, x.project, x.first_prompt, x.host].join(" ").toLowerCase().includes(q) || (x.threads || []).some(t => taskSearch(t).includes(q))))
    .sort((a, b) => state.sessSort === "idle" ? (b.idle_secs ?? b.quiet_for_secs ?? -1) - (a.idle_secs ?? a.quiet_for_secs ?? -1) : state.sessSort === "age" ? b.age_secs - a.age_secs : state.sessSort === "name" ? String(a.session_name ?? a.project).localeCompare(String(b.session_name ?? b.project)) : b.rss - a.rss);
  return state.sessReverse ? result.reverse() : result;
}

function filteredTabs(b, filter = state.tabState) {
  const q = state.tabFilter.trim().toLowerCase(), idle = t => t.active ? -1 : t.idle_secs ?? -.5;
  const result = b.tabs.filter(t => !goneTab(t) && (!state.tabProfile || t.profile === state.tabProfile) && (filter === "all" || filter === "stale" && isStale(b, t) && !t.pinned || filter === "chat" && t.kind === "chat") && (!q || [t.title, t.url, t.site].join(" ").toLowerCase().includes(q)))
    .sort((a, c) => state.tabSort === "site" ? a.site.localeCompare(c.site) || idle(c) - idle(a) : state.tabSort === "title" ? a.title.localeCompare(c.title) : state.tabSort === "window" ? a.window_id - c.window_id || a.index - c.index : idle(c) - idle(a));
  return state.tabReverse ? result.reverse() : result;
}

function autoModeValues(c, mode) {
  if (mode === "off") return { close_sessions: false, stop_servers: false, close_tabs: false };
  const anyTarget = c.auto_close_sessions || c.auto_stop_servers || c.auto_close_tabs;
  return { close_sessions: c.auto_close_sessions || !anyTarget, stop_servers: c.auto_stop_servers, close_tabs: c.auto_close_tabs, dry_run: mode === "preview" };
}

function actionSucceeded(record) {
  if (record.mode === "dry-run") return false;
  if (record.status && record.status !== "legacy") return record.status === "success";
  return /^(terminated|killed|stopped|closed|already gone|not open any more)/.test(record.result) && !/partial|failed|still running/i.test(record.result);
}

function domainInactivityLabel(hours) {
  return hours === 168 ? "1 week" : hours < 1
    ? plural(Math.round(hours * 60), "minute") : plural(hours, "hour");
}

export { ACTIVITY_WINDOW, recordActivity, cleanupImpact, domainInactivityLabel, domainRecommendations, state, armed, listModels, goneTab, goneSession, holders, holderByKey, appIconName, POLL_MS, sync, autoMode, autoModeWord, sessionKey, tabKey, sessionName, sessionCount, taskSearch, sessionProtection, reconcileList, isStale, sitesOf, hostnameOfTab, siteHostChoices, canCloseSession, canCloseTab, filteredSessions, filteredTabs, autoModeValues, actionSucceeded };
