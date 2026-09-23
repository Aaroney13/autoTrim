// Synthetic tasks and IPC only. Never connect to Codex or close real work.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require('playwright');
(async () => {
  const browser = await chromium.launch({ executablePath: process.env.AUTOTRIM_TEST_CHROMIUM });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 820 } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', route => {
      const file = new URL(route.request().url()).pathname.slice(1) || 'index.html';
      if (file === 'main.js') return route.fulfill({ contentType: 'text/javascript', body: `
        import { state } from './state.js'; import { renderAll } from './render.js';
        Object.assign(window, { state, renderAll });` });
      if (!['index.html','styles.css','format.js','state.js','render.js','actions.js'].includes(file)) return route.abort();
      return route.fulfill({ body: fs.readFileSync(path.join(__dirname,'../tray/ui',file)), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://autotrim.test/');
    await page.evaluate(() => {
      const now = Math.floor(Date.now()/1000), GB = 1024**3;
      const session = {pid:42,start_time:10,kind:'codex',host:'Codex app',engine:true,state:'active',rss:GB,age_secs:1000,threads:[],pids:[42]};
      const task = {id:'idle',backend_pid:42,backend_created_at:10,codex_home:'/synthetic',revision:'a'.repeat(64),name:'Finished investigation',cwd:'/project',transcript:'/synthetic/task',updated_at:now-9000,state:'idle',protection:null};
      state.snap = {taken_at:now, system:{total_mem:32*GB,used_mem:16*GB,used_swap:0,total_swap:GB,uptime_secs:1000,cpu_pct:5,load_one:1,load_five:1,load_fifteen:1},
        groups:[{name:'Codex',kind:'agent',rss:GB,cpu:1,procs:2,pids:[42]}],sessions:[session],browsers:[],ports:[],trends:[],advice:[],
        codex_backends:[{pid:42,error:null,tasks:[task,{...task,id:'active',name:'Current task',state:'active',protection:'Active work or unknown activity'}]}]};
      state.src = {daemon_running:true,interval_secs:30,snapshot_taken_at:now}; state.log = []; state.view = 'g:Codex'; window.calls=[];
      window.__TAURI__ = {core:{invoke:async (command,args) => {
        calls.push({command,args});
        if(command==='archive_codex_task') {
          state.snap = {...state.snap,taken_at:state.snap.taken_at+1,codex_backends:[{pid:42,error:null,tasks:state.snap.codex_backends[0].tasks.filter(t=>t.id!==args.task.id)}]};
          const record={id:'record-1',action:'archive_codex_task',mode:'manual',status:'success',result:'archived and verified unloaded',target:args.task.name,ts:now,rss:0,resume:'Restore from archived tasks.'};
          state.log.push(record);return record;
        }
        if(command==='restore_codex_task') return {status:'success',result:'restored'};
        return {snapshot:state.snap,source_info:state.src,action_log:state.log,settings:null,service_info:null,update_status:null,app_icons:{}}[command];
      }}};
      renderAll(true);
    });
    assert.equal(await page.locator('[data-review-codex="active"]').count(),0);
    await page.locator('.row-title', {hasText:'Finished investigation'}).click();
    await page.screenshot({path:'/tmp/autotrim-codex-task-controls.png'});
    await page.locator('.work-detail [data-review-codex="idle"]').click();
    await page.locator('#action-review').waitFor({state:'visible'});
    assert.match(await page.locator('#review-title').textContent(),/Archive/);
    assert.equal(await page.evaluate(()=>calls.filter(c=>c.command==='archive_codex_task').length),0);
    await page.locator('#review-submit').click();
    await page.waitForFunction(()=>document.getElementById('review-status').textContent.includes('1 of 1 task archived'));
    assert.equal(await page.evaluate(()=>calls.filter(c=>c.command==='archive_codex_task').length),1);
    assert.equal(await page.evaluate(()=>calls.filter(c=>c.command==='close_session').length),0);
    await page.locator('#review-submit').click();
    await page.locator('[data-action-details]').first().click();
    await page.locator('[data-restore-codex]').click();
    assert.equal(await page.evaluate(()=>calls.filter(c=>c.command==='restore_codex_task').length),0);
    await page.locator('[data-restore-codex]').click();
    await page.waitForFunction(()=>calls.some(c=>c.command==='restore_codex_task'));
    assert.deepEqual(await page.evaluate(()=>calls.find(c=>c.command==='restore_codex_task').args),{actionId:'record-1'});
    assert.deepEqual(errors,[]);
    console.log('Codex task archive review, backend preservation, and restore controls passed');
  } finally { await browser.close(); }
})().catch(e=>{ console.error(e); process.exitCode=1; });
