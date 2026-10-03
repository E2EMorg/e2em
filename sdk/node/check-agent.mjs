// Native desktop qualification helper; no account integration or retained drafts.
import fs from 'node:fs/promises';
import assert from 'node:assert/strict';
import {Client} from './index.js';

const credential = JSON.parse(await fs.readFile(process.argv[2], 'utf8'));
const cases = JSON.parse(await fs.readFile(new URL('../../tests/conformance/assessments.json', import.meta.url), 'utf8'));
const client = await Client.open({socketPath: credential.socket_path, ...credential});
try {
  for (const fixture of cases) {
    const result = await client.assess(fixture.request);
    assert.equal(result.status, fixture.expected.status);
    assert.equal(result.action, fixture.expected.action);
    assert.deepEqual(result.findings.flatMap(f => f.spans.map(s => [s.start, s.end])), fixture.expected.spans);
  }
  console.log(JSON.stringify({client: 'node', fixtures: cases.length, provider: client.capabilityManifest.provider}));
} finally { client.close(); }
