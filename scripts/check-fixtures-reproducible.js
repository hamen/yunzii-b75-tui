#!/usr/bin/env node
'use strict';
//
// Asserts that every committed animated-GIF fixture is exactly what
// `scripts/make-test-gifs.js` produces right now.
//
//   node scripts/check-fixtures-reproducible.js
//
// Why this exists: the generator was added so the fixtures would stop being
// binaries taken on trust. That argument only holds while the two agree. Left
// unchecked, someone edits a fixture by hand, or edits the generator without
// regenerating, and the repo goes back to committed blobs nobody can rebuild --
// with a script sitting next to them implying otherwise, which is worse than
// having no script at all.
//
// The generator writes into `fixtures/`, so this runs it against a scratch
// directory and compares, rather than regenerating in place and diffing: a
// check that repairs what it is checking always passes.

const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const repoRoot = path.join(__dirname, '..');
const fixturesDir = path.join(repoRoot, 'fixtures');
const generator = path.join(__dirname, 'make-test-gifs.js');

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'yunzii-fixtures-'));
let failures = 0;

try {
  // The generator resolves its output as `__dirname/../fixtures`, so a copy of
  // it inside the scratch tree writes into the scratch tree.
  const scratchScripts = path.join(tmp, 'scripts');
  fs.mkdirSync(scratchScripts, { recursive: true });
  fs.mkdirSync(path.join(tmp, 'fixtures'), { recursive: true });
  fs.copyFileSync(generator, path.join(scratchScripts, 'make-test-gifs.js'));

  execFileSync(process.execPath, [path.join(scratchScripts, 'make-test-gifs.js')], {
    stdio: 'pipe',
  });

  const regenerated = fs
    .readdirSync(path.join(tmp, 'fixtures'))
    .filter((f) => f.endsWith('.gif'))
    .sort();

  if (regenerated.length === 0) {
    console.error('FAIL: the generator produced no GIF files at all');
    process.exit(1);
  }

  for (const name of regenerated) {
    const fresh = fs.readFileSync(path.join(tmp, 'fixtures', name));
    const committedPath = path.join(fixturesDir, name);

    if (!fs.existsSync(committedPath)) {
      console.error(
        `FAIL: ${name} is produced by make-test-gifs.js but is not committed ` +
          `-- run "node scripts/make-test-gifs.js" and add it`
      );
      failures++;
      continue;
    }

    const committed = fs.readFileSync(committedPath);
    if (Buffer.compare(fresh, committed) !== 0) {
      console.error(
        `FAIL: fixtures/${name} does not match a clean run of make-test-gifs.js ` +
          `(committed ${committed.length} bytes, regenerated ${fresh.length} bytes) ` +
          `-- run "node scripts/make-test-gifs.js" and commit the result`
      );
      failures++;
      continue;
    }

    console.log(`OK:   fixtures/${name} matches the generator (${committed.length} bytes)`);
  }

  // The reverse direction: an animation fixture in the tree that the generator
  // does not know how to build is exactly the untrustworthy blob this guards
  // against.
  const committedAnims = fs
    .readdirSync(fixturesDir)
    .filter((f) => f.startsWith('test-anim-') && f.endsWith('.gif'))
    .sort();
  for (const name of committedAnims) {
    if (!regenerated.includes(name)) {
      console.error(
        `FAIL: fixtures/${name} is committed but make-test-gifs.js does not build it ` +
          `-- add it to the generator or delete it`
      );
      failures++;
    }
  }
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}

if (failures > 0) {
  console.error(`\n${failures} fixture(s) are not reproducible.`);
  process.exit(1);
}
console.log('All GIF fixtures are byte-identical to a clean generator run.');
