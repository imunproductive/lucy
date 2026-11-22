// Build the dashboard into a temporary folder and compare it with the
// committed dist/: the server embeds dist/, so it must be the build of the
// source as it is. `npm run build` updates it.
import {execFileSync} from 'node:child_process';
import {mkdtempSync, readdirSync, readFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, relative} from 'node:path';
import {fileURLToPath} from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const committed = join(root, 'dist');
const fresh = mkdtempSync(join(tmpdir(), 'lucy-dashboard-'));

function files(folder) {
  return readdirSync(folder, {recursive: true, withFileTypes: true})
    .filter(entry => entry.isFile())
    .map(entry => relative(folder, join(entry.parentPath, entry.name)).replaceAll('\\', '/'))
    .sort();
}

try {
  execFileSync(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), 'build', '--outDir', fresh, '--emptyOutDir', '--logLevel', 'error'], {
    cwd: root,
    stdio: 'inherit',
  });
  const want = files(fresh);
  const have = files(committed);
  const problems = [
    ...want.filter(file => !have.includes(file)).map(file => `missing from dist: ${file}`),
    ...have.filter(file => !want.includes(file)).map(file => `not in a fresh build: ${file}`),
    ...want.filter(file => have.includes(file) && !readFileSync(join(fresh, file)).equals(readFileSync(join(committed, file)))).map(file => `differs: ${file}`),
  ];
  if (problems.length) {
    console.error(`web/dashboard/dist does not match the source:\n  ${problems.join('\n  ')}\nRun \`just dashboard-build\` and commit dist/.`);
    process.exitCode = 1;
  } else {
    console.log(`web/dashboard/dist matches a fresh build (${want.length} files).`);
  }
} finally {
  rmSync(fresh, {recursive: true, force: true});
}
