#!/usr/bin/env node
// Permission-free simulations of CodeTally menus, populated only from fixtures.
import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, renameSync, rmSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium } from 'playwright';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const options = { outputDir: join(root, 'docs/screenshots'), executablePath: undefined, port: 1421 };
const args = process.argv.slice(2);
while (args.length) {
  const flag = args.shift();
  if (flag === '--help' || flag === '-h') {
    console.log(`Generate native-style CodeTally menu simulations from fictional data.

Usage: npm run screenshots:menus -- [options]
  --output-dir PATH          Save the four PNGs here (default: docs/screenshots).
  --browser-executable PATH  Use an existing Chromium executable.
  --port NUMBER              Local fixture server port (default: 1421).

Install the browser once with: npx playwright install chromium --only-shell
The tool starts an isolated fixture frontend and stops it after capture. It
reuses the app's combined-menu preview and native menu labels/grouping for PR
and issue examples. These are browser-rendered simulations, not macOS captures.
No GitHub account, local app database, or screen-recording permission is used.`);
    process.exit(0);
  }
  const value = args.shift();
  if (!value) throw new Error(`Missing value for ${flag}`);
  if (flag === '--output-dir') options.outputDir = resolve(root, value);
  else if (flag === '--browser-executable') options.executablePath = resolve(value);
  else if (flag === '--port' && /^\d+$/.test(value) && Number(value) > 0 && Number(value) < 65536) options.port = Number(value);
  else throw new Error(`Unknown option or invalid value: ${flag}`);
}

const origin = `http://127.0.0.1:${options.port}`;
// Render the actual native template bitmaps instead of approximating the icons.
const nativeSource = readFileSync(join(root, 'src-tauri/src/native.rs'), 'utf8');
const icons = Object.fromEntries(['TotalLines', 'OpenPrs', 'OpenIssues'].map(metric => {
  const block = nativeSource.match(new RegExp(`MenuBarMetric::${metric} => \\[([^\\]]+)\\]`));
  const rows = [...(block?.[1] ?? '').matchAll(/"([.#]{16})"/g)].map(match => match[1]);
  if (rows.length !== 16) throw new Error(`Cannot read native ${metric} icon bitmap.`);
  const pixels = rows.flatMap((row, y) => [...row].flatMap((pixel, x) => pixel === '#' ? [`<rect x="${x + 1}" y="${y + 1}" width="1" height="1"/>`] : []));
  return [metric, `<svg width="18" height="18" viewBox="0 0 18 18" fill="currentColor" aria-hidden="true">${pixels.join('')}</svg>`];
}));
const captures = [
  ['summary', 'light', 'menu-bar-summary-light.png'],
  ['summary', 'dark', 'menu-bar-summary-dark.png'],
  ['prs', 'light', 'menu-pull-requests.png'],
  ['issues', 'light', 'menu-issues.png'],
];
const temporary = mkdtempSync(join(tmpdir(), 'codetally-menu-simulations-'));
let server;
let browser;
let logs = '';

try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(options.port), '--strictPort'], {
    cwd: root,
    env: { ...process.env, VITE_CODETALLY_SCREENSHOT_MODE: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  server.stdout.on('data', data => { logs = (logs + data).slice(-4000); });
  server.stderr.on('data', data => { logs = (logs + data).slice(-4000); });
  let startError;
  server.on('error', error => { startError = error; });
  // Wait for this process to report its own ready URL before making requests;
  // an occupied port must not accidentally capture another running app.
  const deadline = Date.now() + 20_000;
  while (!logs.includes(origin)) {
    if (startError || server.exitCode !== null || Date.now() > deadline) throw new Error(`Fixture server failed to start: ${startError?.message ?? logs}`);
    await delay(100);
  }
  browser = await chromium.launch({ executablePath: options.executablePath, headless: true });
  for (const [kind, theme, filename] of captures) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 1050 }, deviceScaleFactor: 2, colorScheme: theme, timezoneId: 'UTC' });
    try {
      await context.route('**/*', route => {
        const url = new URL(route.request().url());
        return url.origin === origin ? route.continue() : route.abort();
      });
      const page = await context.newPage();
      // Freeze relative dates and chart ranges while keeping real timer behavior.
      await page.clock.setFixedTime(new Date('2026-10-04T14:00:00Z'));
      await page.goto(origin, { waitUntil: 'networkidle' });
      await page.getByRole('heading', { name: 'Code at a glance' }).waitFor();
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await page.getByRole('radio', { name: theme === 'light' ? 'Light' : 'Dark', exact: true }).check();
      await page.getByRole('switch', { name: 'Combined menu bar item', exact: true }).check();
      await page.getByRole('region', { name: 'Combined menu bar preview' }).waitFor();
      const text = await page.evaluate(async ({ kind, theme, icons }) => {
        const { screenshotCall } = await import('/src/kanban/screenshotBackend.ts');
        let menu;
        if (kind === 'summary') {
          menu = document.querySelector('.native-menu-preview').cloneNode(true);
        } else {
          const { items } = await screenshotCall('get_activity_feed', { kind, state: 'open' });
          menu = document.createElement('section');
          menu.className = 'native-menu-preview';
          menu.setAttribute('aria-label', `${kind === 'prs' ? 'Pull request' : 'Issue'} menu simulation`);
          const add = (className, text) => {
            const row = document.createElement('div');
            row.className = className;
            row.textContent = text;
            menu.append(row);
          };
          // Match native.rs: repository groups keep the first-seen order and
          // item titles have a #number prefix with a bounded 74-character label.
          const groups = new Map();
          for (const item of items.slice(0, 25)) {
            if (!groups.has(item.repository)) groups.set(item.repository, []);
            groups.get(item.repository).push(item);
          }
          for (const [repository, tickets] of groups) {
            if (menu.childElementCount) add('native-menu-preview-separator', '');
            add('native-menu-preview-muted-row', repository);
            for (const ticket of tickets) {
              const prefix = `#${ticket.number} — `;
              const title = [...ticket.title];
              const remaining = Math.max(0, 74 - [...prefix].length);
              add('native-menu-preview-info-row', prefix + title.slice(0, remaining).join('') + (title.length > remaining ? '…' : ''));
            }
          }
          add('native-menu-preview-separator', '');
          const actions = document.createElement('div');
          actions.className = 'native-menu-preview-actions';
          for (const label of ['Open CodeTally', 'Settings…', 'Quit CodeTally']) {
            const action = document.createElement('div');
            action.textContent = label;
            actions.append(action);
          }
          menu.append(actions);
        }
        const stage = document.createElement('div');
        stage.className = `fixture-menu-stage ${theme}`;
        const strip = document.createElement('div');
        strip.className = 'fixture-menu-strip';
        const selected = document.createElement('span');
        selected.className = 'fixture-menu-selected';
        selected.innerHTML = icons[kind === 'prs' ? 'OpenPrs' : kind === 'issues' ? 'OpenIssues' : 'TotalLines'];
        if (kind !== 'summary') {
          // The label is populated from the same fixtures as the visible rows.
          const dashboard = await screenshotCall('get_dashboard');
          selected.append(document.createTextNode(` ${kind === 'prs' ? dashboard.totals.open_prs + ' PRs' : dashboard.totals.open_issues + ' issues'}`));
        }
        strip.append(selected);
        stage.append(strip, menu);
        document.body.append(stage);
        return menu.textContent;
      }, { kind, theme, icons });
      await page.addStyleTag({ content: `
        .fixture-menu-stage { position:fixed; top:0; left:0; z-index:2147483647; width:400px; padding:0 20px 20px; color:#252529; background:linear-gradient(130deg,#dce5ed,#edf0f5); font-family:-apple-system,BlinkMacSystemFont,sans-serif; }
        .fixture-menu-stage.dark { color:#eee; background:linear-gradient(130deg,#293441,#1c2430); }
        .fixture-menu-strip { margin:0 -20px; height:28px; padding:0 30px; display:flex; justify-content:flex-end; background:#f4f4f4df; box-shadow:0 1px 0 #0002; }
        .dark .fixture-menu-strip { background:#292b30e8; }
        .fixture-menu-selected { display:flex; align-items:center; gap:4px; padding:0 7px; font-size:12px; background:#0001; border-radius:4px; }
        .dark .fixture-menu-selected { background:#fff2; }
        .fixture-menu-stage .native-menu-preview { margin:6px 0 0; width:360px; }
        .fixture-menu-stage .native-menu-preview-actions { color:inherit; }
      ` });
      if (kind === 'summary' && !text.includes('54,500 total lines')) throw new Error('Fixture summary totals were not rendered.');
      if (kind !== 'summary' && !text.includes('example-team/')) throw new Error('Fixture ticket menu was not rendered.');
      // Round the capture bounds so fractional row heights cannot expose a
      // one-pixel sliver of the underlying fixture app at the bottom.
      await page.locator('.fixture-menu-stage').evaluate(stage => { stage.style.height = `${Math.ceil(stage.getBoundingClientRect().height)}px`; });
      await page.locator('.fixture-menu-stage').screenshot({ path: join(temporary, filename), animations: 'disabled' });
      console.log(`Generated ${filename}`);
    } finally { await context.close(); }
  }
  mkdirSync(options.outputDir, { recursive: true });
  for (const [, , filename] of captures) renameSync(join(temporary, filename), join(options.outputDir, filename));
  writeFileSync(join(options.outputDir, 'menu-simulations.json'), JSON.stringify({ kind: 'browser-rendered menu simulations', fixture: 'fictional CodeTally screenshot backend', captures: captures.map(([, , filename]) => filename) }, null, 2) + '\n');
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  await browser?.close();
  if (server && server.exitCode === null) {
    const stopped = new Promise(resolve => server.once('exit', resolve));
    server.kill('SIGTERM');
    await Promise.race([stopped, delay(3000)]);
    if (server.exitCode === null && server.signalCode === null) server.kill('SIGKILL');
  }
  rmSync(temporary, { recursive: true, force: true });
}
