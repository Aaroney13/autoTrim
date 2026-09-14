// Synthetic browser preview. Loaded only by tests/preview-tray.py.
const MB = 1024 ** 2;
const epoch = Math.floor(Date.now() / 1000);
const sessions = [
  { pid: 100, start_time: 10, kind: 'claude_code', host: 'terminal', project: '/fixture/atlas', cwd: '/fixture/atlas', session_name: 'Atlas research', state: 'stale', rss: 300*MB, cpu: 0, age_secs: 50000, idle_secs: 40000, quiet_for_secs: 10000, procs: 1, pids: [100], ports: [], transcript: '/fixture/transcript.jsonl', session_id: 'fixture', is_self: false, engine: false },
  { pid: 101, start_time: 11, kind: 'claude_code', host: 'terminal', project: '/fixture/current', session_name: 'Current work', state: 'active', rss: 200*MB, cpu: 10, age_secs: 1000, idle_secs: 0, pids: [101], ports: [], is_self: true, engine: false },
];
const tab = { id: 7, window_id: 1, profile: 'Default', index: 0, url: 'https://example.com/fixture', site: 'example.com', title: 'Fixture notes', pinned: false, active: false, last_active: epoch-90000, idle_secs: 90000, kind: 'chat' };
const snapshot = { taken_at: epoch, scanner_pid: 999, system: { os: 'Fixture', total_mem: 16000*MB, used_mem: 10000*MB, available_mem: 6000*MB, total_swap: 2000*MB, used_swap: 500*MB, compressed: 1000*MB, wired: 1000*MB, free_pct: 38, uptime_secs: 90000, cpu_pct: 4, load_one:0.5, load_five:0.4, load_fifteen:0.3 },
  groups: [ { name: 'Claude Code sessions', kind: 'agent', rss: 500*MB, cpu: 10, procs: 2, pids: [100,101] }, { name: 'Chrome', kind: 'browser', rss: 400*MB, cpu: 1, procs: 4, pids: [200] }, { name: 'Notes', kind: 'app', rss: 100*MB, cpu: 0, procs: 1, pids: [300] } ],
  sessions, browsers: [{ name: 'Chrome', rss: 400*MB, renderers: 2, renderer_rss: 300*MB, extension_renderers: 0, tab_sized_renderers: 2, small_renderers: 0, windows: 1, tabs: [tab], open_profiles: [{dir:'Default',label:'Personal'}], sites: [{site: 'example.com', tabs: 1, stale_tabs: 1, oldest_idle_secs: 90000, est_rss:300*MB, kind:'chat'}], per_tab_estimate: 300*MB, can_close_tabs: true, stale_tabs:1, chat_tabs:1, stale_chat_tabs:1 }],
  ports: [{ pid: 400, start_time: 10, port: 3000, protocol: 'TCP', addr: '127.0.0.1', process: 'node', owner: 'node', owner_managed: false, owner_rss: 80*MB, owner_cpu:0, open_for_secs: 90000, label:'Fixture server', label_source:'process' }], trends: [], advice: [], auto: null };
let settings = { auto_close_sessions: false, auto_stop_servers: false, auto_dry_run: false, auto_grace_minutes: 10, auto_hosts:['terminal'], tab_stale_after_secs:86400, open_window_at_launch:true };
const logs = [];
window.__TAURI__ = { core: { async invoke(command, args = {}) {
  if (command === 'snapshot') return structuredClone(snapshot);
  if (command === 'source_info') return { daemon_running: true, interval_secs:30, snapshot_taken_at:snapshot.taken_at };
  if (command === 'action_log') return structuredClone(logs);
  if (command === 'settings') return structuredClone(settings);
  if (command === 'service_info') return {installed:false, running:false, supported:true};
  if (command === 'update_status') return {enabled:false,current_version:'fixture'};
  if (command === 'set_auto') { settings = {...settings, auto_close_sessions:args.close_sessions, auto_stop_servers:args.stop_servers, auto_dry_run:args.dry_run}; return settings; }
  if (command === 'close_session' || command === 'stop_server') {
    const rec = {ts:epoch, id:'fixture-action', status:'partial', mode:'manual', action:command, pid:args.pid, target:'Fixture target', rss:300*MB, result:'partial failure: fixture child survived', resume:'echo fixture-resume'};
    logs.push(rec); return rec;
  }
  if (command === 'close_tabs') { const rec = {ts:epoch,status:'success',mode:'manual',action:'close_tab',target:'Fixture notes',result:'closed',resume:'open https://example.com/fixture'}; logs.push(rec); return [rec]; }
  if (command === 'quit_app' || command === 'restart_app') { const rec={ts:epoch,status:'success',mode:'manual',action:command,target:args.name,result:'quit',rss:100*MB}; logs.push(rec); return rec; }
  if (command === 'hide_window') return null;
  throw new Error(`Fixture refuses unsupported command: ${command}`);
} } };
