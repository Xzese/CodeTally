import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

// Ruby's bundled YAML parser lets these checks inspect the actual workflow
// without installing npm dependencies or an additional Ruby test framework.
const workflowPath = process.env.RELEASE_WORKFLOW_PATH ?? fileURLToPath(new URL('../.github/workflows/release.yml', import.meta.url));
const workflow = JSON.parse(execFileSync('ruby', ['-r', 'yaml', '-r', 'json', '-e', 'puts JSON.generate(YAML.load_file(ARGV.fetch(0)))', workflowPath], { encoding: 'utf8' }));

test('source steps select main without custom credentials and preserve branches', () => {
  const root = mkdtempSync(join(tmpdir(), 'codetally-release-source-'));
  const git = (...args) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  try {
    git('init', '-b', 'main');
    const commit = (message) => git('-c', 'user.name=Release test', '-c', 'user.email=release@example.invalid', 'commit', '--allow-empty', '-m', message);
    commit('Released commit');
    commit('New main commit');
    const main = git('rev-parse', 'HEAD');
    const branches = git('show-ref', '--heads');
    const events = [['schedule', 'main', main], ['workflow_dispatch', 'main', main]];

    for (const [index, [event, branch, sha]] of events.entries()) {
      git('checkout', '--detach', sha);
      const outputFile = join(root, `output-${index}`);
      const summaryFile = join(root, `summary-${index}`);
      const env = {
        PATH: process.env.PATH, HOME: root,
        GITHUB_EVENT_NAME: event, GITHUB_REF: `refs/heads/${branch}`,
        GITHUB_SHA: sha, GITHUB_OUTPUT: outputFile, GITHUB_STEP_SUMMARY: summaryFile,
      };
      for (const step of workflow.jobs.source.steps) {
        if (step.run) execFileSync('bash', ['-e', '-c', step.run], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
      }
      assert.equal(readFileSync(outputFile, 'utf8'), `sha=${sha}\n`, `${event} on ${branch}`);
      assert.ok(readFileSync(summaryFile, 'utf8').includes(sha));
      assert.equal(git('show-ref', '--heads'), branches);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('every job uses the selected commit with read-only source access', () => {
  const jobs = workflow.jobs;
  const source = jobs.source;
  // The remaining publication entry points are nightly and manual main runs.
  const triggers = workflow.on ?? workflow.true; // Ruby YAML 1.1 parses "on" as true.
  assert.deepEqual(Object.keys(triggers).sort(), ['schedule', 'workflow_dispatch']);
  assert.deepEqual(triggers.schedule, [{ cron: '0 0 * * *' }]);
  assert.equal(source.if, "github.ref == 'refs/heads/main'");
  const checkouts = (job) => job.steps.filter((step) => step.uses?.startsWith('actions/checkout@'));
  assert.deepEqual(source.permissions, { contents: 'read' });
  assert.equal(checkouts(source)[0].with.ref, '${{ github.sha }}');
  assert.equal(checkouts(source)[0].with['persist-credentials'], false);
  // Repository-specific credentials must never be a prerequisite for source selection.
  assert.doesNotMatch(JSON.stringify(source), /secrets\./);
  assert.equal(source.outputs.sha, '${{ steps.commit.outputs.sha }}');
  for (const name of ['preflight', 'checks', 'build', 'publish']) {
    const job = jobs[name];
    assert.ok([job.needs].flat().includes('source'));
    assert.ok(checkouts(job).length > 0);
    for (const step of checkouts(job)) assert.equal(step.with.ref, '${{ needs.source.outputs.sha }}');
  }
  const publish = jobs.publish.steps.find((step) => step.env?.RELEASE_SHA);
  assert.equal(publish.env.RELEASE_SHA, '${{ needs.source.outputs.sha }}');
});
