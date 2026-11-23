#!/usr/bin/env node
/**
 * Builds the Tessera design system for Claude Design from the app's real code:
 *
 *   design/out/project/   the design system's files (tokens.json, README, component bundle,
 *                         previews), at the paths the Design System artifact keeps them under
 *   design/out/app/       the whole app in the browser on the mock backend (a plain `vite build`)
 *
 * Run from app/: `npm run design`. Publishing is a separate step (design/README.md).
 */
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const appDir = path.resolve(here, '..');
const out = path.join(here, 'out');
const project = path.join(out, 'project');

function git(...args) {
  const r = spawnSync('git', args, { cwd: appDir, encoding: 'utf8' });
  return r.status === 0 ? r.stdout.trim() : 'unknown';
}
const sha = () => git('rev-parse', '--short', 'HEAD');
/** "main@1a2b3c4": the branch and commit this system was built from */
const ref = () => `${git('rev-parse', '--abbrev-ref', 'HEAD')}@${sha()}`;

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: appDir, stdio: 'inherit', shell: process.platform === 'win32' });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(' ')} failed`);
}

rmSync(project, { recursive: true, force: true });
mkdirSync(path.join(project, 'components'), { recursive: true });

/* ------------------------------------------------------------ bundle */

run('npx', ['vite', 'build', '--config', 'design/vite.config.mjs']);
const COMPONENTS = [
  'Window', 'Treemap', 'TitleBar', 'CommandBar', 'SearchBox', 'StatusBar', 'Pane', 'DrivePicker',
  'ContextMenu', 'ConfirmDialog', 'Toast', 'Tooltip', 'Button', 'SubtleButton', 'Card', 'Acrylic', 'Icon', 'Logo',
];
let js = readFileSync(path.join(out, 'bundle/bundle.js'), 'utf8');
// the bundle may be inlined into a <script>: nothing in it may end or comment out that element
js = js.replaceAll('<!--', '\\x3C!--').replaceAll('</script', '<\\/script');
if (/<!--|<\/script/i.test(js)) throw new Error('bundle.js still holds <!-- or </script');
const header = { format: 4, namespace: 'Tessera', components: COMPONENTS.map(name => ({ name })) };
writeFileSync(path.join(project, 'components/bundle.js'), `/* @ds-bundle: ${JSON.stringify(header)} */\n${js}`);
let css = readFileSync(path.join(out, 'bundle/bundle.css'), 'utf8');
if (/<\/style/i.test(css)) throw new Error('bundle.css holds </style');
writeFileSync(path.join(project, 'components/bundle.css'), css);

/* ------------------------------------------------------------ tokens */

const { createServer } = await import('vite');
const server = await createServer({ root: appDir, configFile: false, logLevel: 'error', server: { middlewareMode: true, hmr: false }, appType: 'custom' });
let tokens;
try {
  const theme = await server.ssrLoadModule('/src/lib/theme.ts');
  const motion = await server.ssrLoadModule('/src/lib/treemap/motion.ts');
  const en = await server.ssrLoadModule('/src/lib/locales/en.ts').catch(() => null);
  tokens = makeTokens(theme, motion.fluentLayout, readPaintStyle(), en);
} finally {
  await server.close();
}
writeFileSync(path.join(project, 'tokens.json'), JSON.stringify(tokens, null, 2) + '\n');
// the page never writes tokens.css for a system synced from code, so the sync does
writeFileSync(path.join(project, 'tokens.css'), tokensCss(tokens));

/** tokens.css as the Design System page compiles it (its format.md › "tokens.css as compiled") */
function tokensCss(t) {
  const [first, ...rest] = t.color.themes.map(x => x.id);
  const val = (tok, theme) => (typeof tok.value === 'string' ? tok.value : (tok.value[theme] ?? tok.value[first]));
  const themed = [...t.color.tokens, ...t.shadow.tokens];
  const lines = [`/* ${t.name} — generated from tokens.json */`];
  lines.push(`:root, [data-theme="${first}"] {`, ...themed.map(x => `  --${x.name}: ${val(x, first)};`), '}');
  for (const th of rest) lines.push(`[data-theme="${th}"] {`, ...themed.map(x => `  --${x.name}: ${val(x, th)};`), '}');
  const plain = [...t.spacing.tokens, ...t.radius.tokens, ...t.duration.tokens, ...t.easing.tokens];
  lines.push(':root {', ...plain.map(x => `  --${x.name}: ${x.value};`), ...Object.entries(t.type.families).map(([k, v]) => `  --font-${k}: ${v};`), '}');
  for (const g of t.type.groups)
    for (const st of g.styles) {
      const fam = st.family ?? g.family;
      lines.push(`.${st.name} { font-family: var(--font-${fam}); font-size: ${st.fontSize}; line-height: ${st.lineHeight}; font-weight: ${st.fontWeight}; }`);
    }
  return lines.join('\n') + '\n';
}

// the logo masters, uploaded as assets when publishing (design/README.md)
const logos = path.join(project, 'assets/Logos');
mkdirSync(logos, { recursive: true });
for (const f of ['tessera.svg', 'tessera-16.svg']) cpSync(path.join(appDir, '../docs/brand', f), path.join(logos, f));

/** what applyThemeCss writes onto <html>, per scheme */
function cssVars(theme, scheme) {
  const vars = {};
  const el = { style: { setProperty: (k, v) => (vars[k.replace(/^--/, '')] = v) }, dataset: {} };
  theme.applyThemeCss(theme.makeTheme(scheme, null), el);
  return vars;
}

/** the logo's fixed colours, from the 16 px artwork in Logo.svelte (the same as docs/brand) */
function readLogoColours() {
  const src = readFileSync(path.join(appDir, 'src/components/Logo.svelte'), 'utf8');
  const hex = [...src.matchAll(/(fill|stroke)="(#[0-9A-Fa-f]{6})"/g)].map(m => m[2].toLowerCase());
  const [paper, ink, blue, sky, navy] = [...new Set(hex)];
  if (!navy) throw new Error('logo colours not found in Logo.svelte');
  return [
    ['brand-blue', blue, 'Logo: the big tile. Fixed in both themes; never a UI colour.'],
    ['brand-sky', sky, 'Logo: the small top tile.'],
    ['brand-navy', navy, 'Logo: the lower tile.'],
    ['brand-paper', paper, 'Logo: the disc and the centre hub.'],
    ['brand-ink', ink, 'Logo: the disc outline.'],
  ];
}

/** paint.ts pulls in the runes-based i18n module, so its radii are read from the source */
function readPaintStyle() {
  const src = readFileSync(path.join(appDir, 'src/lib/treemap/paint.ts'), 'utf8');
  const body = /export const paintStyle = {([^}]*)}/.exec(src)?.[1] ?? '';
  const num = k => Number(new RegExp(`${k}:\\s*([\\d.]+)`).exec(body)?.[1]);
  const style = { radius: num('radius'), folderRadius: num('folderRadius') };
  if (!Number.isFinite(style.radius) || !Number.isFinite(style.folderRadius)) throw new Error('paintStyle not found in paint.ts');
  return style;
}

function readUiCss() {
  const src = readFileSync(path.join(appDir, 'src/ui.css'), 'utf8');
  const root = /:root\s*{([^}]*)}/.exec(src)?.[1] ?? '';
  const vars = {};
  for (const m of root.matchAll(/--([\w-]+):\s*([^;]+);/g)) vars[m[1]] = m[2].trim();
  return vars;
}

function makeTokens(theme, layout, paintStyle, en) {
  const light = cssVars(theme, 'light');
  const dark = cssVars(theme, 'dark');
  const dict = en?.default ?? en?.en ?? {};
  const catName = i => dict[`cat.${i}`] ?? `category ${i}`;
  const usage = {
    'solid-base': 'Window background without Mica (Windows 10, Transparency effects off).',
    'mica-a': 'Mica wash, top-left stop (browser stand-in for the real Mica backdrop).',
    'mica-b': 'Mica wash, bottom-right stop.',
    layer: 'Layer under cards (LayerFillColorDefault).',
    card: 'Card and details-pane fill (CardBackgroundFillColorDefault).',
    'card-2': 'Secondary card fill.',
    'card-stroke': 'Card border.',
    ctl: 'Standard button / text box fill (ControlFillColorDefault).',
    'ctl-hover': 'Standard control, pointer over.',
    'ctl-press': 'Standard control, pressed.',
    'ctl-stroke': 'Control border; also the border of acrylic surfaces.',
    'ctl-stroke-2': 'Control bottom edge (the Fluent "elevation" line); window outline.',
    'ctl-strong': 'Strong control stroke: unchecked boxes, slider rails.',
    'ctl-input-active': 'Text box that has the focus.',
    'ctl-alt': 'Sunken track of the segmented control.',
    'subtle-hover': 'Subtle (.sbtn) button, pointer over; menu rows.',
    'subtle-press': 'Subtle button, pressed.',
    divider: 'Separators and hairlines.',
    text: 'Primary text (TextFillColorPrimary).',
    'text-2': 'Secondary text: sizes, paths, captions.',
    'text-3': 'Tertiary text: hints, placeholders.',
    'text-dis': 'Disabled text.',
    accent: 'Accent (Windows accent colour; this is the default blue): accent buttons, focus pill, selection.',
    'on-accent': 'Text on accent fills.',
    'accent-soft': 'Accent wash behind selected rows.',
    'accent-hover': 'Accent button, pointer over (90 % accent).',
    'accent-press': 'Accent button, pressed (80 % accent).',
    critical: '"Delete permanently" and other destructive actions.',
    'critical-bg': 'Critical banner / error toast background.',
    'on-critical': 'Text on critical fills.',
    'critical-hover': 'Danger button, pointer over (90 % critical).',
    'critical-press': 'Danger button, pressed (80 % critical).',
    caution: 'Warnings: admin hint, locked items.',
    acrylic: 'Acrylic tint (menus, tooltips, toasts); the blur and noise are in .acrylic.',
    'acrylic-solid': 'Acrylic fallback when transparency effects are off.',
    'dlg-top': 'Content dialog, upper (content) area.',
    'dlg-stroke': 'Content dialog border.',
    smoke: 'Dimming layer behind a content dialog.',
    'tm-stage': 'Treemap: the stage behind all tiles.',
    'tm-folder': 'Treemap: open folder body.',
    'tm-folder-head': 'Treemap: open folder title band.',
    'tm-folder-text': 'Treemap: folder title text.',
    'tm-leafdir': 'Treemap: closed folder fill.',
    'tm-leafdir-stroke': 'Treemap: closed folder border.',
    'tm-leafdir-ink': 'Treemap: closed folder label.',
    'tm-hover': 'Treemap: light over the hovered tile.',
    'tm-reveal': 'Treemap: the pointer light on tile edges near the cursor (accent based in the light theme).',
    'tm-stripe': 'Treemap: stripes on space not scanned yet (dashed ctl-strong border).',
    'tm-dots': 'Treemap: dots on a merged "N small items" tile.',
    'tm-hatch': 'Treemap: hatch on locked items.',
  };
  const colors = [];
  const shadows = [];
  for (const name of Object.keys(light)) {
    if (name.startsWith('cat-')) continue;
    const entry = { name, value: { light: light[name], dark: dark[name] ?? light[name] }, usage: usage[name] ?? '' };
    (name.startsWith('shadow') ? shadows : colors).push(entry);
  }
  for (let i = 0; i < 12; i++) {
    const t = theme.makeTheme('light', null);
    const d = theme.makeTheme('dark', null);
    colors.push({ name: `cat-${i}`, value: { light: t.fill[i], dark: d.fill[i] }, usage: `Treemap tile fill: ${catName(i)}.` });
    colors.push({ name: `cat-${i}-ink`, value: { light: t.ink[i], dark: d.ink[i] }, usage: `Treemap label on a ${catName(i)} tile.` });
  }
  for (const [name, value, use] of readLogoColours()) colors.push({ name, value, usage: use });
  shadows.forEach(s => (s.usage = s.name === 'shadow' ? 'Flyouts, menus, tooltips, toasts.' : 'Content dialog.'));
  const ui = readUiCss();
  return {
    name: 'Tessera Fluent',
    version: 1,
    meta: {
      source: 'github',
      repo: '4zuma/tessera',
      package: 'app',
      generatedBy: 'app/design/build.mjs',
      paths: {
        tokens: ['app/src/lib/theme.ts', 'app/src/ui.css', 'app/src/lib/treemap/motion.ts', 'app/src/lib/treemap/paint.ts'],
        assets: ['docs/brand/tessera.svg', 'docs/brand/tessera-16.svg'],
        docs: ['app/design/system'],
      },
      components: Object.fromEntries(
        [
          ['Window', 'App'], ['Treemap', 'components/Stage'], ['TitleBar'], ['CommandBar'], ['SearchBox'], ['StatusBar'], ['Pane'],
          ['DrivePicker'], ['ContextMenu'], ['ConfirmDialog'], ['Toast'], ['Tooltip'], ['Icon'], ['Logo'],
        ].map(([name, file = `components/${name}`]) => [name, `app/src/${file}.svelte`]),
      ),
      ref: ref(),
      synced: new Date().toISOString().slice(0, 10),
    },
    color: { themes: [{ id: 'light', name: 'Light' }, { id: 'dark', name: 'Dark' }], tokens: colors },
    type: {
      fonts: [],
      families: { sans: ui.font, display: ui['font-display'] },
      groups: [
        {
          name: 'Fluent type ramp',
          family: 'sans',
          styles: [
            { name: 'type-caption', fontSize: '12px', lineHeight: '16px', fontWeight: 400, usage: 'Sizes, menu shortcuts, status bar.' },
            { name: 'type-body', fontSize: '14px', lineHeight: '20px', fontWeight: 400, usage: 'All UI text.' },
            { name: 'type-body-strong', fontSize: '14px', lineHeight: '20px', fontWeight: 600, usage: 'Names in the details pane, dialog leads.' },
            { name: 'type-subtitle', family: 'display', fontSize: '20px', lineHeight: '28px', fontWeight: 600, usage: 'Dialog titles.' },
            { name: 'type-title', family: 'display', fontSize: '28px', lineHeight: '36px', fontWeight: 600, usage: 'Drive picker heading, big numbers.' },
          ],
        },
        {
          name: 'Treemap',
          family: 'sans',
          styles: [
            { name: 'type-tile-label', fontSize: '12px', lineHeight: '16px', fontWeight: 400, usage: 'File and size labels drawn on the canvas.' },
            { name: 'type-tile-label-strong', fontSize: '12px', lineHeight: '16px', fontWeight: 600, usage: 'Folder titles drawn on the canvas.' },
          ],
        },
      ],
    },
    spacing: {
      tokens: [
        { name: 'space-1', value: '4px', usage: 'Icon to text in compact rows; menu padding.' },
        { name: 'space-2', value: '8px', usage: 'Gap between window regions (map, pane), button icon gap.' },
        { name: 'space-3', value: '12px', usage: 'Standard button side padding.' },
        { name: 'space-4', value: '16px', usage: 'Card padding.' },
        { name: 'space-6', value: '24px', usage: 'Dialog padding.' },
        { name: 'control-height', value: '32px', usage: 'Buttons, search box, segmented control.' },
        { name: 'tm-outer', value: `${layout.outer}px`, usage: 'Treemap: margin around the whole map.' },
        { name: 'tm-pad', value: `${layout.pad}px`, usage: 'Treemap: inset of children inside an open folder.' },
        { name: 'tm-gap', value: `${layout.gap}px`, usage: 'Treemap: gap between sibling tiles.' },
        { name: 'tm-header', value: `${layout.header}px`, usage: 'Treemap: open folder title band height.' },
        { name: 'tm-label-min-w', value: `${layout.labelMinW}px`, usage: 'Treemap: narrower tiles get no label (tooltip only).' },
        { name: 'tm-label-min-h', value: `${layout.labelMinH}px`, usage: 'Treemap: shorter tiles get no label.' },
      ],
    },
    radius: {
      tokens: [
        { name: 'r-ctl', value: ui['r-ctl'], usage: 'Buttons, text boxes, menu rows.' },
        { name: 'r-card', value: ui['r-card'], usage: 'Cards, menus, flyouts, dialogs, the window.' },
        { name: 'tm-radius', value: `${paintStyle.radius}px`, usage: 'Treemap: file and closed-folder tiles.' },
        { name: 'tm-folder-radius', value: `${paintStyle.folderRadius}px`, usage: 'Treemap: open folders.' },
      ],
    },
    shadow: { tokens: shadows },
    duration: {
      tokens: ['t-fast', 't-norm', 't-slow'].map(n => ({ name: n, value: ui[n], usage: { 't-fast': 'Hover fills, small state changes.', 't-norm': 'Flyouts opening.', 't-slow': 'Dialogs, pane.' }[n] })),
    },
    easing: {
      tokens: ['ease-decel', 'ease-fast', 'ease-accel'].map(n => ({ name: n, value: ui[n], usage: { 'ease-decel': 'Things coming in (decelerate).', 'ease-fast': 'Fluent fast-out, snappy moves.', 'ease-accel': 'Things leaving (accelerate).' }[n] })),
    },
  };
}

/* ------------------------------------------------------------ hand-written content */

cpSync(path.join(here, 'system'), project, { recursive: true });
const index = path.join(project, 'design-system.json');
if (!existsSync(index)) {
  // the index for a FIRST publish only; an update re-reads the published one (design/README.md)
  writeFileSync(
    index,
    JSON.stringify(
      {
        v: 3,
        layout: 'files',
        createdOnFiles: { v: 1, at: new Date().toISOString() },
        title: 'Tessera Fluent',
        namespace: 'Tessera',
        editing: 'code',
        libraries: [
          { name: 'react', version: '18' },
          { name: 'react-dom', version: '18' },
        ],
        lastChange: { by: process.env.DS_BY || 'Claude', at: new Date().toISOString(), via: `GitHub · 4zuma/tessera@${sha()}`, note: 'synced from app/design' },
        sections: {},
        groups: ['Logos'],
        assetGroups: { Logos: { name: 'Logos', tile: 'l', order: [], files: {} } },
        blobs: {},
        docs: { readme: 'project/README.md', sections: [] },
      },
      null,
      2,
    ) + '\n',
  );
}

/* ------------------------------------------------------------ the whole app */

run('npx', ['vite', 'build', '--outDir', 'design/out/app', '--emptyOutDir']);

console.log(`\ndesign system: ${path.relative(appDir, project)}  (bundle ${(js.length / 1024).toFixed(0)} KB, ${tokens.color.tokens.length} colours)`);
console.log(`whole app:     ${path.relative(appDir, path.join(out, 'app'))}`);
