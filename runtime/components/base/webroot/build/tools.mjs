// Windows installs npm and esbuild with cmd shims. Spawn their underlying programs directly.
import { dirname, join } from 'node:path';

export const npmCommand = process.platform === 'win32'
  ? [process.execPath, join(dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js')]
  : ['npm'];

export function esbuildBinary(modules) {
  return process.platform === 'win32'
    ? join(modules, '@esbuild', 'win32-' + process.arch, 'esbuild.exe')
    : join(modules, '.bin/esbuild');
}
