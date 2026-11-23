// Fontsource supplies both the WOFF2 and the Unicode ranges for each face.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
export const packs = JSON.parse(readFileSync(new URL('fonts.json', import.meta.url)));

export function fontLibraries() {
  return Object.entries(packs).filter(([id]) => id !== 'excalidraw-cjk').flatMap(([id, pack]) => {
    let styles = [];
    return pack.sources.map((source, index) => ({
      id: 'font-' + source.package.split('/')[1], name: source.package, npm: source.package,
      license: pack.license, licenseFiles: ['LICENSE'], copy: [],
      produce(ctx) {
        const pkg = ctx.package(this.id);
        if (pkg.integrity !== source.integrity || this.version !== source.version)
          throw new Error('Review font pack source and integrity in fonts.json for ' + source.package);
        const css = readFileSync(join(pkg.root, 'wght.css'), 'utf8');
        const prefix = 'fonts-extra/' + id + '/';
        const files = [];
        if (index === 0) styles = [];
        for (const [rule] of css.matchAll(/@font-face\s*\{[^}]+\}/g)) {
          const file = rule.match(/url\(\.\/files\/([\w.-]+)\)/)?.[1];
          if (!file) throw new Error('Unknown fontsource CSS in ' + source.package);
          const subset = file.slice(source.package.split('/')[1].length + 1, -'-wght-normal.woff2'.length);
          if (!pack.subsets.includes(subset) && !(pack.subsets.includes('numbered') && /^\d+$/.test(subset))) continue;
          files.push([prefix + file, readFileSync(join(pkg.root, 'files', file))]);
          styles.push(rule.replace(/font-family: [^;]+;/, 'font-family: "Atkinson Hyperlegible Next";')
            .replace('./files/', './'));
        }
        if (!files.length) throw new Error('No matching font subsets for ' + source.package);
        if (index === pack.sources.length - 1) files.push([prefix + id + '.css', Buffer.from(styles.join('\n') + '\n')]);
        return files;
      }
    }));
  });
}

export function assetPack(path) {
  if (path.startsWith('excalidraw/fonts/Xiaolai/')) return 'excalidraw-cjk';
  if (path.startsWith('fonts-extra/')) return path.split('/')[1];
}
