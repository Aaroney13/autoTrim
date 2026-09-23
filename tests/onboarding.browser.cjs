// Run with node tests/onboarding.browser.cjs (Playwright + Chromium).
// All IPC and network requests are fixtures; no config or login service is touched.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const uiDir = path.join(__dirname, '../tray/ui');
const fixture = fs.readFileSync(path.join(__dirname, 'tray-fixture.js'), 'utf8');
(async () => {
  const browser = await chromium.launch({ executablePath: process.env.AUTOTRIM_TEST_CHROMIUM });
  try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 740 } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', route => {
      const name = new URL(route.request().url()).pathname.slice(1) || 'index.html';
      if (name === 'logo.png') return route.fulfill({path:path.join(__dirname, '../tray/ui/logo.png'),contentType:'image/png'});
      if (/^brand\/[a-z-]+\.(png|svg)$/.test(name)) return route.fulfill({path:path.join(uiDir,name),contentType:name.endsWith('.svg')?'image/svg+xml':'image/png'});
      if (name === 'fixture.js') return route.fulfill({contentType:'text/javascript', body: fixture + `
        const originalInvoke = window.__TAURI__.core.invoke;
        const saved = JSON.parse(sessionStorage.getItem('setup-saved') || 'null');
        window.setupSettings = saved || { ...settings, onboarding_completed: false, focus_areas: ['browser','agent'], stale_after_secs:21600, notify:true };
        window.setupCalls = []; window.failSetup = false;
        window.scanCalls = 0; window.failScan = false; window.scanDelay = 0;
        window.suggestions = [
          {domain:'news.example.com', tabs:[{title:'An article <with markup>',profile:'Personal',idle_secs:259200},{title:'Another article',profile:'Work',idle_secs:172800}]},
          {domain:'shop.example.com', tabs:[{title:'Saved product',profile:'Personal',idle_secs:90000}]}
        ];
        window.__TAURI__.core.invoke = async (command, args) => {
          if (command === 'settings') return structuredClone(window.setupSettings);
          if (command === 'service_info') return { supported: !location.search.includes('unsupported'), installed:false, running:false };
          if (command === 'suggest_tab_rules') {
            window.scanCalls++;
            await new Promise(resolve => setTimeout(resolve, window.scanDelay));
            if (window.failScan) throw new Error('Chrome session file is unreadable');
            return structuredClone(window.suggestions);
          }
          if (command === 'complete_onboarding') {
            window.setupCalls.push(structuredClone(args));
            await new Promise(resolve => setTimeout(resolve, 100));
            if (window.failSetup) throw new Error('Fixture service failure. Retry or choose Open manually.');
            window.setupSettings = {...window.setupSettings, ...args.choices, onboarding_completed:true,
              stale_after_secs:args.choices.stale_after_hours*3600, tab_stale_after_secs:args.choices.tab_stale_after_hours*3600};
            sessionStorage.setItem('setup-saved', JSON.stringify(window.setupSettings));
            return structuredClone(window.setupSettings);
          }
          return originalInvoke(command, args);
        };
      `});
      if (!/^[a-z-]+\.(html|css|js)$/.test(name)) return route.abort();
      const file = path.join(uiDir, name);
      if (!fs.existsSync(file)) return route.abort();
      let body = fs.readFileSync(file, 'utf8');
      if (name === 'index.html') body = body.replace('<script type="module"', '<script src="/fixture.js"></script><script type="module"');
      return route.fulfill({body,contentType:name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'});
    });
    const heading = () => page.locator('#setup-title').textContent();
    const next = () => page.locator('.setup-next').click();
    const captureCards = async name => {
      for (const width of [1000, 390, 320]) {
        await page.setViewportSize({width,height:740});
        assert.equal(await page.locator('#onboarding').evaluate(el => el.scrollWidth <= el.clientWidth), true);
        await page.locator('#onboarding').screenshot({path:`/private/tmp/autotrim-setup-${name}-${width}.png`,animations:'disabled'});
      }
      await page.setViewportSize({width:1000,height:740});
    };
    await page.goto('http://autotrim.test/');
    await page.locator('#onboarding[open]').waitFor();
    assert.match(await heading(), /little setup/);
    assert.equal(await page.getByRole('button', {name:'Continue free', exact:true}).count(), 1);
    assert.equal(await page.locator('.setup-welcome, .setup-mark').count(), 0);
    assert.equal(await page.locator('.setup-logo').evaluate(el => el.complete && el.naturalWidth > 0), true);
    assert.equal(await page.locator('.setup-progress').textContent(), '');
    assert.equal(await page.locator('.setup-progress [aria-current=step]').getAttribute('aria-label'), 'Step 1 of 5: Welcome');
    await captureCards('welcome');
    await page.getByRole('button', {name:'Sign up / sign in', exact:true}).click();
    assert.match(await page.locator('#setup-account-status').textContent(), /coming soon/);
    assert.equal(await page.locator('#setup-account-status').isVisible(), true);
    assert.equal(await page.evaluate(() => setupCalls.length), 0);
    assert.match(await heading(), /little setup/);
    await page.locator('#setup-title').focus();
    await page.keyboard.press('Enter');
    assert.equal(await heading(), 'What brings you here?');
    await page.waitForFunction(() => [...document.querySelectorAll('.setup-app-logos img')].every(el => el.complete && el.naturalWidth > 0));
    await captureCards('focus');
    await page.locator('[data-focus][value=browser]').uncheck();
    await page.locator('[data-focus][value=agent]').uncheck();
    await next();
    assert.match(await page.locator('#setup-error').textContent(), /at least one/);
    await page.locator('[data-focus][value=app]').check();
    await next();
    await page.locator('#onboarding').screenshot({path:'/private/tmp/autotrim-setup-preferences.png'});
    await page.getByLabel('AI session idle hours').fill('0');
    await next();
    assert.match(await heading(), /tidy up/);
    await page.getByLabel('AI session idle hours').fill('12');
    await page.getByLabel('Browser tab idle hours').fill('48');
    await page.locator('[data-bool=notify]').uncheck();
    await page.evaluate(async () => { const { refresh } = await import('/render.js'); await refresh(); });
    assert.equal(await page.getByLabel('AI session idle hours').inputValue(), '12');
    await next();
    await page.locator('[name=startup][value=manual]').check();
    await page.locator('[data-bool=open_window_at_launch]').uncheck();
    await page.locator('#setup-back').click();
    assert.equal(await page.getByLabel('Browser tab idle hours').inputValue(), '48');
    await next();
    assert.equal(await page.locator('[name=startup][value=manual]').isChecked(), true);
    assert.equal(await page.evaluate(() => setupCalls.length), 0);
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const size of [{width:1000,height:740},{width:760,height:500},{width:390,height:740}]) {
        await page.setViewportSize(size);
        assert.equal(await page.locator('#onboarding').evaluate(el => el.scrollWidth <= el.clientWidth), true);
        await page.locator('#onboarding').screenshot({path:`/private/tmp/autotrim-setup-${colorScheme}-${size.width}.png`});
      }
    }
    await page.setViewportSize({width:1000,height:740});
    await page.evaluate(() => { window.failSetup = true; });
    await next();
    await page.locator('#setup-error').filter({hasText:'Fixture service failure'}).waitFor();
    assert.equal(await page.locator('#onboarding').evaluate(el=>el.open),true);
    assert.equal(await page.evaluate(() => setupSettings.onboarding_completed),false);
    await page.evaluate(() => { window.failSetup = false; });
    await next();
    await page.locator('#onboarding[open]').waitFor({state:'hidden'});
    const saved = await page.evaluate(() => setupCalls.at(-1).choices);
    assert.deepEqual(saved, {focus_areas:['app'],stale_after_hours:12,tab_stale_after_hours:48,notify:false,background:false,open_window_at_launch:false,add_tab_domains:[],tab_rules_revision:'fixture-0'});
    await page.reload();
    await page.locator('.nav-holders').waitFor();
    assert.equal(await page.locator('#onboarding').evaluate(el=>el.open),false);
    assert.equal(await page.locator('.nav-holders .item .name').first().textContent(), 'Notes');
    await page.locator('.nav-primary [data-view=settings]').click();
    await page.evaluate(() => Object.assign(setupSettings, { auto_close_sessions:false, auto_stop_servers:false, auto_close_tabs:true }));
    await page.locator('[data-setup]').click();
    await page.locator('#onboarding[open]').waitFor();
    assert.equal(await heading(), 'What brings you here?');
    assert.equal(await page.locator('[data-focus][value=app]').isChecked(),true);
    await next();
    assert.match(await page.locator('#onboarding').textContent(), /existing automatic cleanup settings will stay enabled/);
    await page.locator('#setup-later').click();
    await page.evaluate(() => sessionStorage.clear());
    await page.goto('http://autotrim.test/?unsupported');
    await page.locator('#onboarding[open]').waitFor();
    await page.getByRole('button', {name:'Continue free', exact:true}).click();
    await next(); await next(); await next();
    assert.equal(await page.locator('[name=startup][value=background]').isDisabled(),true);
    assert.equal(await page.locator('[name=startup][value=manual]').isChecked(),true);
    await page.locator('#setup-later').click();
    assert.equal(await page.evaluate(() => setupCalls.length),0);
    await page.reload();
    await page.locator('#onboarding[open]').waitFor();
    assert.match(await heading(), /little setup/);
    // Suggestions stay local drafts until Finish; review back/skip/remove is reversible.
    await next(); await next(); await next();
    assert.match(await heading(), /Which Chrome tabs/);
    assert.equal(await page.evaluate(() => scanCalls), 0);
    assert.equal(await page.locator('[data-starter]:checked').count(), 0);
    assert.match(await page.locator('.setup-new-tabs').textContent(), /without waiting for the site inactivity timer/);
    await page.locator('#setup-review-tabs').click();
    await page.locator('#setup-domain-add').waitFor();
    assert.equal(await page.locator('.setup-domain-card h2').textContent(), 'news.example.com');
    assert.equal(await page.locator('.setup-tab-list li').count(), 2);
    assert.match(await page.locator('.setup-tab-list').textContent(), /<with markup>/);
    assert.equal(await page.locator('.setup-tab-list markup').count(), 0);
    assert.match(await page.locator('.setup-mode-note').textContent(), /stays off/);
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const width of [1000, 390]) {
        await page.setViewportSize({width, height:740});
        assert.equal(await page.locator('#onboarding').evaluate(el => el.scrollWidth <= el.clientWidth), true);
        assert.equal(await page.locator('.setup-next').evaluate(el => el.getBoundingClientRect().bottom <= innerHeight), true);
        await page.locator('#onboarding').screenshot({path:`/private/tmp/autotrim-setup-chrome-${colorScheme}-${width}.png`});
      }
    }
    await page.setViewportSize({width:1000, height:740});
    await page.emulateMedia({reducedMotion:'reduce'});
    await page.locator('#setup-domain-add').click();
    assert.equal(await page.locator('.setup-page').evaluate(el => getComputedStyle(el).animationName), 'none');
    await page.locator('#setup-domain-previous').click();
    assert.match(await page.locator('#setup-domain-add').textContent(), /Keep on whitelist/);
    await page.locator('#setup-domain-skip').click();
    assert.match(await page.locator('.setup-review-status').textContent(), /0 added/);
    await page.locator('#setup-domain-add').click();
    assert.match(await heading(), /ready to save/);
    await page.locator('[data-remove-domain]').click();
    assert.equal(await page.locator('[data-remove-domain]').count(), 0);
    await page.locator('#setup-domain-restart').click();
    await page.emulateMedia({reducedMotion:'no-preference'});
    await page.locator('#setup-domain-add').click();
    assert.equal(await page.locator('.setup-page').evaluate(el => getComputedStyle(el).animationName), 'setup-forward');
    await page.locator('#setup-domain-skip').click();
    assert.equal(await page.evaluate(() => setupCalls.length), 0);
    await next();
    assert.match(await page.locator('.setup-save-summary').textContent(), /1 domain/);
    await page.locator('#setup-back').click();
    assert.equal(await page.locator('[data-remove-domain]').count(), 1);
    await next(); await next();
    await page.locator('#onboarding[open]').waitFor({state:'hidden'});
    assert.deepEqual(await page.evaluate(() => setupCalls.at(-1).choices.add_tab_domains), ['news.example.com']);
    assert.equal(await page.evaluate(() => setupSettings.auto_close_tabs), false);

    // Empty/error scans can be retried, and leaving while a scan runs cannot replace Startup.
    await page.evaluate(() => { window.dispatchEvent(new Event('autotrim-setup')); });
    await page.locator('#onboarding[open]').waitFor();
    await page.evaluate(() => { window.failScan = true; });
    await next(); await next();
    await page.locator('#setup-review-tabs').click();
    await page.locator('#setup-scan-retry').waitFor();
    assert.match(await page.locator('.setup-scan').textContent(), /unreadable/);
    await page.evaluate(() => { window.failScan = false; window.suggestions = []; });
    await page.locator('#setup-scan-retry').click();
    await page.getByText('No new domains to suggest', {exact:true}).waitFor();
    await page.evaluate(() => { window.scanDelay = 300; });
    await page.locator('#setup-scan-retry').click();
    await next();
    await page.waitForTimeout(400);
    assert.match(await heading(), /When should autoTrim run/);
    await page.locator('#setup-later').click();
    assert.equal(await page.evaluate(() => setupCalls.length), 1);

    // Starter suggestions work without a scan, remain opt-in, and save exact hosts.
    await page.evaluate(() => {
      setupSettings.auto_tab_domains = [{domain:'reddit.com',include_subdomains:true}];
      window.dispatchEvent(new Event('autotrim-setup'));
    });
    await page.locator('#onboarding[open]').waitFor();
    await next(); await next();
    const scansBefore = await page.evaluate(() => scanCalls);
    assert.equal(await page.locator('[data-starter=reddit]').isChecked(), true);
    assert.equal(await page.locator('[data-starter=reddit]').isDisabled(), true);
    await page.locator('[data-starter=google]').check();
    await page.locator('[data-starter=instagram]').check();
    assert.match(await page.locator('#setup-starter-count').textContent(), /4 domains selected/);
    await page.locator('[data-starter=instagram]').uncheck();
    assert.match(await page.locator('#setup-starter-count').textContent(), /2 domains selected/);
    assert.match(await page.locator('.setup-starters').textContent(), /Gmail and Docs aren’t included/);
    assert.equal(await page.locator('.setup-starter').first().evaluate(el => getComputedStyle(el).display), 'flex');
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({colorScheme});
      for (const size of [{width:1000,height:740},{width:760,height:500},{width:390,height:740}]) {
        await page.setViewportSize(size);
        assert.equal(await page.locator('#onboarding').evaluate(el => el.scrollWidth <= el.clientWidth), true);
        assert.equal(await page.locator('.setup-next').evaluate(el => el.getBoundingClientRect().bottom <= innerHeight), true);
        await page.locator('#onboarding').screenshot({path:`/private/tmp/autotrim-setup-starters-${colorScheme}-${size.width}.png`});
      }
    }
    await page.setViewportSize({width:1000,height:740});
    await next();
    assert.match(await page.locator('.setup-save-summary').textContent(), /2 domains/);
    await page.locator('#setup-back').click();
    assert.equal(await page.locator('[data-starter=google]').isChecked(), true);
    assert.equal(await page.evaluate(() => scanCalls), scansBefore);
    assert.equal(await page.evaluate(() => setupCalls.length), 1);
    await next(); await next();
    await page.locator('#onboarding[open]').waitFor({state:'hidden'});
    assert.deepEqual(await page.evaluate(() => setupCalls.at(-1).choices.add_tab_domains), ['google.com','www.google.com']);
    assert.equal(await page.evaluate(() => setupSettings.auto_close_tabs), false);

    // Cancelling discards starter choices; a personalized decision can partially select a preset.
    await page.evaluate(() => {
      scanDelay = 0; suggestions = [{domain:'www.instagram.com',tabs:[{title:'A post',profile:'Personal',idle_secs:90000}]}];
      window.dispatchEvent(new Event('autotrim-setup'));
    });
    await page.locator('#onboarding[open]').waitFor();
    await next(); await next();
    await page.locator('#setup-review-tabs').click();
    await page.locator('#setup-domain-add').click();
    await page.locator('#setup-back').click();
    assert.equal(await page.locator('[data-starter=instagram]').evaluate(el => el.indeterminate), true);
    await page.locator('[data-starter=instagram]').check();
    assert.match(await page.locator('#setup-starter-count').textContent(), /2 domains selected/);
    await page.locator('#setup-later').click();
    await page.evaluate(() => { window.dispatchEvent(new Event('autotrim-setup')); });
    await page.locator('#onboarding[open]').waitFor();
    await next(); await next();
    assert.equal(await page.locator('[data-starter=instagram]').isChecked(), false);
    await page.locator('[data-starter=instagram]').check();
    await page.locator('#setup-back').click();
    await page.locator('#setup-back').click();
    await page.locator('[data-focus][value=browser]').uncheck();
    await next(); await next(); await next();
    await page.locator('#onboarding[open]').waitFor({state:'hidden'});
    assert.deepEqual(await page.evaluate(() => setupCalls.at(-1).choices.add_tab_domains), []);
    assert.deepEqual(errors, []);
    console.log('PASS: onboarding, validation, save retry, replay, responsive layout, Chrome starter choices and exact hosts, existing rules, partial selections, cancellation, focus changes, review/add/skip/back/remove, draft persistence, reduced motion, scan failure/retry, and late-response handling.');
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
