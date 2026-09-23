// Synthetic data only: no native tab or process actions.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ executablePath: process.env.AUTOTRIM_TEST_CHROMIUM });
  try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 740 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => {
      const file = new URL(route.request().url()).pathname.slice(1) || 'index.html';
      if (file === 'logo.png') return route.fulfill({path:path.join(__dirname, '../tray/ui/logo.png'),contentType:'image/png'});
      if (file === 'main.js') return route.fulfill({ contentType: 'text/javascript', body: `
        import { state } from './state.js';
        import { renderAll, refresh } from './render.js';
        Object.assign(window, { state, renderAll, refresh });
      ` });
      if (!['index.html', 'styles.css', 'format.js', 'state.js', 'render.js', 'actions.js'].includes(file)) return route.abort();
      return route.fulfill({ body: fs.readFileSync(path.join(__dirname, '../tray/ui', file)), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://autotrim.test/');
    await page.evaluate(() => {
      const GB = 1024 ** 3, now = Date.now() / 1000;
      const tabs = Array.from({ length: 80 }, (_, i) => ({ id: i + 1, title: i ? `Project notes ${String(i).padStart(2, '0')}` : 'Roadmap & release planning', url: `https://example.com/projects/roadmap/${i}?view=notes`, site: i < 40 ? 'example.com' : 'notes.example.com', profile: 'Work', window_id: 2, index: i + 1, idle_secs: (80 - i) * 7200, active: i === 3, pinned: i === 4, kind: 'web' }));
      const chrome = { name: 'Chrome', tabs, open_profiles: [{ dir: 'Work', label: 'Work profile' }], per_tab_estimate: 100 * 1024 ** 2, can_close_tabs: true, renderer_rss: 8 * GB, renderers: 10, extension_renderers: 1 };
      const group = { name: 'Chrome', kind: 'app', rss: 9 * GB, cpu: 3, procs: 20, pids: [] };
      state.snap = { taken_at: now, system: { total_mem: 32 * GB, used_mem: 16 * GB, used_swap: 0, total_swap: GB, uptime_secs: 1000, cpu_pct: 5, load_one: 1, load_five: 1, load_fifteen: 1 }, groups: [group], browsers: [chrome], sessions: [], ports: [], trends: [], advice: [] };
      state.src = { daemon_running: true, interval_secs: 30, snapshot_taken_at: now };
      state.view = 'g:Chrome';
      window.calls = [];
      window.__TAURI__ = { core: { invoke: async (command, args) => {
        calls.push({ command, args });
        if (command === 'close_tabs') return args.expectedTabs.map(t => ({ target: t.url, status: 'skipped', result: 'Fixture: kept open' }));
        return { snapshot: state.snap, source_info: state.src, action_log: [], settings: null, service_info: null }[command];
      } } };
      renderAll(true);
    });
    const list = page.locator('.resource-list').first();
    const rows = list.locator('.work-row');
    const items = list.locator('.work-item');
    const checked = () => rows.locator('input:checked').count();
    const options = page.locator('.work-heading .list-options');
    async function openOptions() { if (!await options.evaluate(el => el.open)) await options.locator('summary').click(); }
    async function chooseAll() { await openOptions(); await page.locator('[data-list-eligible]').first().click(); }
    async function clear() { if (await list.locator('[data-list-clear]').count()) await list.locator('[data-list-clear]').click(); }
    async function sort(value, session = false) { await openOptions(); await page.locator(session ? '[data-sess-sort]' : '[data-tab-sort]').selectOption(value); await options.locator('summary').press('Escape'); }
    assert.equal(await page.locator('.resource-table, .holder-meter').count(), 0, 'no comparison tables or sidebar bars');
    assert.equal(await list.locator('.list-inspector').count(), 0);
    assert.equal(await list.locator('.selection-bar').count(), 0);
    assert.equal(await page.locator('[data-tab-filter]').count(), 0, 'search starts tucked into the heading');
    assert.equal(await page.locator('.browser-details').evaluate(el => el.open), false);
    assert.equal(await list.locator('.work-group').count(), 2, 'group by website, independently of activity');
    await openOptions();
    await page.locator('[data-tab-profile]').selectOption('Work');
    assert.match(await options.locator('summary').textContent(), /Work profile/);
    await page.locator('[data-tab-profile]').selectOption('');
    await page.locator('[data-tab-sort]').press('Escape');
    assert.equal(await options.evaluate(el => el.open), false);
    assert.match(await rows.first().locator('.row-context').getAttribute('title'), /Work profile · Window 2 · Tab 1/);
    // Direct closes carry exact targets and exclude pinned/active tabs.
    await openOptions(); await page.locator('[data-list-stale]').first().click();
    assert.equal(await page.locator('#action-review').evaluate(el => el.open), false);
    let targets = await page.evaluate(() => calls.filter(c => c.command === 'close_tabs').at(-1).args.expectedTabs);
    assert.equal(targets.length, 67);
    assert.equal(targets.some(t => t.id === 4 || t.id === 5), false);
    await rows.first().locator('input').check();
    await rows.nth(5).locator('input').click({modifiers:['Shift']});
    assert.equal(await checked(), 5, 'range selection skips pinned tabs; active tabs have no checkbox');
    await list.locator('[data-list-group="example.com"]').click();
    assert.equal(await rows.count(), 40);
    assert.equal(await checked(), 0, 'collapsing a group discards its selection');
    await page.evaluate(() => refresh());
    assert.equal(await list.locator('[data-list-group="example.com"]').getAttribute('aria-expanded'), 'false');
    await chooseAll(); assert.equal(await checked(), 40);
    await list.locator('[data-list-review]').click();
    assert.equal(await page.evaluate(() => calls.filter(c => c.command === 'close_tabs').at(-1).args.expectedTabs.length), 40);
    await clear();
    await list.locator('[data-list-group="notes.example.com"]').click();
    assert.equal(await rows.count(), 0);
    // Searching reveals matches from collapsed websites without changing their saved state.
    await page.locator('[data-search-toggle]').click();
    await page.locator('[data-tab-filter]').fill('roadmap/0?');
    assert.equal(await rows.count(), 1);
    assert.equal(await list.locator('.work-group').count(), 1);
    await list.locator('[data-list-group="example.com"]').click(); assert.equal(await rows.count(), 0);
    await list.locator('[data-list-group="example.com"]').click(); assert.equal(await rows.count(), 1);
    await page.locator('[data-tab-filter]').fill('');
    assert.equal(await rows.count(), 0, 'clearing a search restores collapsed websites');
    await page.locator('[data-tab-filter]').press('Escape');
    assert.equal(await page.locator('[data-tab-filter]').count(), 0);
    await list.locator('[data-list-group="example.com"]').click();
    await list.locator('[data-list-group="notes.example.com"]').click();
    // Item details and selection are independent. Keyboard activation opens details.
    await rows.first().locator('input').check();
    await rows.first().locator('.row-title').focus(); await page.keyboard.press('Enter');
    assert.equal(await items.first().locator('.work-detail').count(), 1);
    assert.equal(await checked(), 1);
    assert.match(await items.first().locator('.work-detail').textContent(), /≈ 100 MiB/);
    await page.evaluate(() => refresh());
    assert.equal(await items.first().locator('.work-detail').count(), 1);
    await rows.first().locator('.row-title').press('Space');
    assert.equal(await items.first().locator('.work-detail').count(), 0);
    await rows.nth(1).locator('.row-title').click();
    await items.nth(1).locator('[data-close-tab]').click();
    assert.equal(await page.locator('#action-review').evaluate(el => el.open), false);
    assert.equal(await page.evaluate(() => calls.filter(c => c.command === 'close_tabs').at(-1).args.expectedTabs.length), 1);
    await clear();
    await chooseAll(); assert.equal(await checked(), 78);
    assert.equal(await list.locator('[data-list-review]').textContent(), 'Close 78 tabs');
    await list.locator('[data-list-review]').click();
    assert.equal(await page.evaluate(() => calls.filter(c => c.command === 'close_tabs').at(-1).args.expectedTabs.length), 78);
    await clear();
    await rows.first().locator('input').check();
    await sort('title'); assert.equal(await checked(), 1, 'sort keeps selection by identity');
    await sort('idle'); await clear();
    // The page has one scroll area; refresh and selection do not jump back to the top.
    await page.locator('#main').evaluate(el => el.scrollTop = 1600);
    const before = await page.locator('#main').evaluate(el => el.scrollTop);
    await page.evaluate(() => refresh());
    assert.equal(await page.locator('#main').evaluate(el => el.scrollTop), before);
    await rows.nth(25).locator('input').check();
    assert.ok(await page.locator('#main').evaluate(el => el.scrollTop) > 1000);
    const bar = await list.locator('.selection-bar').boundingBox();
    assert.ok(bar.y >= 0 && bar.y + bar.height <= 740);
    await page.locator('[data-search-toggle]').click();
    await page.locator('[data-tab-filter]').fill('roadmap/20?');
    assert.equal(await rows.count(), 1); assert.equal(await checked(), 0);
    await page.locator('[data-tab-filter]').press('Escape');
    await page.locator('[data-activity-filter]').selectOption('stale');
    assert.equal(await rows.count(), 67);
    await page.locator('[data-activity-filter]').selectOption('all');
    await page.evaluate(() => {
      const session = (pid, name, rss, activity) => ({ pid, session_name:name, project:'/projects/atlas', host:'Terminal', kind:'codex', start_time:pid, rss, idle_secs:pid * 7200, age_secs:86400, state:activity, threads:[] });
      state.snap.sessions = [session(1,'Billing',3000,'stale'),session(2,'Search',1000,'stale'),session(3,'Active work',2000,'active')];
      state.snap.groups.push({name:'Codex',kind:'agent',rss:6000,cpu:1,procs:3,pids:[1,2,3]});
      state.view='g:Codex'; renderAll(true);
    });
    assert.equal(await list.locator('.work-group').count(), 1);
    assert.equal(await rows.first().locator('.row-title').textContent(), 'Billing');
    await sort('name', true); assert.equal(await rows.first().locator('.row-title').textContent(), 'Active work');
    await sort('idle', true); assert.equal(await rows.first().locator('.row-title').textContent(), 'Active work');
    await sort('rss', true); assert.equal(await rows.first().locator('.row-title').textContent(), 'Billing');
    await rows.first().locator('input').check();
    await sort('name', true); assert.equal(await checked(),1);
    await chooseAll(); assert.equal(await checked(),2);
    await list.locator('[data-list-review]').click(); assert.equal(await page.locator('.review-target').count(),2);
    assert.doesNotMatch(await page.locator('.review-targets').textContent(),/Active work/);
    await page.locator('#review-cancel').click(); await clear();
    await page.evaluate(() => {
      const now=state.snap.taken_at;
      state.snap.sessions.push({pid:10,start_time:10,kind:'codex',host:'Codex app',engine:true,state:'active',rss:1024**3,age_secs:86400,idle_secs:0,threads:[
        {id:'a',name:'Current loaded task',cwd:'/projects/atlas',last_activity:now-10},
        {id:'b',name:'Older loaded task',cwd:'/other/atlas',last_activity:now-90000},
        {id:'helper',name:'Review helper',helper:true,cwd:'/other/atlas',last_activity:null},
      ]});
      state.snap.groups.find(g=>g.name==='Codex').pids.push(10);renderAll(true);
    });
    assert.equal(await list.locator('.work-group').count(),2, 'same basename in separate paths remains distinct');
    assert.match(await list.locator('.group-name').first().textContent(), /\/projects\/atlas/);
    const loaded=items.filter({has:page.locator('.row-title', {hasText:'Older loaded task'})});
    assert.equal(await loaded.locator('input').count(),0,'loaded tasks cannot be selected for closing');
    await loaded.locator('.row-title').click();
    assert.match(await loaded.locator('.work-detail').textContent(),/Shared by the backend/);
    assert.equal(await loaded.locator('[data-close-session]').count(),0);
    assert.match(await page.locator('.backend-resources').textContent(),/Codex backend/);
    await page.locator('[data-search-toggle]').click(); await page.locator('[data-session-filter]').fill('Older loaded');
    assert.equal(await rows.count(),1);
    await page.evaluate(() => refresh());
    assert.equal(await page.locator('[data-session-filter]').inputValue(),'Older loaded');
    await page.locator('[data-session-filter]').press('Escape');
    await page.locator('[data-activity-filter]').selectOption('stale'); assert.equal(await rows.count(),2);
    assert.equal(await list.locator('.row-lock').count(),0,'session filters never classify transcript activity as stale work');
    await page.locator('[data-activity-filter]').selectOption('all');
    // Compact, representative snapshots for visual QA of each production view.
    await page.evaluate(() => {
      const chrome=state.snap.browsers[0];chrome.tabs=chrome.tabs.slice(0,8);
      chrome.tabs.forEach((t,i)=>{t.site=['figma.com','github.com','notion.so'][i%3];t.url=`https://${t.site}/project/${i}`;t.title=['Dashboard exploration','autoTrim pull requests','Next release checklist','Design system references','Review the memory fix','Planning notes','Settings components','Improve session grouping'][i];});
      const MB=1024**2;
      state.snap.sessions.filter(x=>!x.engine).forEach((x,i)=>{x.rss=(300-i*50)*MB;x.kind='claude_code';x.project=i===2?'/projects/studio':'/projects/autoTrim';});
      state.snap.groups.push({name:'Claude Code sessions',kind:'agent',rss:750*MB,cpu:2,procs:3,pids:[1,2,3]});
      const codex=state.snap.groups.find(g=>g.name==='Codex');codex.pids=[10];codex.rss=1024**3;
      state.settings={auto_close_sessions:false,auto_stop_servers:false,auto_close_tabs:false,auto_dry_run:true,auto_grace_minutes:10,auto_tab_domains:[],tab_stale_after_secs:86400};
      state.service={supported:true,installed:true,running:true};
    });
    for (const width of [1200,1000,760,390]) {
      await page.setViewportSize({width,height:800});
      for (const view of ['g:Chrome','g:Codex','g:Claude Code sessions','overview','settings','actions']) {
        await page.evaluate(view=>{state.view=view;state.listState.clear();state.searchOpen.clear();state.tabFilter='';state.sessFilter='';renderAll(true);},view);
        if (view === 'g:Chrome') await rows.first().locator('input').check();
        assert.equal(await page.locator('#main').evaluate(el=>el.scrollWidth>el.clientWidth),false,`${view} overflows at ${width}`);
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
        await page.screenshot({path:`/private/tmp/autotrim-grouped-${view.replace(/[^a-z]/gi,'-')}-${width}.png`});
      }
    }
    assert.deepEqual(errors, []);
    console.log('PASS grouped work: website/project groups, search, filters, selection, range, collapse, refresh, exact reviews, protected backends, task attribution, and responsive app-wide design');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
