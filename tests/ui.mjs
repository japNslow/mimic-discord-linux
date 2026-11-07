// Run against npm run dev. Uses the shared runtime's Playwright (or PLAYWRIGHT_MODULE).
import {createRequire} from 'node:module';
import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
const require=createRequire(import.meta.url);
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || require.resolve('playwright'));
const browser=await chromium.launch({headless:true,executablePath:process.env.CHROMIUM_PATH});
const page=await browser.newPage({viewport:{width:1180,height:850},deviceScaleFactor:1});
const errors=[];
const external=[];
page.on('pageerror',e=>errors.push(e.message));
page.on('request',r=>{if(!r.url().startsWith('http://127.0.0.1:1420/') && !r.url().startsWith('data:'))external.push(r.url());});
await page.goto('http://127.0.0.1:1420');
await page.getByRole('heading',{name:'Твоя активность. Твой контроль.'}).waitFor();
assert(await page.getByRole('button',{name:'Запустить сессию',exact:true}).isDisabled());
await page.getByRole('button',{name:'Библиотека',exact:false}).click();
await page.getByRole('heading',{name:'Твоя библиотека начинается здесь'}).waitFor();
await page.getByRole('button',{name:'История',exact:true}).click();
await page.getByRole('heading',{name:'Пока чистый лист'}).waitFor();
await page.getByRole('button',{name:'Безопасность',exact:true}).click();
await page.getByRole('checkbox',{name:'Экспериментальный RPC'}).check();
await page.getByRole('button',{name:'Библиотека',exact:false}).click();
await page.getByRole('button',{name:'RPC · 0',exact:true}).waitFor();
await page.getByRole('button',{name:'Безопасность',exact:true}).click();
await page.getByRole('checkbox',{name:'Экспериментальный RPC'}).uncheck();
await page.getByRole('button',{name:'Активность',exact:true}).click();
await mkdir('docs',{recursive:true});
await page.screenshot({path:'docs/preview.png',fullPage:true});
for(const width of [860,650,390]){
 await page.setViewportSize({width,height:900});
 const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth);
 assert(!overflow,`horizontal overflow at ${width}px`);
}
assert.deepEqual(errors,[]);assert.deepEqual(external,[]);
console.log('UI: navigation, empty states, RPC opt-in, disabled preview controls, responsive layout and no external requests — passed.');

// Exercise desktop controls against a deterministic command bridge, without a Discord account.
const native=await browser.newPage({viewport:{width:1180,height:850}});
await native.addInitScript(()=>{
 const callbacks=new Map(),events=new Map();let next=1;
 const sample=[{id:'123',name:'Test Linux',method:'process',rel_path:'test-linux'},{id:'456',name:'Test RPC',method:'rpc',rel_path:null},{id:'789',name:'Overwatch',method:'process',rel_path:'overwatch.exe'}];
 window.testCommands=[];
 const emit=(event,payload)=>{for(const [id,fn]of events){if(fn.event===event)callbacks.get(fn.handler)?.({event,id,payload});}};
 window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_event,id)=>events.delete(id)};
 window.__TAURI_INTERNALS__={
  transformCallback:(fn)=>{const id=next++;callbacks.set(id,fn);return id;},
  invoke:async(cmd,args)=>{
   window.testCommands.push({cmd,args});
   if(cmd==='plugin:event|listen'){const id=next++;events.set(id,args);return id;}
   if(cmd==='plugin:event|unlisten'){events.delete(args.eventId);return;}
   if(cmd==='get_spoofer_state')return{status:'idle',history_count:0};
   if(cmd==='export_history')return'[]';
   if(cmd==='check_discord_status')return true;
   if(cmd==='fetch_games')return{from_cache:true};
   if(cmd==='get_games_list')return sample;
   const states={start_spoofer:'running',pause_spoofer:'paused',resume_spoofer:'running',stop_spoofer:'idle'};
   if(states[cmd]){emit('spoofer_status',{status:states[cmd]});return;}
   throw new Error(`Unexpected command ${cmd}`);
  }
 };
});
await native.goto('http://127.0.0.1:1420');
await native.getByRole('button',{name:'Загрузить базу игр'}).click();
await native.getByRole('button',{name:'Запустить сессию',exact:true}).click();
await native.getByRole('button',{name:'Пауза',exact:true}).waitFor();
await native.getByRole('button',{name:'Пауза',exact:true}).click();
await native.getByRole('button',{name:'Продолжить',exact:true}).click();
await native.getByRole('button',{name:'Остановить',exact:true}).click();
await native.getByRole('button',{name:'Запустить сессию',exact:true}).waitFor();
const commands=await native.evaluate(()=>window.testCommands);
const config=commands.find(c=>c.cmd==='start_spoofer').args.config;
assert.equal(config.beta_enabled,false);assert.equal(config.spoof_mode,'process_only');assert.equal(config.batch_size,1);
assert(commands.some(c=>c.cmd==='pause_spoofer'));assert(commands.some(c=>c.cmd==='resume_spoofer'));assert(commands.some(c=>c.cmd==='stop_spoofer'));
await native.getByRole('button',{name:'Библиотека',exact:false}).click();
await native.getByRole('button',{name:'Процессы · 2',exact:true}).waitFor();
await native.getByRole('button',{name:'Выбрать Test Linux'}).click();
await native.getByRole('button',{name:'Выбрать Overwatch'}).click();
await native.getByRole('button',{name:'Перейти к сессии'}).click();
await native.getByRole('button',{name:'Запустить сессию',exact:true}).click();
const farmCommand=await native.evaluate(()=>window.testCommands.filter(c=>c.cmd==='start_spoofer').at(-1));
assert.deepEqual(farmCommand.args.config.farm_game_ids,['123','789']);
await native.getByRole('button',{name:'Остановить',exact:true}).click();
console.log('UI command bridge: native defaults, load/start/pause/resume/stop and fixed game selection — passed (mock backend).');
await browser.close();

