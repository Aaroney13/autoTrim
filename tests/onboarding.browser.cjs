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
      if (name === 'fixture.js') return route.fulfill({contentType:'text/javascript', body: fixture + `
        const originalInvoke = window.__TAURI__.core.invoke;
        const saved = JSON.parse(sessionStorage.getItem('setup-saved') || 'null');
        window.setupSettings = saved || { ...settings, onboarding_completed: false, focus_areas: ['browser','agent'], stale_after_secs:21600, notify:true };
        window.setupCalls = []; window.failSetup = false;
        window.__TAURI__.core.invoke = async (command, args) => {
          if (command === 'settings') return structuredClone(window.setupSettings);
          if (command === 'service_info') return { supported: !location.search.includes('unsupported'), installed:false, running:false };
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
    await page.goto('http://autotrim.test/');
    await page.locator('#onboarding[open]').waitFor();
    assert.match(await heading(), /little setup/);
    assert.equal(await page.getByRole('button', {name:'Continue free', exact:true}).count(), 1);
    assert.match(await page.locator('.setup-welcome').textContent(), /No account needed/);
    await page.locator('#onboarding').screenshot({path:'/private/tmp/autotrim-setup-welcome.png'});
    await page.getByRole('button', {name:'Sign up / log in', exact:true}).click();
    assert.match(await page.locator('#setup-account-status').textContent(), /coming soon/);
    assert.equal(await page.locator('#setup-account-status').isVisible(), true);
    assert.equal(await page.evaluate(() => setupCalls.length), 0);
    assert.match(await heading(), /little setup/);
    await page.locator('#setup-title').focus();
    await page.keyboard.press('Enter');
    assert.equal(await heading(), 'What brings you here?');
    await page.locator('#onboarding').screenshot({path:'/private/tmp/autotrim-setup-focus.png'});
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
    assert.deepEqual(saved, {focus_areas:['app'],stale_after_hours:12,tab_stale_after_hours:48,notify:false,background:false,open_window_at_launch:false});
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
    await next(); await next();
    assert.equal(await page.locator('[name=startup][value=background]').isDisabled(),true);
    assert.equal(await page.locator('[name=startup][value=manual]').isChecked(),true);
    await page.locator('#setup-later').click();
    assert.equal(await page.evaluate(() => setupCalls.length),0);
    await page.reload();
    await page.locator('#onboarding[open]').waitFor();
    assert.match(await heading(), /little setup/);
    assert.deepEqual(errors, []);
    console.log('PASS: first run, optional account placeholder, Enter/button continue free, validation, back/refresh preservation, responsive themes, failed-save retry, persisted completion, sidebar priority, settings replay, unsupported startup, and deferred setup.');
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
