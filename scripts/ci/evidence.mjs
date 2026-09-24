import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

// Create this before dependency installation, and finalize it even on failure.
const [mode, directory] = process.argv.slice(2);
if (!['start', 'finish'].includes(mode) || !directory) throw new Error('evidence.mjs start|finish DIRECTORY');
fs.mkdirSync(directory, { recursive: true });
const file = path.join(directory, 'manifest.json');
if (mode === 'start') {
  fs.writeFileSync(file, JSON.stringify({
    schema_version: 1, commit: execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    started_at: new Date().toISOString(), os: { platform: os.platform(), release: os.release(), arch: os.arch() },
    node: process.version, runner: process.env.RUNNER_NAME, job: process.env.GITHUB_JOB,
    conditions: 'Hosted CI; no physical USB or driver loading; performance is a catastrophe ceiling only',
    status: 'running', stages: {},
  }, null, 2));
} else {
  const manifest = JSON.parse(fs.readFileSync(file, 'utf8'));
  const steps = JSON.parse(process.env.EVIDENCE_STEPS || '{}');
  manifest.stages = Object.fromEntries(Object.entries(steps).map(([id, step]) => [id, step.outcome]));
  const outcomes = Object.values(manifest.stages);
  manifest.status = outcomes.some(s => s === 'failure' || s === 'cancelled') ? 'failed'
    : outcomes.some(s => s === 'skipped') ? 'incomplete' : 'completed';
  manifest.finished_at = new Date().toISOString();
  for (const [key, command] of [['rustc', 'rustc'], ['cargo', 'cargo']]) {
    try { manifest[key] = execFileSync(command, ['--version'], { encoding: 'utf8' }).trim(); }
    catch { manifest[key] = 'unavailable'; }
  }
  fs.writeFileSync(file, JSON.stringify(manifest, null, 2));
}
