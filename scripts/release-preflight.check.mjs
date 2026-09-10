import test from 'node:test';
import assert from 'node:assert/strict';
import { preflight, setVersion } from './release-preflight.mjs';

function fixture({ version = '0.1.1', tags = '', atHead = '', release } = {}) {
  const files = {
    'package.json': JSON.stringify({ version }),
    'package-lock.json': JSON.stringify({ version, packages: { '': { version }, dependency: { version: '3.0.0' } } }),
    'src-tauri/tauri.conf.json': JSON.stringify({ version }),
    'src-tauri/Cargo.toml': `[package]\nname = "codetally"\nversion = "${version}"\n`,
    'src-tauri/Cargo.lock': `[[package]]\nname = "codetally"\nversion = "${version}"\n\n[[package]]\nname = "other"\nversion = "9.0.0"\n`,
  };
  return {
    files,
    read: (path) => files[path],
    write: (path, value) => { files[path] = value; },
    run: (cmd, args) => {
      if (cmd === 'git' && args.join(' ') === 'tag --list v*') return tags;
      if (cmd === 'git' && args.join(' ') === 'tag --points-at HEAD') return atHead;
      if (cmd === 'gh' && release) return JSON.stringify(release);
      throw new Error(`Unexpected or failed command: ${cmd} ${args.join(' ')}`);
    },
  };
}

const complete = { isDraft: false, assets: [
  { name: 'CodeTally_0.1.2_aarch64.dmg', size: 42 },
  { name: 'CodeTally_0.1.2_x64.dmg', size: 42 },
] };

test('first release uses the declared version', () => {
  assert.deepEqual(preflight(fixture()), { version: '0.1.1', tag: 'v0.1.1', publish: true });
});

test('reused versions automatically increment beyond the latest stable tag', () => {
  assert.equal(preflight(fixture({ tags: 'v0.1.1\nv0.1.9\nv0.1.10\nv2.0.0-beta.1\nunrelated' })).version, '0.1.11');
});

test('a newer declared major or minor version is preserved', () => {
  assert.equal(preflight(fixture({ version: '0.2.0', tags: 'v0.1.9' })).version, '0.2.0');
});

test('older source branches still select a version above the latest release', () => {
  assert.equal(preflight(fixture({ tags: 'v1.0.0\nv0.9.99' })).version, '1.0.1');
});

test('rerunning the same published commit skips even when source version is older', () => {
  assert.deepEqual(preflight(fixture({ tags: 'v0.1.2', atHead: 'v0.1.2', release: complete })), { version: '0.1.2', tag: 'v0.1.2', publish: false });
});

test('PRs preview the next version without querying or publishing releases', () => {
  assert.equal(preflight({ ...fixture({ tags: 'v0.1.1', atHead: 'v0.1.1' }), pr: true }).version, '0.1.2');
});

test('incomplete releases are not silently treated as successful', () => {
  for (const release of [{ ...complete, isDraft: true }, { ...complete, assets: complete.assets.slice(0, 1) }, { ...complete, assets: complete.assets.map((asset) => ({ ...asset, size: 0 })) }]) {
    assert.throws(() => preflight(fixture({ tags: 'v0.1.2', atHead: 'v0.1.2', release })), /incomplete/);
  }
});

test('GitHub lookup failures are not treated as a new or successful release', () => {
  assert.throws(() => preflight(fixture({ tags: 'v0.1.2', atHead: 'v0.1.2' })), /failed command/);
});

test('inconsistent lockfile versions fail before selecting a release', () => {
  const data = fixture();
  data.files['src-tauri/Cargo.lock'] = data.files['src-tauri/Cargo.lock'].replace('0.1.1', '0.1.0');
  assert.throws(() => preflight(data), /must match/);
});

test('selected version updates all six version fields without changing dependencies', () => {
  const data = fixture();
  setVersion('0.1.12', data);
  assert.equal(preflight(data).version, '0.1.12');
  assert.equal(JSON.parse(data.files['package-lock.json']).packages.dependency.version, '3.0.0');
  assert.match(data.files['src-tauri/Cargo.lock'], /name = "other"\nversion = "9.0.0"/);
});

test('invalid release versions cannot be written', () => {
  assert.throws(() => setVersion('1.2.3\ninjected=true', fixture()), /Invalid/);
});
