#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, mkdirSync, readdirSync, renameSync, rmSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const captureFiles = [
  'dashboard-overview.png', 'repositories.png', 'repository-detail.png', 'analytics.png', 'pull-requests.png',
  'issues.png', 'kanban-board.png', 'settings.png', 'menu-bar-combined.png', 'menu-bar-total-lines.png',
  'menu-bar-source-lines.png', 'menu-bar-test-lines.png', 'menu-bar-open-prs.png', 'menu-bar-open-issues.png',
];
const help = `Generate CodeTally screenshots from deterministic fictional data.

Usage:
  node scripts/generate-screenshots.mjs [--output-dir PATH] [--build-dir PATH]

Options:
  --output-dir PATH  Save PNGs here (default: docs/screenshots).
  --build-dir PATH   Keep and reuse a Tauri Cargo build directory.
  --help             Show this help.

The normal run makes a fresh app bundle with a separate bundle identifier, seeds
a disposable SQLite database under the system temp directory, launches the app
with screenshot mode enabled, and captures the real Tauri window and native
menu-bar items. It writes dashboard, repository, analytics, PR, issue, Kanban,
settings, combined-menu, and separate-metric screenshots. It does not use the
installed CodeTally app database or invoke gh, git, synchronization, or updater
checks. Cargo is run offline. The isolated app process is stopped on completion.

On macOS, grant Accessibility and Screen Recording access to Terminal (or the
host terminal app) before running. The tool checks both permissions before
building. If automation stops after the build, it retains the isolated app and
fixture databases; launch the combined fixture manually with:

  swift scripts/capture-screenshot-fixtures.swift --launch-app "$APP_BUNDLE" --database "$FIXTURE_DB"

Then use macOS screenshots, or grant the permissions and rerun. The existing
capture-screenshots.swift also accepts an --input PNG and --database fixture
path for manual cropping and capture.
`;

function parseArgs(args) {
  const options = { outputDir: join(root, 'docs', 'screenshots'), buildDir: null };
  while (args.length) {
    const arg = args.shift();
    if (arg === '--help' || arg === '-h') { options.help = true; continue; }
    if (arg === '--output-dir' && args.length) { options.outputDir = resolve(root, args.shift()); continue; }
    if (arg === '--build-dir' && args.length) { options.buildDir = resolve(root, args.shift()); continue; }
    throw new Error(`Unknown option or missing value: ${arg}`);
  }
  return options;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    env: options.env ?? process.env,
    encoding: 'utf8',
    stdio: options.stdio ?? 'pipe',
    maxBuffer: 8 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || '').trim().split('\n').slice(-8).join('\n');
    throw new Error(`${command} exited with status ${result.status}${detail ? `:\n${detail}` : ''}`);
  }
  return result.stdout ?? '';
}

function runTauriBuild(targetDir) {
  const cli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  if (!existsSync(cli)) throw new Error('Tauri CLI not found. Install the repository npm dependencies first.');
  const env = { ...process.env, CARGO_TARGET_DIR: targetDir, CARGO_NET_OFFLINE: 'true', VITE_CODETALLY_SCREENSHOT_MODE: '1' };
  run(process.execPath, [cli, 'build', '--debug', '--bundles', 'app', '--config', 'src-tauri/tauri.screenshots.conf.json', '--', '--target-dir', targetDir], { env, stdio: 'inherit' });
}

function preflightCapturePermissions() {
  const helper = join(root, 'scripts', 'capture-screenshot-fixtures.swift');
  const output = run('swift', ['-module-cache-path', join(tmpdir(), 'codetally-screenshot-swift-cache'), helper, '--check-permissions']);
  const permissions = Object.fromEntries(output.trim().split('\n').map((line) => line.split('=')));
  const missing = ['accessibility', 'screen_recording'].filter((permission) => permissions[permission] !== 'granted');
  if (missing.length) {
    const labels = missing.map((permission) => permission === 'screen_recording' ? 'Screen Recording' : 'Accessibility');
    throw new Error(`Screenshot capture requires ${labels.join(' and ')} permission for Terminal (or the host terminal app). Enable them in System Settings → Privacy & Security, then rerun. No build or fixture data was created.`);
  }
}

function appBundle(targetDir) {
  const bundleRoot = join(targetDir, 'debug', 'bundle', 'macos');
  if (!existsSync(bundleRoot)) throw new Error(`Screenshot app bundle was not produced at ${bundleRoot}`);
  const appBundle = readdirSync(bundleRoot).find((name) => name.endsWith('.app'));
  if (!appBundle) throw new Error(`No .app bundle was produced in ${bundleRoot}`);
  const bundle = join(bundleRoot, appBundle);
  if (!existsSync(join(bundle, 'Contents', 'MacOS', 'codetally'))) throw new Error(`Screenshot app executable not found in ${bundle}`);
  return bundle;
}

function fixtureDatabase(targetDir, fixtureRoot, menuCombined) {
  const database = join(fixtureRoot, `codetally-${menuCombined ? 'combined' : 'separate'}.sqlite3`);
  const env = { ...process.env, CARGO_TARGET_DIR: targetDir, CARGO_NET_OFFLINE: 'true' };
  run('cargo', ['run', '--locked', '--offline', '--manifest-path', 'src-tauri/Cargo.toml', '--example', 'kanban_fixture', '--', database], { env, stdio: 'inherit' });
  const seed = String.raw`
import datetime, json, sqlite3, sys
path, combined = sys.argv[1], sys.argv[2] == "combined"
db = sqlite3.connect(path)
db.execute("PRAGMA foreign_keys=ON")
row = db.execute("SELECT value FROM app_metadata WHERE key='app_settings'").fetchone()
settings = json.loads(row[0])
settings.update({
    "theme_mode": "dark",
    "activity_relationship": "everyone",
    "run_in_background": False,
    "show_menu_bar": True,
    "kanban_enabled": True,
    "menu_bar_combined": combined,
    "menu_bar_metrics": ["total_lines", "source_lines", "test_lines", "open_prs", "open_issues"],
    "menu_bar_compact_metrics": [],
})
db.execute("UPDATE app_metadata SET value=? WHERE key='app_settings'", (json.dumps(settings, separators=(",", ":")),))
repos = db.execute("SELECT id, name_with_owner FROM repositories ORDER BY id").fetchall()
targets = [(18420, 14800), (24110, 19740), (11970, 9150)]
today = datetime.date(2026, 9, 27)
for (repo_id, _), (total_target, source_target) in zip(repos, targets):
    for offset in range(30):
        day = today - datetime.timedelta(days=29-offset)
        total = total_target - (29-offset)*18
        source = source_target - (29-offset)*13
        test = total - source
        stamp = day.isoformat() + "T09:30:00Z"
        sha = "fixture-%d-%s" % (repo_id, day.isoformat())
        db.execute("""INSERT INTO code_snapshots(repository_id,commit_sha,commit_date,snapshot_date,total_loc,source_loc,test_loc,created_at)
                      VALUES(?,?,?,?,?,?,?,?)""", (repo_id, sha, stamp, stamp, total, source, test, stamp))
db.commit()
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
db.close()
`;
  run('python3', ['-c', seed, database, menuCombined ? 'combined' : 'separate']);
  return database;
}

function launchApp(bundle, database, temporary) {
  const output = run('swift', ['-module-cache-path', join(temporary, 'swift-launch-cache'), 'scripts/capture-screenshot-fixtures.swift', '--launch-app', bundle, '--database', database]);
  const pid = Number(output.trim().split('\n').at(-1));
  if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error('macOS did not return the fixture app process ID.');
  return { pid };
}

function isRunning(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { return error.code === 'EPERM'; }
}

async function stopApp(child) {
  if (!child?.pid || !isRunning(child.pid)) return;
  try { process.kill(child.pid, 'SIGTERM'); } catch {}
  const deadline = Date.now() + 4000;
  while (isRunning(child.pid) && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 100));
  if (isRunning(child.pid)) { try { process.kill(child.pid, 'SIGKILL'); } catch {} }
}

function driveCapture(pid, outputDir, mode, temporary) {
  run('swift', ['-module-cache-path', join(temporary, 'swift-module-cache'), 'scripts/capture-screenshot-fixtures.swift', '--pid', String(pid), '--output-dir', outputDir, '--mode', mode], { stdio: 'inherit' });
}

function verifyCaptures(outputDir) {
  for (const filename of captureFiles) {
    const path = join(outputDir, filename);
    if (!existsSync(path)) throw new Error(`Expected screenshot was not created: ${path}`);
    const image = readFileSync(path);
    if (statSync(path).size < 512 || image.subarray(0, 8).toString('hex') !== '89504e470d0a1a0a') {
      throw new Error(`Screenshot is not a valid nonempty PNG: ${path}`);
    }
    const width = image.readUInt32BE(16);
    const height = image.readUInt32BE(20);
    if (width < 100 || height < 80) throw new Error(`Screenshot has an unexpectedly small canvas: ${path}`);
  }
  return captureFiles;
}

async function waitForApp(child) {
  await new Promise((resolve) => setTimeout(resolve, 1800));
  if (!isRunning(child.pid)) throw new Error('Screenshot app exited before capture.');
}

async function main() {
  if (process.platform !== 'darwin') throw new Error('Native Tauri and menu-bar captures require macOS.');
  const options = parseArgs(process.argv.slice(2));
  if (options.help) { process.stdout.write(help); return; }
  preflightCapturePermissions();
  mkdirSync(options.outputDir, { recursive: true });

  const temporary = mkdtempSync(join(tmpdir(), 'codetally-screenshot-fixtures-'));
  const targetDir = options.buildDir ?? join(temporary, 'target');
  const fixtureRoot = join(temporary, 'fixtures');
  const stagedOutput = join(temporary, 'screenshots');
  mkdirSync(fixtureRoot);
  mkdirSync(stagedOutput);
  mkdirSync(targetDir, { recursive: true });
  let app;
  let manualAppBundle;
  let manualDatabase;
  let completed = false;
  try {
    runTauriBuild(targetDir);
    const bundle = appBundle(targetDir);
    const combinedDb = fixtureDatabase(targetDir, fixtureRoot, true);
    manualAppBundle = bundle;
    manualDatabase = combinedDb;
    app = launchApp(bundle, combinedDb, temporary);
    await waitForApp(app);
    driveCapture(app.pid, stagedOutput, 'combined', temporary);
    await stopApp(app);

    const separateDb = fixtureDatabase(targetDir, fixtureRoot, false);
    app = launchApp(bundle, separateDb, temporary);
    await waitForApp(app);
    driveCapture(app.pid, stagedOutput, 'separate', temporary);
    await stopApp(app);
    const verifiedCaptures = verifyCaptures(stagedOutput);
    mkdirSync(options.outputDir, { recursive: true });
    for (const filename of verifiedCaptures) renameSync(join(stagedOutput, filename), join(options.outputDir, filename));
    writeFileSync(join(options.outputDir, 'screenshot-fixtures.json'), `${JSON.stringify({
      fixture: 'CodeTally fictional fixture data',
      captures: verifiedCaptures,
    }, null, 2)}\n`);
    console.log(`Fixture screenshots saved to ${options.outputDir}`);
    completed = true;
  } finally {
    await stopApp(app);
    if (completed) rmSync(temporary, { recursive: true, force: true });
    else {
      console.error(`Isolated fixture data retained at ${temporary}${options.buildDir ? `; reusable build retained at ${targetDir}` : ''} for manual capture or diagnosis.`);
      if (manualAppBundle && manualDatabase) {
        console.error(`Manual launch: swift scripts/capture-screenshot-fixtures.swift --launch-app '${manualAppBundle.replaceAll("'", "'\\''")}' --database '${manualDatabase.replaceAll("'", "'\\''")}'`);
      }
    }
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
