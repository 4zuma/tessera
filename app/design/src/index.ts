/**
 * The Tessera design-system bundle: window.Tessera.<Component>, React 18 components that mount
 * the app's real Svelte components (see wrap.ts), on the mock backend. Built by
 * `npm run design` (design/build.mjs); README.md in this folder says how it is published.
 */
import '../../src/ui.css';
import './base.css';
import App from '../../src/App.svelte';
import CommandBar from '../../src/components/CommandBar.svelte';
import ConfirmDialog from '../../src/components/ConfirmDialog.svelte';
import ContextMenu from '../../src/components/ContextMenu.svelte';
import DrivePicker from '../../src/components/DrivePicker.svelte';
import IconSvelte from '../../src/components/Icon.svelte';
import LogoSvelte from '../../src/components/Logo.svelte';
import Pane from '../../src/components/Pane.svelte';
import SearchBox from '../../src/components/SearchBox.svelte';
import Stage from '../../src/components/Stage.svelte';
import StatusBar from '../../src/components/StatusBar.svelte';
import TitleBar from '../../src/components/TitleBar.svelte';
import Toast from '../../src/components/Toast.svelte';
import Tooltip from '../../src/components/Tooltip.svelte';
import { ICONS } from '../../src/lib/icons';
import { makePrimitives } from './primitives';
import { SCENARIOS } from './runtime';
import { wrap } from './wrap';

const fill = { display: 'block', position: 'relative', width: '100%', height: '100%' };
const flexFill = { display: 'flex', width: '100%', height: '100%', minWidth: 0, minHeight: 0 };

const Window = wrap('Window', App, {
  host: fill,
  window: true,
  attrs: p => ({ 'data-frame': p.frame === 'desk' ? 'desk' : 'window', 'data-backdrop': p.backdrop === 'solid' ? 'solid' : 'mica' }),
});
const Treemap = wrap('Treemap', Stage, { host: flexFill });
const TitleBar_ = wrap('TitleBar', TitleBar, { host: { display: 'block', width: '100%' } });
const CommandBar_ = wrap('CommandBar', CommandBar, {
  host: { display: 'block', width: '100%' },
  props: p => ({ paneOpen: p.paneOpen ?? true, canTogglePane: true, onTogglePane: () => {}, search: !!p.search }),
});
const SearchBox_ = wrap('SearchBox', SearchBox, { host: { display: 'block', width: '100%' } });
const StatusBar_ = wrap('StatusBar', StatusBar, { host: { display: 'block', width: '100%' } });
const Pane_ = wrap('Pane', Pane, { host: { display: 'flex', height: '100%', minHeight: 0 } });
const DrivePicker_ = wrap('DrivePicker', DrivePicker, { host: { display: 'flex', width: '100%', height: '100%' } });
const ContextMenu_ = wrap('ContextMenu', ContextMenu);
const ConfirmDialog_ = wrap('ConfirmDialog', ConfirmDialog);
const Toast_ = wrap('Toast', Toast);
const Tooltip_ = wrap('Tooltip', Tooltip);
const Icon = wrap('Icon', IconSvelte, { standalone: true, props: p => ({ name: p.name in ICONS ? p.name : 'file', size: Number(p.size ?? 16) }) });
const Logo = wrap('Logo', LogoSvelte, { standalone: true, props: p => ({ size: Number(p.size ?? 32) }) });
const { Button, SubtleButton, Card, Acrylic } = makePrimitives(Icon);

const Tessera = {
  Window,
  Treemap,
  TitleBar: TitleBar_,
  CommandBar: CommandBar_,
  SearchBox: SearchBox_,
  StatusBar: StatusBar_,
  Pane: Pane_,
  DrivePicker: DrivePicker_,
  ContextMenu: ContextMenu_,
  ConfirmDialog: ConfirmDialog_,
  Toast: Toast_,
  Tooltip: Tooltip_,
  Button,
  SubtleButton,
  Card,
  Acrylic,
  Icon,
  Logo,
  /** names, for pickers and docs */
  icons: Object.keys(ICONS),
  scenarios: [...SCENARIOS],
};

(globalThis as unknown as { Tessera: typeof Tessera }).Tessera = Tessera;
export default Tessera;
