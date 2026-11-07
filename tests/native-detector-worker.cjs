// Optional, isolated diagnostic. This is never loaded by the application.
// The Python supervisor terminates this worker after reading the result so a
// Discord/Electron addon is never unloaded inside an incompatible Node runtime.
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const [modulePath, catalogPath, helperPath, workDir, resultPath] = process.argv.slice(2);
const apps = JSON.parse(fs.readFileSync(catalogPath, 'utf8'));
const names = ['Factorio', 'Overwatch', 'ELDEN RING', 'Rocket League'];
const cases = names.map(name => {
  const game = apps.find(g => g.name === name);
  if (!game) throw new Error(`Catalog does not contain ${name}`);
  const exe = game.executables.find(e => e.os === 'win32' && !e.is_launcher && !e.arguments)
    ?? game.executables.find(e => e.os === 'linux' && !e.is_launcher && !e.arguments);
  if (!exe || exe.name.startsWith('/') || exe.name.includes('\\') || exe.name.includes(':')
      || exe.name.split('/').some(v => !v || v === '.' || v === '..')) {
    throw new Error('Unsafe or unsupported executable rule');
  }
  return { name, applicationId: game.id, executable: exe.name };
});
const children = cases.map(sample => {
  const target = path.join(workDir, sample.executable);
  fs.mkdirSync(path.dirname(target), { recursive: true, mode: 0o700 });
  fs.copyFileSync(helperPath, target, fs.constants.COPYFILE_EXCL);
  fs.chmodSync(target, 0o700);
  const child = spawn(target, [String(process.pid)], { env: {}, stdio: 'ignore' });
  return { child, ...sample };
});
// Reproduce the executable-rule projection in Discord's public web bundle:
// all OS names are supplied, grouped by command-line rule, with numeric IDs.
const ids = new Map();
const rules = [];
for (const game of apps) {
  const groups = new Map();
  for (const exe of game.executables ?? []) {
    const argument = exe.arguments || '';
    if (!groups.has(argument)) groups.set(argument, []);
    groups.get(argument).push(exe.name);
  }
  for (const [cmdline, executables] of groups) {
    const id = rules.length + 1;
    ids.set(id, game.id);
    rules.push({ id, name: game.name, executables, cmdline, thirdPartySkus: [] });
  }
}
const seen = new Map();
const native = require(modulePath);
native.init();
native.setProcessObserverUserId('1'); // Local observer seed only, never an account login.
native.setObservedGamesCallback2(rules, processes => {
  for (const p of processes) {
    const ours = children.find(c => c.child.pid === Number(p.pid));
    if (ours) seen.set(ours.name, ids.get(p.id) ?? null);
  }
});
// The observer skips polling until the error callback is registered.
native.setGameDetectionCallback(() => {});
native.setGameDetectionErrorCallback(() => {});
setTimeout(() => {
  const samples = children.map(c => ({ name: c.name, executable: c.executable,
    expectedApplicationId: c.applicationId, detectedApplicationId: seen.get(c.name) ?? null,
    matched: seen.get(c.name) === c.applicationId }));
  for (const { child } of children) child.kill();
  const result = { ruleCount: rules.length, rpcUsed: false, wineUsed: false,
    windowsCreated: false, serverCreditTested: false, samples };
  fs.writeFileSync(`${resultPath}.tmp`, JSON.stringify(result), { mode: 0o600 });
  fs.renameSync(`${resultPath}.tmp`, resultPath);
}, 11000);
setInterval(() => {}, 1000);
