// Run with: node --test tests/tray-ui.test.mjs
// Exercise production UI logic with a stubbed bridge; never call the daemon.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const html = readFileSync(new URL('../tray/ui/index.html', import.meta.url), 'utf8');
const readModule = name => readFileSync(new URL(`../tray/ui/${name}.js`, import.meta.url), 'utf8');
const moduleSource = source => source
  // Git checkouts on Windows may use CRLF. Normalize before matching
  // module declarations and the injected action factory's closing brace.
  .replace(/\r\n?/g, '\n')
  .replace(/^import .*;\n/gm, '').replace(/^export \{[^}]*\};?\n/gm, '')
  .replace(/^export function createActions[^\n]*\n/m, '')
  .replace(/^return \{[^\n]*\n\}\n?$/m, '')
  .replace(/^const \{[^\n]* = createActions[^\n]*\n/m, '');
const buildScript = (readSource = readModule) => ['format', 'state', 'actions', 'render']
  .map(name => moduleSource(readSource(name))).join('\n');
const script = buildScript();

function load(invoke = async () => {}, source = script) {
  const elements = new Map();
  const document = {
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, {
        innerHTML: '', textContent: '', disabled: false, style: {},
        querySelectorAll: () => [],
      });
      return elements.get(id);
    },
  };
  const context = vm.createContext({
    document, window: { __TAURI__: { core: { invoke } } },
    setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0,
  });
  vm.runInContext(source, context);
  // Rendering is tested in the browser. These tests check decisions and
  // bridge calls independently from the DOM implementation.
  vm.runInContext('const actualRefresh = refresh; renderAll = () => {}; renderHeader = () => {}; renderMain = () => {}; renderSide = () => {}; afterAction = () => {}; refresh = async () => {};', context);
  const api = vm.runInContext(`({ state, filteredSessions, filteredTabs,
    canCloseSession, canCloseTab, isStale, autoMode, autoModeValues,
    refreshNow: actualRefresh, actionSucceeded, twoStep, viewActions, sessionTable, saveAuto, executeReview, reconcileList, sessionKey, tabKey, reviewPorts, updateCard, runUpdate,
    setReview(value) { state.actionReview = value; } })`, context);
  return { ...api, elements, context };
}

test('fixture loader executes real UI modules with LF, CRLF, and mixed line endings', async () => {
  for (const newline of ['\n', '\r\n', 'mixed']) {
    const source = buildScript(name => readModule(name).replace(/\r\n?/g, '\n')
      .split('\n').map((line, index, lines) => line + (index === lines.length - 1 ? ''
        : newline === 'mixed' ? (index % 2 ? '\r\n' : '\n') : newline)).join(''));
    const calls = [];
    const ui = load(async (command, args) => {
      calls.push([command, args.pid, args.expectedStartTime]);
      return { status: 'success', result: 'terminated', target: 'Fixture' };
    }, source);
    ui.setReview({ kind: 'sessions', targets: [{ id: 42, name: 'Fixture', startTime: 123 }],
      selected: new Set([42]), busy: false });
    await ui.executeReview();
    assert.deepEqual(calls, [['close_session', 42, 123]], newline);
    assert.equal(ui.state.gone.pids.has(42), true, newline);
  }
});

const sessions = [
  { pid: 1, state: 'stale', session_name: 'Billing', project: 'atlas', rss: 300, idle_secs: 100 },
  { pid: 2, state: 'stale', session_name: 'Migration', project: 'ledger', rss: 200, idle_secs: 200 },
  { pid: 3, state: 'active', session_name: 'Search', project: 'ledger', rss: 100, idle_secs: 0 },
];

test('session batch eligibility follows search and state, excluding protected sessions', () => {
  const ui = load();
  ui.state.sessFilter = 'ledger';
  ui.state.sessState = 'stale';
  const visible = ui.filteredSessions(sessions);
  assert.deepEqual(Array.from(visible, x => x.pid), [2]);
  assert.equal(ui.canCloseSession(sessions[2]), false);
  assert.equal(ui.canCloseSession({ ...sessions[0], is_self: true }), false);
  assert.equal(ui.canCloseSession({ ...sessions[0], engine: true }), false);
  ui.state.gone.pids.add(2);
  assert.equal(ui.filteredSessions(sessions).length, 0);
});

test('tab batches respect profile, query, configured threshold, and pinned/active exclusions', () => {
  const ui = load();
  ui.state.settings = { tab_stale_after_secs: 7200 };
  ui.state.tabProfile = 'Work';
  ui.state.tabFilter = 'notes';
  ui.state.tabState = 'stale';
  const tab = { id: 1, profile: 'Work', title: 'Notes', url: 'https://example.com/notes', site: 'example.com', idle_secs: 8000, active: false, pinned: false };
  const browser = { can_close_tabs: true, tabs: [tab,
    { ...tab, id: 2, profile: 'Personal' }, { ...tab, id: 3, pinned: true },
    { ...tab, id: 4, active: true }, { ...tab, id: 5, idle_secs: 100 },
  ] };
  assert.deepEqual(Array.from(ui.filteredTabs(browser), t => t.id), [1]);
  assert.equal(ui.canCloseTab(browser, browser.tabs[2]), false);
  assert.equal(ui.canCloseTab(browser, browser.tabs[3]), false);
  ui.state.settings.tab_stale_after_secs = 9000;
  assert.equal(ui.filteredTabs(browser).length, 0);
});

test('auto modes preserve server-only selection and atomically switch preview/live behavior', () => {
  const ui = load();
  const c = { auto_close_sessions: false, auto_stop_servers: true, auto_dry_run: false };
  assert.equal(ui.autoMode(c), 'on');
  assert.equal(JSON.stringify(ui.autoModeValues(c, 'preview')), JSON.stringify({ close_sessions: false, stop_servers: true, dry_run: true }));
  assert.equal(JSON.stringify(ui.autoModeValues(c, 'off')), JSON.stringify({ close_sessions: false, stop_servers: false }));
  assert.equal(JSON.stringify(ui.autoModeValues({ ...c, auto_stop_servers: false }, 'on')), JSON.stringify({ close_sessions: true, stop_servers: false, dry_run: false }));
});

test('settings writes cannot overlap', async () => {
  let finish, calls = 0;
  const ui = load(() => { calls++; return new Promise(resolve => { finish = resolve; }); });
  const writing = ui.saveAuto({ dry_run: true });
  await ui.saveAuto({ dry_run: false });
  assert.equal(calls, 1);
  finish({ auto_close_sessions: true, auto_stop_servers: false, auto_dry_run: true });
  await writing;
  assert.equal(ui.autoMode(ui.state.settings), 'preview');
  assert.equal(ui.state.settingsBusy, false);
});

test('confirmation sends only selected frozen identities and accounts for partial failure', async () => {
  const calls = [];
  const ui = load(async (command, args) => {
    calls.push({ command, args });
    if (args.pid === 2) throw new Error('Now active');
    return { target: 'Billing', result: 'terminated' };
  });
  ui.setReview({ kind: 'sessions', targets: [
    { id: 1, name: 'Billing', startTime: 100 },
    { id: 2, name: 'Migration', startTime: 200 },
    { id: 3, name: 'Unchecked', startTime: 300 },
  ], selected: new Set([1, 2]), busy: false });
  await ui.executeReview();
  assert.deepEqual(calls.map(c => [c.command, c.args.pid, c.args.expectedStartTime]), [['close_session', 1, 100], ['close_session', 2, 200]]);
  assert.equal(ui.state.gone.pids.has(1), true);
  assert.equal(ui.state.gone.pids.has(2), false);
  assert.match(ui.elements.get('review-status').textContent, /Now active/);
  assert.equal(ui.elements.get('review-submit').textContent, 'View actions');
});

test('tab confirmation sends the reviewed URL and profile', async () => {
  let payload;
  const ui = load(async (command, args) => { assert.equal(command, 'close_tabs'); payload = args; return [{ result: 'closed' }]; });
  ui.setReview({ kind: 'tabs', browser: 'Chrome', targets: [{ id: 7, url: 'https://example.com', profile: 'Work' }], selected: new Set([7]), busy: false });
  await ui.executeReview();
  assert.equal(JSON.stringify(payload.expectedTabs), JSON.stringify([{ id: 7, url: 'https://example.com', profile: 'Work' }]));
  assert.equal(ui.state.gone.tabs.has(7), true);
});

test('a failed request never offers an automatic retry', async () => {
  const ui = load(async () => { throw new Error('Connection interrupted'); });
  ui.setReview({ kind: 'tabs', browser: 'Chrome', targets: [{ id: 7, url: 'https://example.com', profile: 'Work' }], selected: new Set([7]), busy: false });
  await ui.executeReview();
  assert.match(ui.elements.get('review-status').textContent, /Check Actions and refresh/);
  assert.equal(ui.elements.get('review-submit').textContent, 'View actions');
  assert.equal(ui.state.gone.tabs.size, 0);
});

test('history escapes commands and does not offer resume for preview-only actions', () => {
  const ui = load();
  ui.state.log = [{ ts: 1, pid: 1, mode: 'dry-run', action: 'close_session', target: '<img src=x>', result: 'dry run', rss: 1, resume: 'echo "test"' }];
  const rendered = ui.viewActions();
  assert.match(rendered, /&lt;img src=x&gt;/);
  assert.doesNotMatch(rendered, /data-copy-command/);
  ui.state.log[0].mode = 'manual';
  assert.match(ui.viewActions(), /data-copy-command="echo &quot;test&quot;"/);
});

test('tab recovery keeps shortcut hints outside the copy control and preserves quoted URLs', () => {
  const ui = load();
  ui.state.log = [{ ts: 1, mode: 'manual', action: 'close_tab', target: 'Notes', result: 'closed',
    resume: "open 'https://example.com'   # or ⌘⇧T in the same browser profile" }];
  const rendered = ui.viewActions();
  const button = rendered.match(/<button[^>]*data-copy-command[\s\S]*?<\/button>/)[0];
  assert.match(button, /<code>open 'https:\/\/example.com'<\/code>/);
  assert.doesNotMatch(button, /⌘⇧T/);
  assert.match(rendered, /recovery-hint.*Or press ⌘⇧T/);
  ui.state.log[0].resume = "open 'https://example.com/   # or ⌘⇧T in the URL'";
  assert.doesNotMatch(ui.viewActions(), /class="muted recovery-hint"/);
});

test('copy control copies the displayed command and retains success feedback', async () => {
  const ui = load();
  let copied;
  ui.context.navigator = { clipboard: { writeText: async text => { copied = text; } } };
  const classes = new Set();
  const label = { textContent: 'Copy' };
  ui.context.copyButton = {
    classList: { contains: c => classes.has(c), add: (...cs) => cs.forEach(c => classes.add(c)), remove: (...cs) => cs.forEach(c => classes.delete(c)) },
    querySelector: selector => selector === 'code' ? { textContent: 'echo "test"' } : label,
    setAttribute() {}, removeAttribute() {},
  };
  await vm.runInContext('copyCommand(copyButton)', ui.context);
  assert.equal(copied, 'echo "test"');
  assert.equal(label.textContent, '✓ Copied!');
  assert.equal(classes.has('copied'), true);
  assert.equal(classes.has('copy-feedback'), true);
  assert.equal(classes.has('copying'), false);
});


test('compact selections drop hidden, protected, and replaced targets', () => {
  const ui = load();
  const oldSession = { pid: 7, start_time: 100 };
  const oldTab = { id: 8, profile: 'Work', url: 'https://example.com/old' };
  const browser = { name: 'Chrome' };
  const ids = [ui.sessionKey(oldSession), ui.tabKey(browser, oldTab), 'hidden', 'protected'];
  const saved = ui.reconcileList('list', ids.map(id => ({ id, eligible: true })));
  ids.forEach(id => saved.selected.add(id));
  const next = ui.reconcileList('list', [
    { id: ui.sessionKey({ ...oldSession, start_time: 200 }), eligible: true },
    { id: ui.tabKey(browser, { ...oldTab, url: 'https://example.com/new' }), eligible: true },
    { id: 'protected', eligible: false },
  ]);
  assert.equal(next.selected.size, 0);
  assert.equal(ui.reconcileList('another-list', ids.map(id => ({ id, eligible: true }))).selected.size, 0);
});

test('inspection follows the same item across reordering and recovers when it disappears', () => {
  const ui = load();
  const saved = ui.reconcileList('sessions', [{ id: 'a' }, { id: 'b' }]);
  saved.inspected = 'b';
  assert.equal(ui.reconcileList('sessions', [{ id: 'b' }, { id: 'a' }]).inspected, 'b');
  assert.equal(ui.reconcileList('sessions', [{ id: 'a' }]).inspected, 'a');
  assert.equal(ui.reconcileList('sessions', []).inspected, null);
});

test('port review excludes managed services and stops each selected process once', async () => {
  const calls = [];
  const ui = load(async (command, args) => { calls.push({ command, args }); return { result: 'terminated' }; });
  vm.runInContext('openReview = (kind, targets) => { state.actionReview = {kind, targets, selected: new Set(targets.map(t => t.id)), busy: false}; };', ui.context);
  ui.state.snap = { ports: [
    { pid: 7, port: 3000, process: 'node', owner: 'Terminal', owner_managed: false },
    { pid: 7, port: 3001, process: 'node', owner: 'Terminal', owner_managed: false },
    { pid: 8, port: 5432, process: 'postgres', owner: 'Database app', owner_managed: true },
  ] };
  ui.reviewPorts(ui.state.snap.ports);
  await ui.executeReview();
  assert.deepEqual(calls.map(c => [c.command, c.args.pid, c.args.force]), [['stop_server', 7, false]]);
  assert.match(ui.elements.get('review-status').textContent, /1 of 1 server stopped/);
});

test('updates escape release notes and disable installation during a download', () => {
  const ui = load();
  ui.state.update = { enabled: true, current_version: '0.2.0', phase: 'available', version: '0.3.0', notes: '<img src=x onerror=alert(1)>', error: null };
  assert.match(ui.updateCard(), /Install and restart/);
  assert.doesNotMatch(ui.updateCard(), /<img/);
  assert.match(ui.updateCard(), /&lt;img/);
  ui.state.update.phase = 'installing';
  assert.match(ui.updateCard(), /data-update="install" disabled/);
  ui.state.update.enabled = false;
  assert.doesNotMatch(ui.updateCard(), /data-update=/);
});

test('update clicks cannot overlap and cannot choose an arbitrary download', async () => {
  const calls = [];
  let finish;
  const ui = load((name, args) => {
    calls.push([name, args]);
    if (name === 'install_update') return new Promise(resolve => { finish = resolve; });
    return Promise.resolve({ enabled: true, phase: 'available', version: '0.3.0' });
  });
  ui.state.update = { enabled: true, phase: 'available', version: '0.3.0' };
  const first = ui.runUpdate('install');
  await ui.runUpdate('install');
  await ui.runUpdate('check');
  assert.deepEqual(calls, [['install_update', undefined]]);
  finish();
  await first;
  assert.equal(ui.state.updateBusy, false);
});

test('older and same-second daemon snapshots cannot undo a fresh post-action scan', async () => {
  let snap = { taken_at: 200, sessions: [], browsers: [], marker: 'fresh' };
  const ui = load(async command => command === 'snapshot' ? snap : command === 'action_log' ? [] : {});
  await ui.refreshNow({ fresh: true });
  assert.equal(ui.state.snap.marker, 'fresh');
  snap = { ...snap, marker: 'stale daemon' };
  await ui.refreshNow(); assert.equal(ui.state.snap.marker, 'fresh');
  snap = { ...snap, taken_at: 199 };
  await ui.refreshNow(); assert.equal(ui.state.snap.marker, 'fresh');
  snap = { ...snap, taken_at: 201, marker: 'next tick' };
  await ui.refreshNow(); assert.equal(ui.state.snap.marker, 'next tick');
});

test('failed actions remain visible when log polling fails', async () => {
  const ui = load(async command => {
    if (command === 'action_log') throw new Error('temporary log read failure');
    if (command === 'snapshot') return {taken_at: 1, sessions: [], browsers: []};
    return {};
  });
  ui.state.log = [{status:'partial', target:'fixture', result:'partial failure: child survived'}];
  await ui.refreshNow(); assert.equal(ui.state.log.length, 1);
  for (const status of ['partial','failure','intent','dry_run','skipped']) {
    assert.equal(ui.actionSucceeded({status, result:'terminated', mode:'auto'}), false);
  }
  assert.equal(ui.actionSucceeded({status:'success', result:'terminated', mode:'dry-run'}), false);
});

test('two-step confirmation does not call IPC on its first click', async () => {
  const ui = load(); let calls = 0;
  const button = {dataset:{}, textContent:'Quit', classList:{add(){},remove(){}}};
  const run = async () => { calls++; };
  ui.twoStep(button, 'fixture', run); assert.equal(calls, 0); assert.equal(button.textContent, 'Confirm');
  ui.twoStep(button, 'fixture', run); await Promise.resolve(); assert.equal(calls, 1);
});
