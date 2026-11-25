import { execFileSync } from 'node:child_process';
import {
  constants, copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync,
  readdirSync, readFileSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('..', import.meta.url));
const [outputDir, workDir, ...options] = process.argv.slice(2);
if (!outputDir || !workDir || options.some(option => option !== '--offline')) {
  throw new Error('Use node tools/package-windows.mjs OUTPUT_DIR WORK_DIR [--offline]');
}
if (process.platform !== 'win32') {
  throw new Error('Build this Windows x64 package on a Windows host.');
}

function command(program, args, options = {}) {
  return execFileSync(program, args, {
    cwd: repo,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
    ...options,
  });
}

if (command('git', ['status', '--porcelain']).trim()) {
  throw new Error('Commit the source tree before packaging so binaries and source match.');
}
const commit = command('git', ['rev-parse', 'HEAD']).trim();
const cargoOptions = options.includes('--offline') ? ['--offline'] : [];
const target = 'x86_64-pc-windows-msvc';
const metadata = JSON.parse(command('cargo', [
  'metadata', '--locked', '--format-version', '1', '--filter-platform', target, ...cargoOptions,
]));
const project = metadata.packages.find(pkg => pkg.id === metadata.resolve.root);
if (!project || project.name !== 'parsley') {
  throw new Error('Could not locate the Parsley Cargo package.');
}

const name = `Parsley-${project.version}-windows-x64.zip`;
const output = resolve(outputDir, name);
if (existsSync(output)) {
  throw new Error(`Choose an output directory without an existing ${name}.`);
}
mkdirSync(resolve(workDir), { recursive: true });
const stage = mkdtempSync(join(resolve(workDir), 'parsley-package-'));
const bundle = join(stage, 'bundle');
for (const directory of ['source', 'examples', 'dependency-licences']) {
  mkdirSync(join(bundle, directory), { recursive: true });
}

command('cargo', ['build', '--locked', '--release', '--bins', '--target', target, ...cargoOptions], { stdio: 'inherit' });
for (const name of ['parsley.exe', 'parsley-desktop.exe']) {
  const binary = join(metadata.target_directory, target, 'release', name);
  const bytes = readFileSync(binary);
  const pe = bytes.readUInt32LE(60);
  if (bytes.toString('ascii', 0, 2) !== 'MZ' || bytes.toString('ascii', pe, pe + 4) !== 'PE\u0000\u0000'
      || bytes.readUInt16LE(pe + 4) !== 0x8664) {
    throw new Error(`${name} is not a Windows x64 binary.`);
  }
  copyFileSync(binary, join(bundle, name));
}

const sourceTar = join(stage, 'source.tar');
command('git', ['archive', '--format=tar', `--output=${sourceTar}`, commit]);
command('tar.exe', ['-xf', sourceTar, '-C', join(bundle, 'source')]);
for (const file of ['CONTRIBUTING.md', 'LICENSE', 'README.md']) {
  copyFileSync(join(bundle, 'source', file), join(bundle, file));
}
cpSync(join(bundle, 'source', 'docs'), join(bundle, 'docs'), { recursive: true });
mkdirSync(join(bundle, 'ui'));
copyFileSync(join(bundle, 'source', 'ui', 'OFL-LICENSE.txt'), join(bundle, 'ui', 'OFL-LICENSE.txt'));
// These crate archives omit the notices present at their pinned upstream commits.
const pinnedNotices = new Map([
  ['webview2-com@0.39.1', ['webview2-rs-LICENSE.txt']],
  ['webview2-com-macros@0.8.1', ['webview2-rs-LICENSE.txt']],
  ['webview2-com-sys@0.39.1', [
    'webview2-rs-LICENSE.txt',
    'Microsoft-WebView2-1.0.3800.47-LICENSE.txt',
    'Microsoft-WebView2-1.0.3800.47-NOTICE.txt',
  ]],
]);
copyFileSync(join(bundle, 'source', 'ui', 'OFL-LICENSE.txt'),
  join(bundle, 'dependency-licences', 'Geist-OFL-LICENSE.txt'));
for (const pkg of metadata.packages) {
  if (pkg.id === project.id) continue;
  const source = dirname(pkg.manifest_path);
  const notices = readdirSync(source, { withFileTypes: true })
    .filter(entry => entry.isFile() && /^(licen[sc]e|copying|notice)/i.test(entry.name));
  const pinned = pinnedNotices.get(`${pkg.name}@${pkg.version}`) ?? [];
  if (!notices.length && !pinned.length) {
    throw new Error(`No licence notice was found for ${pkg.name} ${pkg.version}.`);
  }
  const dest = join(bundle, 'dependency-licences', `${pkg.name}-${pkg.version}`);
  mkdirSync(dest);
  for (const notice of notices) copyFileSync(join(source, notice.name), join(dest, notice.name));
  for (const notice of pinned) copyFileSync(join(repo, 'tools', 'notices', notice), join(dest, notice));
}

for (const [mode, report, ...args] of [
  ['demo', 'incident.html'],
  ['demo-case', 'graphics-driver-reset.html', 'graphics'],
  ['demo-case', 'crashing-application.html', 'application'],
  ['demo-case', 'unexplained-restart.html', 'restart'],
  ['demo-compare', 'comparison.html'],
  ['demo-health', 'health.html'],
  ['demo-inventory', 'inventory.html'],
  ['demo-inventory-compare', 'inventory-comparison.html'],
  ['demo-history', 'history.html'],
  ['demo-connections', 'connections.html'],
  ['demo-bandwidth', 'bandwidth.html'],
  ['demo-support', 'support.html', '--include-health', '--include-inventory'],
]) {
  const output = join(bundle, 'examples', report);
  const commandArgs = mode === 'demo-case' ? [mode, ...args, output] : [mode, output, ...args];
  command(join(bundle, 'parsley.exe'), commandArgs);
}

if (command('git', ['rev-parse', 'HEAD']).trim() !== commit || command('git', ['status', '--porcelain']).trim()) {
  throw new Error('The source changed during packaging.');
}
const zip = join(stage, name);
command('tar.exe', ['-a', '-cf', zip, '-C', bundle, '.']);
mkdirSync(dirname(output), { recursive: true });
copyFileSync(zip, output, constants.COPYFILE_EXCL);
console.log(`Packaged ${basename(output)} from ${commit.slice(0, 7)}.`);
