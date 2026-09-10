import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const stableVersion = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const compare = (a, b) => {
  const left = a.split('.').map(Number);
  const right = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return left[i] - right[i];
  return 0;
};

export function setVersion(version, { read = (path) => readFileSync(path, 'utf8'), write = writeFileSync } = {}) {
  if (!stableVersion.test(version)) throw new Error('Invalid release version.');
  for (const path of ['package.json', 'package-lock.json', 'src-tauri/tauri.conf.json']) {
    const data = JSON.parse(read(path));
    data.version = version;
    if (path === 'package-lock.json') data.packages[''].version = version;
    write(path, `${JSON.stringify(data, null, 2)}\n`);
  }
  write('src-tauri/Cargo.toml', read('src-tauri/Cargo.toml').replace(/^version = "[^"]+"/m, `version = "${version}"`));
  write('src-tauri/Cargo.lock', read('src-tauri/Cargo.lock').replace(/(\[\[package\]\]\s+name = "codetally"\s+version = ")[^"]+"/, (_, prefix) => `${prefix}${version}"`));
}

export function preflight({ read = (path) => readFileSync(path, 'utf8'), run = (cmd, args) => execFileSync(cmd, args, { encoding: 'utf8' }).trim(), pr = false } = {}) {
  const pkg = JSON.parse(read('package.json'));
  const lock = JSON.parse(read('package-lock.json'));
  const tauri = JSON.parse(read('src-tauri/tauri.conf.json'));
  const cargo = read('src-tauri/Cargo.toml').match(/^version = "([^"]+)"/m)?.[1];
  const cargoLock = read('src-tauri/Cargo.lock').match(/\[\[package\]\]\s+name = "codetally"\s+version = "([^"]+)"/)?.[1];
  const versions = [pkg.version, lock.version, lock.packages?.['']?.version, tauri.version, cargo, cargoLock];
  let version = pkg.version;
  if (!stableVersion.test(version) || versions.some((value) => value !== version)) {
    throw new Error('All app and lockfile versions must match and use a stable major.minor.patch version.');
  }
  const parseTags = (value) => value.split('\n').filter((tag) => tag.startsWith('v') && stableVersion.test(tag.slice(1)));
  const tags = parseTags(run('git', ['tag', '--list', 'v*']));
  const publishedTags = pr ? [] : parseTags(run('git', ['tag', '--points-at', 'HEAD']));
  if (!publishedTags.length) {
    const latest = tags.map((tag) => tag.slice(1)).sort(compare).at(-1);
    if (latest && compare(version, latest) <= 0) {
      const [major, minor, patch] = latest.split('.').map(Number);
      version = `${major}.${minor}.${patch + 1}`;
    }
    return { version, tag: `v${version}`, publish: true };
  }
  const tag = publishedTags.sort((a, b) => compare(a.slice(1), b.slice(1))).at(-1);
  version = tag.slice(1);
  // A tag alone does not prove that publication and both uploads completed.
  const release = JSON.parse(run('gh', ['release', 'view', tag, '--json', 'isDraft,assets']));
  const dmgs = release.assets.filter((asset) => asset.name.endsWith('.dmg') && asset.size > 0);
  if (release.isDraft || !dmgs.some((asset) => asset.name.includes('aarch64')) || !dmgs.some((asset) => asset.name.includes('x64'))) {
    throw new Error(`${tag} exists but its published release is incomplete. Inspect the existing release before retrying.`);
  }
  return { version, tag, publish: false };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv[2] === '--set-version') {
      setVersion(process.env.RELEASE_VERSION);
      console.log(`Applied release version ${process.env.RELEASE_VERSION} to app manifests and lockfiles.`);
    } else {
      const result = preflight({ pr: process.env.GITHUB_EVENT_NAME === 'pull_request' });
      const message = result.publish ? `${result.tag} is available for release.` : `${result.tag} is already published for this commit; skipping builds and publication.`;
      console.log(message);
      if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(result).map(([key, value]) => `${key}=${value}\n`).join(''));
      if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${message}\n`);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
