import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, renameSync, rmSync } from 'node:fs';
import { spawn, spawnSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
function gitPath(cwd, arg) {
  const result = spawnSync('git', ['-C', cwd, 'rev-parse', arg], { encoding: 'utf8' });
  return result.status === 0 ? resolve(cwd, result.stdout.trim()) : null;
}
const scriptCommon = gitPath(scriptRoot, '--git-common-dir');
const activeRoot = gitPath(process.cwd(), '--show-toplevel');
const repoRoot = activeRoot && gitPath(activeRoot, '--git-common-dir') === scriptCommon ? activeRoot : scriptRoot;
const primaryRoot = scriptCommon && scriptCommon.endsWith('/.git') ? dirname(scriptCommon) : scriptRoot;
const [command, ...args] = process.argv.slice(2);
const managed = ['build', 'dev'].includes(command) && !args.some(arg => ['--help', '-h', '--version', '-V'].includes(arg));
const keep = process.env.CODETALLY_KEEP_BUILD_ARTIFACTS === '1' || process.env.CI === 'true' || process.env.GITHUB_ACTIONS === 'true';
if (managed && !keep && args.some(arg => arg === '--target-dir' || arg.startsWith('--target-dir='))) {
  console.error('Use CODETALLY_KEEP_BUILD_ARTIFACTS=1 with an explicit Cargo --target-dir.');
  process.exit(2);
}
const targetDir = managed && !keep ? mkdtempSync(join(tmpdir(), 'codetally-cargo-')) : null;
const env = targetDir ? { ...process.env, CARGO_TARGET_DIR: targetDir, CARGO_BUILD_TARGET_DIR: targetDir, CARGO_BUILD_BUILD_DIR: targetDir } : process.env;
let interrupted = 0;
let child;
const handlers = new Map();
for (const [signal, code] of [['SIGINT', 130], ['SIGTERM', 143]]) {
  const handler = () => {
    interrupted = code;
    if (child?.pid) {
      try {
        if (process.platform === 'win32') child.kill(signal);
        else process.kill(-child.pid, signal);
      } catch (error) {
        if (error.code !== 'ESRCH') console.error(error.message);
      }
    }
  };
  handlers.set(signal, handler);
  process.on(signal, handler);
}

function profiles() {
  const roots = [targetDir];
  for (const entry of readdirSync(targetDir, { withFileTypes: true })) {
    if (entry.isDirectory() && entry.name !== 'debug' && entry.name !== 'release') roots.push(join(targetDir, entry.name));
  }
  const outputs = [];
  for (const root of roots) {
    for (const entry of readdirSync(root, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(root, entry.name);
      if (existsSync(join(path, 'bundle')) || existsSync(join(path, 'codetally')) || existsSync(join(path, 'codetally.exe'))) {
        outputs.push({ path, label: `${root === targetDir ? 'native' : root.slice(targetDir.length + 1)}-${entry.name}` });
      }
    }
  }
  return outputs;
}

async function preserveOutputs() {
  const outputs = profiles();
  if (!outputs.length) throw new Error('Successful build produced no app, installer or executable to preserve');
  const outputRoot = join(primaryRoot, 'artifacts', 'tauri');
  mkdirSync(outputRoot, { recursive: true });
  const lock = join(outputRoot, '.publish.lock');
  let acquired = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    try { mkdirSync(lock); acquired = true; break; }
    catch (error) {
      if (error.code !== 'EEXIST') throw error;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
  if (!acquired) throw new Error(`Another publication holds ${lock}; compiler output retained for recovery`);
  try {
    for (const { path, label } of outputs) {
      const destination = join(outputRoot, label);
      const stage = join(outputRoot, `.${randomUUID()}.partial`);
      const backup = join(outputRoot, `.${randomUUID()}.previous`);
      mkdirSync(stage);
      try {
        if (existsSync(join(path, 'bundle'))) cpSync(join(path, 'bundle'), join(stage, 'bundle'), { recursive: true, verbatimSymlinks: true });
        for (const binary of ['codetally', 'codetally.exe']) {
          if (existsSync(join(path, binary))) cpSync(join(path, binary), join(stage, binary));
        }
        if (existsSync(destination)) renameSync(destination, backup);
        try { renameSync(stage, destination); }
        catch (error) { if (existsSync(backup)) renameSync(backup, destination); throw error; }
        rmSync(backup, { recursive: true, force: true });
        console.log(`Preserved desktop output in ${destination}`);
      } finally { rmSync(stage, { recursive: true, force: true }); }
    }
  } finally { rmSync(lock, { recursive: true, force: true }); }
}

let status = 1;
let cleanupAllowed = true;
try {
  const binary = process.env.CODETALLY_TAURI_BIN || process.execPath;
  const cli = process.env.CODETALLY_TAURI_BIN ? [] : [join(repoRoot, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')];
  status = await new Promise(resolve => {
    child = spawn(binary, [...cli, command, ...args].filter(arg => arg !== undefined), {
      cwd: repoRoot, env, stdio: 'inherit', detached: process.platform !== 'win32',
      shell: process.platform === 'win32' && binary === 'tauri',
    });
    child.once('error', error => { console.error(error.message); resolve(1); });
    child.once('exit', (code, signal) => resolve(code ?? (signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 1)));
  });
  status = interrupted || status;
  if (command === 'build' && status === 0 && targetDir) {
    try { await preserveOutputs(); }
    catch (error) { cleanupAllowed = false; status = 1; console.error(`${error.message}; output remains at ${targetDir}`); }
  }
} finally {
  status = interrupted || status;
  for (const [signal, handler] of handlers) process.off(signal, handler);
  if (targetDir && cleanupAllowed) {
    rmSync(targetDir, { recursive: true, force: true });
    console.log('Removed this run’s Cargo compiler output');
  }
}
process.exitCode = status;
