// Run with: node tests/tray-interactions.browser.cjs (requires Playwright + Chromium).
// Real rendering and input with synthetic snapshots; never calls the daemon or native actions.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const uiDir = path.join(__dirname, '../tray/ui');
(async () => {
  const browser = await chromium.launch({executablePath: process.env.AUTOTRIM_TEST_CHROMIUM});
  const failures = []; let passed = 0;
  try {
    const page = await browser.newPage({viewport:{width:1000,height:740}});
    // Load the real ES modules and styles without starting the native poll loop.
    // Every request is fulfilled locally; the test cannot contact external services.
    const files = new Set(['index.html', 'styles.css', 'format.js', 'state.js', 'actions.js', 'render.js']);
    await page.route('**/*', route => {
      const file = new URL(route.request().url()).pathname.slice(1) || 'index.html';
      if (file === 'logo.png') return route.fulfill({path:path.join(__dirname, '../tray/ui/logo.png'),contentType:'image/png'});
      if (file === 'main.js') return route.fulfill({contentType:'text/javascript',body:`
        import { GB, epochNow } from './format.js';
        import { state } from './state.js';
        import { renderAll, refresh } from './render.js';
        Object.assign(window, { GB, epochNow, state, renderAll, refresh,
          navigate: view => document.querySelector('[data-view="' + view + '"]').click() });
      `});
      if (!files.has(file)) return route.abort();
      return route.fulfill({path:path.join(uiDir,file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    await page.goto('http://autotrim.test/');
    await page.evaluate(() => {
      const port = (pid, port, label, managed=false) => ({pid,port,label,process:'node',protocol:'TCP',addr:'127.0.0.1',owner:'Terminal',owner_managed:managed,open_for_secs:3600});
      const trend = (key,name,growth) => ({key,name,growth,kind:'group',span_secs:7200,rss_now:4*GB,bytes_per_hour:GB,cpu_mean:5,rising_frac:.8});
      const chromeTabs = [
        {id:1,window_id:1,profile:'Default',index:0,url:'https://www.example.com/a',site:'example.com',title:'Example home',pinned:false,active:false,idle_secs:90000,kind:'page'},
        {id:2,window_id:1,profile:'Default',index:1,url:'https://www.example.com/b',site:'example.com',title:'Example notes',pinned:true,active:false,idle_secs:90000,kind:'page'},
        {id:3,window_id:2,profile:'Work',index:0,url:'http://shop.example.com:8080/cart',site:'example.com',title:'Example shop',pinned:false,active:true,idle_secs:10,kind:'page'},
      ];
      const chrome = {name:'Chrome',rss:400*1024*1024,renderers:2,renderer_rss:300*1024*1024,extension_renderers:0,tabs:chromeTabs,open_profiles:[{dir:'Default',label:'Personal'},{dir:'Work',label:'Work'}],per_tab_estimate:100*1024*1024,can_close_tabs:true};
      state.snap = {taken_at:epochNow(),system:{total_mem:16*GB,used_mem:8*GB,used_swap:0,total_swap:GB,uptime_secs:1000,cpu_pct:5,load_one:1,load_five:1,load_fifteen:1},groups:[{name:'Chrome',kind:'browser',rss:chrome.rss,cpu:1,procs:3,pids:[200]}],browsers:[chrome],sessions:[],advice:[],ports:[port(101,3000,'Frontend server'),port(102,8080,'API server'),port(103,9000,'Managed service',true)],trends:[trend('g:Chrome','Google Chrome',3*GB),trend('g:Codex','Codex sessions',2*GB)],auto:{close_sessions:false,stop_servers:false,close_tabs:false,dry_run:true,tab_rules_revision:'browser-0',pending:[]}};
      state.src={daemon_running:true,interval_secs:30,snapshot_taken_at:epochNow()};
      state.settings={auto_close_sessions:false,auto_stop_servers:false,auto_close_tabs:false,auto_dry_run:true,auto_grace_minutes:10,auto_tab_inactive_hours:24,auto_tab_domains:[],tab_rules_revision:'browser-0',auto_hosts:['terminal'],tab_stale_after_secs:86400,open_window_at_launch:true};
      state.service={installed:true,running:true,supported:true};
      window.__bridgeCalls=[];
      window.__previewRows=[
        {browser:'Chrome',profile:'Personal',window_id:1,id:1,title:'Example home',domain:'www.example.com',idle_secs:90000,status:'eligible',reason:'Past the shared timer'},
        {browser:'Chrome',profile:'Personal',window_id:1,id:2,title:'Example notes',domain:'www.example.com',idle_secs:90000,status:'pinned',reason:'Pinned tabs stay open'},
        {browser:'Chrome',profile:'Work',window_id:2,id:3,title:'Example shop',domain:'shop.example.com',idle_secs:10,status:'selected',reason:'Selected in its window'},
        {browser:'Chrome',profile:'Work',window_id:2,id:4,title:'Background',domain:'shop.example.com',idle_secs:null,status:'unknown_activity',reason:'Chrome did not record a selection time'},
      ];
      window.__TAURI__={core:{invoke:async (command,args={})=>{
        window.__bridgeCalls.push([command,args]);
        if(command==='snapshot') return structuredClone(state.snap);
        if(command==='source_info') return structuredClone(state.src);
        if(command==='action_log') return [];
        if(command==='settings') return structuredClone(state.settings);
        if(command==='service_info') return structuredClone(state.service);
        if(command==='update_status') return {enabled:false,current_version:'test'};
        if(command==='set_tab_rules') {
          if(args.expected_revision !== state.settings.tab_rules_revision) throw new Error('settings changed; review the current rules and try again');
          state.settings={...state.settings,auto_tab_domains:args.domains.map(rule=>({...rule,domain:rule.domain.trim().toLowerCase().replace(/\.$/, '')})),auto_tab_inactive_hours:args.inactive_hours,tab_rules_revision:'browser-'+Date.now()};
          return structuredClone(state.settings);
        }
        if(command==='set_auto') {
          if(args.close_sessions!=null) state.settings.auto_close_sessions=args.close_sessions;
          if(args.stop_servers!=null) state.settings.auto_stop_servers=args.stop_servers;
          if(args.close_tabs!=null) state.settings.auto_close_tabs=args.close_tabs;
          if(args.dry_run!=null) state.settings.auto_dry_run=args.dry_run;
          return structuredClone(state.settings);
        }
        if(command==='preview_tab_rules') return structuredClone(window.__previewRows);
        return null;
      }}};
      renderAll(true);
    });
    async function check(name, run) { try { await run(); passed++; console.log('PASS',name); } catch(e) {failures.push(name); console.log('FAIL',name,e.message);} }
    async function go(view) { await page.evaluate(view=>navigate(view),view); }
    async function openFirstRow() {
      const button = page.locator('tbody tr.compact-row').first().locator('.row-title');
      if (await button.getAttribute('aria-expanded') !== 'true') await button.click();
    }
    async function selectedFill(row) {
      return row.evaluate(row=>{const probe=document.createElement('span');probe.style.backgroundColor='var(--sel)';row.append(probe);const wanted=getComputedStyle(probe).backgroundColor;probe.remove();return [...row.cells].every(cell=>getComputedStyle(cell).backgroundColor===wanted);});
    }
    await check('inline details start closed, toggle by keyboard, and survive refresh without changing selection', async () => {
      await go('ports');
      assert.equal(await page.locator('.list-inspector').count(), 0);
      const row = page.locator('.compact-row').first();
      const button = row.locator('.row-title');
      await row.locator('input').check();
      assert.equal(await page.locator('.list-inspector').count(), 0);
      await button.focus(); await page.keyboard.press('Enter');
      assert.equal(await button.getAttribute('aria-expanded'), 'true');
      assert.equal(await row.evaluate(el => el.nextElementSibling.classList.contains('compact-detail')), true);
      await page.evaluate(() => refresh());
      assert.equal(await button.getAttribute('aria-expanded'), 'true');
      assert.equal(await row.locator('input').isChecked(), true);
      await page.locator('.inspector-actions button').click();
      assert.equal(await page.locator('#action-review').evaluate(el => el.open), true);
      await page.locator('#review-cancel').click();
      await button.focus(); await page.keyboard.press('Space');
      assert.equal(await button.getAttribute('aria-expanded'), 'false');
      assert.equal(await page.locator('.list-inspector').count(), 0);
      assert.equal(await row.locator('input').isChecked(), true);
      await page.evaluate(() => refresh());
      assert.equal(await page.locator('.list-inspector').count(), 0);
      await row.locator('input').uncheck();
    });
    await check('single-column views fit wide and narrow windows in light and dark mode', async () => {
      for (const colorScheme of ['light', 'dark']) {
        await page.emulateMedia({colorScheme});
        for (const width of [1200, 1000, 760, 390]) {
          await page.setViewportSize({width, height:740});
          for (const view of ['overview', 'ports', 'settings']) {
            await go(view);
            if (view === 'ports') await openFirstRow();
            if (view === 'settings') {
              const cards = page.locator('.settings-card');
              const first = await cards.nth(0).boundingBox(), second = await cards.nth(1).boundingBox();
              assert.ok(second.y >= first.y + first.height, 'settings sections stack');
            }
            const overflows = await page.locator('#main').evaluate(el => el.scrollWidth > el.clientWidth);
            assert.equal(overflows, false, `${view} overflows at ${width}px`);
            if (view === 'ports') {
              const table = await page.locator('.compact-table').boundingBox(), detail = await page.locator('.compact-detail').boundingBox();
              assert.ok(Math.abs(table.width - detail.width) < 1, 'details use the table width');
            }
            await page.evaluate(() => document.querySelector('#main').scrollTop = 0);
            await page.screenshot({path:`/private/tmp/autotrim-layout-${view}-${colorScheme}-${width}.png`});
          }
        }
      }
    });
    await check('settings stay compact and cleanup options survive saves and refreshes', async () => {
      await go('settings');
      const options = page.locator('[data-keep-open="cleanup-options"]');
      assert.equal(await options.evaluate(el => el.open), false);
      assert.equal(await page.locator('[data-add-domain]').isVisible(), false);
      for (const width of [1000, 760, 390]) {
        await page.setViewportSize({width, height:740});
        assert.equal(await page.locator('#main').evaluate(el => el.scrollWidth <= el.clientWidth), true);
        if (width === 1000) assert.equal(await page.locator('#main').evaluate(el => el.scrollHeight <= el.clientHeight), true);
        await page.screenshot({path:`/private/tmp/autotrim-settings-${width}.png`});
      }
      await page.setViewportSize({width:760, height:740});
      await options.locator('summary').first().focus();
      await page.keyboard.press('Enter');
      assert.equal(await page.locator('[data-add-domain]').isVisible(), true);
      await page.locator('[data-auto-mode="preview"]').check();
      await page.waitForFunction(() => !state.settingsBusy && state.settings.auto_close_sessions);
      assert.equal(await options.evaluate(el => el.open), true);
      await page.locator('[data-auto="stop_servers"]').check();
      await page.waitForFunction(() => !state.settingsBusy && state.settings.auto_stop_servers);
      assert.equal(await options.evaluate(el => el.open), true);
      await page.evaluate(() => refresh());
      assert.equal(await options.evaluate(el => el.open), true);
      await page.locator('[data-domain-hours]').selectOption('2');
      await page.waitForFunction(() => !state.settingsBusy && state.settings.auto_tab_inactive_hours === 2);
      assert.equal(await options.evaluate(el => el.open), true);
      await page.locator('[data-auto-mode="off"]').check();
      await page.waitForFunction(() => !state.settingsBusy && !state.settings.auto_close_sessions);
    });
    for (const colorScheme of ['light','dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1000,760]) {
        await page.setViewportSize({width,height:740});
        await go('overview');
        await check(`${colorScheme} ${width}px Overview has compact charts instead of trends`, async () => {
          assert.equal(await page.locator('.activity-chart').count(), 3);
          assert.equal(await page.locator('.activity-chart svg[role="img"]').count(), 3);
          assert.equal(await page.locator('.overview-trends').count(), 0);
          assert.equal(await page.locator('#main tbody tr').count(), 0);
          assert.equal(await page.locator('#main').evaluate(el => el.scrollWidth <= el.clientWidth), true);
          await page.evaluate(async () => {
            await refresh();
            state.snap = {...state.snap, taken_at: state.snap.taken_at + 30,
              system: {...state.snap.system, used_mem: 9 * GB, cpu_pct: 25, used_swap: GB / 4}};
            await refresh();
          });
          assert.equal(await page.locator('.activity-line').count(), 3);
          assert.equal(await page.locator('.activity-chart.cpu figcaption b').textContent(), '25.0%');
          await go('ports');
          await go('overview');
          assert.equal(await page.locator('.activity-line').count(), 3);
        });
      }
      await page.setViewportSize({width:1000,height:740});
      await go('ports');
      await check(`${colorScheme} checkbox selection keeps full fill while focused`,async()=>{
        await openFirstRow();
        const row=page.locator('tbody tr.compact-row').nth(1); await row.locator('input').check();
        assert.equal(await page.locator('.list-inspector h3').textContent(),'Frontend server');
        assert.equal(await selectedFill(row),true);
        await row.locator('input').uncheck();
      });
      for (const key of ['Enter','Space']) await check(`${colorScheme} ${key} selects and fills row`,async()=>{
        await openFirstRow();const row=page.locator('tbody tr.compact-row').nth(1);
        await row.locator('.row-title').focus();await page.keyboard.press(key);
        assert.equal(await page.locator('.list-inspector h3').textContent(),'API server');assert.equal(await selectedFill(row),true);
      });
    }
    await go('ports');
    for (const view of ['overview','ports','actions','settings']) await check(`${view} click survives actual background refresh`,async()=>{
      await go(view==='ports'?'overview':'ports');
      const b=await page.locator(`.nav-primary [data-view=${view}]`).boundingBox();await page.mouse.move(b.x+50,b.y+3);await page.mouse.down();
      await page.evaluate(()=>refresh());await page.mouse.up();await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,30)));
      assert.equal(await page.evaluate(()=>state.view),view);
    });
    for (const target of ['.row-context','.metric','input']) await check(`row ${target} click survives actual background refresh`,async()=>{
      await go('ports');
      await openFirstRow();
      const row=page.locator('tbody tr.compact-row').nth(1), b=await row.locator(target).boundingBox();await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.down();
      await page.evaluate(()=>refresh());await page.mouse.up();await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,30)));
      if(target==='input'){assert.equal(await row.locator('input').isChecked(),true);assert.equal(await page.locator('.list-inspector h3').textContent(),'Frontend server');}
      else assert.equal(await page.locator('.list-inspector h3').textContent(),'API server');
    });
    for (const target of ['.row-title','input']) await check(`${target} keeps keyboard focus when the web view does not focus mouse clicks`,async()=>{
      await go('ports');await page.locator('tbody tr.compact-row').nth(1).locator('input').uncheck();
      await openFirstRow();
      await page.evaluate(()=>document.addEventListener('mousedown',event=>event.preventDefault(),{once:true}));
      const control=page.locator('tbody tr.compact-row').nth(1).locator(target);await control.click();
      assert.equal(await control.evaluate(el=>el===document.activeElement),true);
      if(target==='input'){assert.equal(await control.isChecked(),true);await page.keyboard.press('Space');assert.equal(await control.isChecked(),false);}
    });
    for (const target of ['.row-title','.metric','.row-context','input','all']) await check(`${target} mouse focus has no outline; keyboard focus remains visible across refresh`,async()=>{
      await go('ports');
      await page.locator('tbody tr.compact-row').first().locator('.row-title').focus();
      await page.keyboard.press('Tab');
      // macOS WebKit leaves mouse focus to our handler. Preserve that condition here.
      await page.evaluate(()=>document.addEventListener('mousedown',event=>event.preventDefault(),{once:true}));
      const row=page.locator('tbody tr.compact-row').nth(1);
      await (target==='all'?page.locator('[data-list-all]'):row.locator(target)).click();
      const focused=target==='all'?page.locator('[data-list-all]'):row.locator(target==='input'?'input':'.row-title');
      const outline=()=>focused.evaluate(el=>getComputedStyle(el).outlineStyle);
      assert.equal(await focused.evaluate(el=>el===document.activeElement),true);
      assert.equal(await outline(),'none','pointer inspection must not draw the keyboard outline');
      await page.evaluate(()=>refresh());
      assert.equal(await outline(),'none','refresh must preserve pointer focus appearance');
      await page.keyboard.press('Tab');await page.keyboard.press('Shift+Tab');
      assert.equal(await focused.evaluate(el=>el===document.activeElement),true);
      assert.equal(await outline(),'solid','keyboard navigation must show the focus outline');
      await page.keyboard.press('Space');await page.evaluate(()=>refresh());
      assert.equal(await outline(),'solid','keyboard activation and refresh must retain the outline');
      await page.locator('[data-list-all]').uncheck();
    });
    for(const event of ['pointercancel','blur']) await check(`${event} releases deferred refresh`,async()=>{
      await go('ports'); await page.locator('tbody tr.compact-row').nth(1).locator('.row-title').hover();await page.mouse.down();
      const oldLabel = await page.locator('tbody tr.compact-row').first().locator('.row-title').textContent();
      await page.evaluate(event=>{state.snap.ports[0].label='Updated '+event;return refresh();},event);
      assert.equal(await page.locator('tbody tr.compact-row').first().locator('.row-title').textContent(),oldLabel,'refresh waits until the active press finishes');
      await page.evaluate(event=>event==='blur'?window.dispatchEvent(new Event('blur')):document.dispatchEvent(new PointerEvent('pointercancel',{isPrimary:true})),event);
      await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,30)));await page.mouse.up();
      assert.equal(await page.locator('tbody tr.compact-row').first().locator('.row-title').textContent(),'Updated '+event);
    });
    await check('releasing chorded mouse buttons does not freeze refresh',async()=>{
      await go('ports'); await page.locator('tbody tr.compact-row').nth(1).locator('.row-title').hover();
      await page.mouse.down({button:'left'}); await page.mouse.down({button:'right'});
      await page.evaluate(()=>{state.snap.ports[0].label='Updated after chord';return refresh();});
      await page.mouse.up({button:'left'}); await page.mouse.up({button:'right'});
      await page.evaluate(()=>{state.snap.ports[0].label='Updated after release';return refresh();});
      await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,30)));
      assert.equal(await page.locator('tbody tr.compact-row').first().locator('.row-title').textContent(),'Updated after release');
    });
    await check('dragging row text preserves the selection instead of inspecting',async()=>{
      await go('ports'); await openFirstRow();
      const b=await page.locator('tbody tr.compact-row').nth(1).locator('.row-context').boundingBox();
      await page.mouse.move(b.x+1,b.y+b.height/2);await page.mouse.down();
      await page.mouse.move(b.x+b.width-1,b.y+b.height/2,{steps:10});
      await page.evaluate(()=>refresh());await page.mouse.up();await page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,30)));
      assert.match(await page.evaluate(()=>window.getSelection().toString()),/Terminal/);
      assert.equal(await page.locator('.list-inspector h3').textContent(),'Updated after release');
      await page.locator('tbody tr.compact-row').nth(1).locator('.metric').click();
      assert.equal(await page.locator('.list-inspector h3').textContent(),'API server');
    });
    await check('managed rows inspect without selectable or destructive controls',async()=>{
      await go('ports');const row=page.locator('tbody tr.compact-row').nth(2);await row.locator('.row-context').click();assert.equal(await page.locator('.list-inspector h3').textContent(),'Managed service');assert.equal(await row.locator('input').count(),0);assert.equal(await page.locator('.inspector-actions button').isDisabled(),true);
    });
    await check('memory breakdown and observed action changes fit desktop and narrow windows', async () => {
      await page.evaluate(() => {
        window.getSelection().removeAllRanges();
        Object.assign(state.snap.system, { os:'macOS fixture', total_mem:16*GB, used_mem:14*GB, compressed:6*GB, wired:3*GB, free_pct:35 });
        state.log = [{ ts:epochNow(), pid:0, mode:'manual', action:'close_tab', status:'success', target:'Fixture browser tab', result:'closed', rss:GB,
          memory_observation: { before:{taken_at_ms:1000,used_mem:14*GB,used_swap:GB}, after:{taken_at_ms:2500,used_mem:13.8*GB,used_swap:GB+1024**2}, tab_batch:true, attempted_actions:5 } }];
        state.view = 'actions'; renderAll(true);
      });
      for (const width of [1000, 390]) {
        await page.setViewportSize({width,height:740});
        await page.locator('#memory-summary').click();
        assert.equal(await page.locator('#memory-details').getAttribute('open'), '');
        assert.match(await page.locator('.memory-detail').textContent(), /outside used \(includes cache\)/);
        assert.doesNotMatch(await page.locator('#head').textContent(), /35%|free|unused/);
        assert.match(await page.locator('.memory-observation').textContent(), /batch of 5 tab attempts/);
        assert.match(await page.locator('.memory-observation').textContent(), /\+1 MiB/);
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
        assert.equal(overflow, false, `page overflow at ${width}px`);
        const box = await page.locator('.memory-detail').boundingBox();
        assert.ok(box.x >= 0 && box.x + box.width <= width, 'memory breakdown stays in viewport');
        await page.screenshot({path:`/private/tmp/autotrim-memory-${width}.png`});
        await page.locator('#memory-summary').press('Escape');
        assert.equal(await page.locator('#memory-details').getAttribute('open'), null);
      }
    });
    for (const colorScheme of ['light','dark']) {
      await page.emulateMedia({colorScheme});
      await page.setViewportSize({width:760,height:740});
      await page.evaluate(()=>{state.settings.auto_tab_domains=[];state.settings.auto_tab_inactive_hours=24;state.view='settings';renderAll(true);window.__bridgeCalls=[];});
      await page.locator('[data-keep-open="cleanup-options"] > summary').click();
      await check(`${colorScheme} suggestions filter and add an exact domain by keyboard`,async()=>{
        await page.evaluate(()=>{
          for (const tab of state.snap.browsers[0].tabs) tab.last_active=state.snap.taken_at-tab.idle_secs;
        });
        await page.locator('[data-add-domain]').click();
        try {
          const suggestions=page.locator('[data-domain-suggestion]');
          assert.equal(await suggestions.count(),2);
          assert.equal(await suggestions.first().getAttribute('data-domain-suggestion'),'www.example.com');
          assert.match(await suggestions.first().textContent(),/2 tabs.*1 inactive/);
          assert.equal(await page.locator('#domain-input').evaluate(el=>el===document.activeElement),true);
          const description=await suggestions.first().getAttribute('aria-describedby');
          assert.match(await page.locator('#'+description).textContent(),/2 tabs.*1 inactive/);
          await page.locator('#domain-editor').screenshot({path:`/private/tmp/autotrim-domain-recommendations-${colorScheme}.png`});
          await page.locator('#domain-input').fill('SHOP');
          assert.equal(await suggestions.count(),1);
          assert.match(await suggestions.textContent(),/shop\.example\.com/);
          await page.locator('#domain-input').fill('www');
          await suggestions.focus();
          await page.keyboard.press('Enter');
          assert.equal(await page.locator('#domain-input').inputValue(),'www.example.com');
          assert.equal(await page.locator('#domain-subdomains').isChecked(),false);
          assert.equal(await page.evaluate(()=>window.__bridgeCalls.length),0,'choosing a suggestion only fills the draft');
          await page.evaluate(()=>refresh());
          assert.equal(await page.locator('#domain-input').inputValue(),'www.example.com');
          await page.locator('#domain-submit').click();
          const writes=await page.evaluate(()=>window.__bridgeCalls.filter(([command])=>command==='set_tab_rules'));
          assert.deepEqual(writes[0][1].domains,[{domain:'www.example.com',include_subdomains:false}]);
          assert.equal(await page.evaluate(()=>window.__bridgeCalls.some(([command])=>command==='set_auto'||command==='close_tabs')),false);
          await page.locator('[data-add-domain]').click();
          assert.equal(await page.locator('[data-domain-suggestion="www.example.com"]').count(),0);
          await page.locator('#domain-input').fill('custom.example.com');
          assert.equal(await suggestions.count(),0);
          assert.match(await page.locator('#domain-suggestions').textContent(),/enter.*domain|add.*domain/i);
          await page.locator('#domain-input').fill('');
          assert.equal(await suggestions.count(),1);
        } finally {
          await page.locator('#domain-cancel').click();
          await page.evaluate(()=>{state.settings.auto_tab_domains=[];renderAll(true);window.__bridgeCalls=[];});
        }
      });
      await check(`${colorScheme} domain settings fit 760px and add by keyboard`,async()=>{
        await page.locator('[data-keep-open="cleanup-options"] > summary').click();
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth),true);
        assert.equal(await page.locator('[data-auto-domain-target]').locator('xpath=preceding-sibling::span/small').evaluate(el=>getComputedStyle(el).display),'block');
        await page.locator('[data-add-domain]').click();
        await page.locator('#domain-input').fill('WWW.Example.com.');
        await page.keyboard.press('Enter');
        await page.locator('.domain-row').waitFor();
        assert.match(await page.locator('.domain-row').textContent(),/www\.example\.com/);
        const calls=await page.evaluate(()=>window.__bridgeCalls.map(call=>call[0]));
        assert.equal(calls.includes('set_tab_rules'),true);
        assert.equal(calls.includes('set_auto'),false);
      });
      await check(`${colorScheme} long suggestion lists fit narrow dialogs and empty lists allow manual entry`,async()=>{
        const originalTabs=await page.evaluate(()=>state.snap.browsers[0].tabs);
        try {
          await page.evaluate(()=>{
            const hosts=['github.com','stackoverflow.com','news.ycombinator.com','docs.google.com','reddit.com','youtube.com',`${'long-domain-'.repeat(4)}example.example.com`];
            state.snap.browsers[0].tabs=hosts.map((host,i)=>({id:500+i,profile:'Default',url:'https://'+host,title:host,site:host,pinned:false,active:false,last_active:state.snap.taken_at-(i+1)*86400,idle_secs:(i+1)*86400}));
          });
          await page.locator('[data-add-domain]').click();
          for (const width of [760,420]) {
            await page.setViewportSize({width,height:740});
            assert.equal(await page.locator('#domain-editor').evaluate(el=>el.scrollWidth<=el.clientWidth),true);
            assert.equal(await page.locator('.domain-suggestion-list').evaluate(el=>el.scrollHeight>el.clientHeight),true);
            await page.locator('[data-domain-suggestion]').last().focus();
            await page.screenshot({path:`/private/tmp/autotrim-domain-suggestions-${colorScheme}-${width}.png`});
          }
          await page.keyboard.press('Enter');
          assert.equal(await page.locator('#domain-input').inputValue(),'github.com');
          await page.locator('#domain-cancel').click();
          await page.evaluate(()=>{state.snap.browsers[0].tabs=[];});
          await page.locator('[data-add-domain]').click();
          assert.equal(await page.locator('[data-domain-suggestion]').count(),0);
          assert.match(await page.locator('#domain-suggestions').textContent(),/enter a domain/i);
          await page.locator('#domain-input').fill('manual.example.com');
          assert.equal(await page.locator('#domain-submit').isEnabled(),true);
        } finally {
          await page.locator('#domain-cancel').click();
          await page.evaluate(tabs=>{state.snap.browsers[0].tabs=tabs;},originalTabs);
          await page.setViewportSize({width:760,height:740});
        }
      });
      await check(`${colorScheme} editor survives polling with draft and focus`,async()=>{
        await page.locator('[data-add-domain]').click();
        const input=page.locator('#domain-input');await input.fill('draft.example.com');await input.focus();
        await page.evaluate(()=>refresh());
        assert.equal(await input.inputValue(),'draft.example.com');
        assert.equal(await input.evaluate(el=>el===document.activeElement),true);
        await page.keyboard.press('Escape');
        assert.equal(await page.locator('#domain-editor').evaluate(el=>el.open),false);
      });
      await check(`${colorScheme} edits, previews statuses, and removes a rule`,async()=>{
        await page.locator('[data-edit-domain="0"]').click();
        await page.locator('#domain-subdomains').check();
        await page.keyboard.press('Enter');
        assert.match(await page.locator('.domain-row').textContent(),/Includes subdomains/);
        await page.locator('[data-review-domains]').click();
        await page.locator('.preview-table tbody tr').first().waitFor();
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=document.documentElement.clientWidth),true);
        const preview=await page.locator('#tab-preview').textContent();
        for(const label of ['Eligible','Pinned','Selected','Unknown activity']) assert.match(preview,new RegExp(label));
        assert.equal(await page.locator('#tab-preview').getByText(/Close/).count(),0);
        await page.keyboard.press('Escape');
        await page.locator('[data-remove-domain="0"]').click();
        await page.locator('.domain-empty').waitFor();
      });
      await check(`${colorScheme} Chrome Sites offers counted host choices and reopens an exact rule`,async()=>{
        await page.locator('[data-add-domain]').click();await page.locator('#domain-input').fill('www.example.com');await page.keyboard.press('Enter');
        await go('g:Chrome');
        if (await page.locator('.browser-details').getAttribute('open') == null) await page.locator('.browser-details > summary').click();
        const siteRow=page.getByRole('region', { name: 'Sites in these results', exact: true }).locator('tbody tr').first();
        if (await siteRow.locator('.row-title').getAttribute('aria-expanded') !== 'true') await siteRow.locator('.row-title').click();
        await page.locator('[data-auto-domain-site]').click();
        assert.equal(await page.locator('#domain-editor-title').textContent(),'Edit auto-close domain');
        const choices=page.locator('#domain-host-choice option');
        assert.equal(await choices.count(),2);
        assert.match(await choices.first().textContent(),/www\.example\.com \(2 tabs\)/);
        await page.locator('#domain-host-choice').selectOption('shop.example.com');
        assert.equal(await page.locator('#domain-editor-title').textContent(),'Add auto-close domain');
        assert.equal(await page.locator('#domain-input').inputValue(),'shop.example.com');
        await page.locator('#domain-cancel').click();
      });
    }
    await check('editor refuses a stale save after polling replaces and reorders rules',async()=>{
      await go('settings');
      await page.locator('[data-keep-open="cleanup-options"] > summary').click();
      await page.locator('[data-edit-domain="0"]').click();
      await page.locator('#domain-input').fill('renamed.example.com');
      await page.evaluate(()=>{
        window.__bridgeCalls=[];
        state.settings={...state.settings,tab_rules_revision:'external-edit',auto_tab_domains:[
          {domain:'other.example.com',include_subdomains:false},
          {domain:'www.example.com',include_subdomains:false},
        ]};
        renderAll(false);
      });
      await page.keyboard.press('Enter');
      assert.equal(await page.locator('#domain-editor').evaluate(el=>el.open),true);
      assert.equal(await page.locator('#domain-input').inputValue(),'renamed.example.com');
      assert.match(await page.locator('#domain-error').textContent(),/changed outside this editor/i);
      assert.equal(await page.evaluate(()=>window.__bridgeCalls.some(call=>call[0]==='set_tab_rules')),false);
      await page.locator('#domain-cancel').click();
    });
    console.log(`${passed} passed; ${failures.length} failed`);assert.deepEqual(failures,[]);
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
