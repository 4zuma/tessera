/**
 * React shells around the real Svelte components. Claude Design mounts components with
 * React 18 (window.React, supplied by the page); each shell renders one empty <div> and mounts
 * the Svelte component into it, so what shows is the app's own markup, styles and behaviour.
 */
import { mount, unmount, type Component } from 'svelte';
import AcrylicDefs from './AcrylicDefs.svelte';
import { reactive } from './reactive.svelte';
import { apply, boot, prime, type PageProps } from './runtime';

/* eslint-disable @typescript-eslint/no-explicit-any */
type Props = Record<string, any>;

// the page supplies React 18 as a global; the bundle never carries its own copy
export const R = (): any => {
  const React = (globalThis as any).React;
  if (!React) throw new Error('Tessera: window.React is missing (load React 18 before bundle.js)');
  return React;
};

const PAGE_KEYS: (keyof PageProps)[] = [
  'scenario', 'scheme', 'lang', 'colorBy', 'query', 'pad', 'gap', 'header', 'minHeaderW', 'minArea', 'minSide', 'minWidth', 'labelMinW', 'labelMinH', 'radius', 'folderRadius',
];

/** the page props that are words; the rest are px and may come in as strings from markup */
const WORD_KEYS = new Set<keyof PageProps>(['scenario', 'scheme', 'lang', 'colorBy', 'query']);

function pageProps(p: Props): PageProps {
  const out: Props = {};
  for (const k of PAGE_KEYS) if (p[k] !== undefined && p[k] !== null && p[k] !== '') out[k] = typeof p[k] === 'string' && !WORD_KEYS.has(k) ? Number(p[k]) : p[k];
  return out as PageProps;
}

let defs = false;
function ensureDefs() {
  if (defs || document.getElementById('acrylic')) return;
  defs = true;
  const el = document.createElement('div');
  el.setAttribute('data-tessera', 'acrylic-defs');
  document.body.appendChild(el);
  mount(AcrylicDefs, { target: el });
}

export interface WrapOpts {
  /** the host <div>'s own style: how the component sits in the slot it was given */
  host?: Record<string, string | number>;
  /** the Svelte component's props from the React props */
  props?: (p: Props) => Props;
  /** the whole window: App.svelte boots the app and draws the acrylic filter itself */
  window?: boolean;
  /** extra attributes on the host <div> */
  attrs?: (p: Props) => Record<string, string>;
  /** needs no app (icons, the logo): never boots it */
  standalone?: boolean;
}

export function wrap(name: string, Comp: Component<any>, opts: WrapOpts = {}) {
  function Shell(props: Props) {
    const React = R();
    const ref = React.useRef(null);
    const live = React.useRef(null);
    const page = pageProps(props);
    if (!live.current && !opts.standalone) prime(page);
    React.useEffect(() => {
      const state = reactive(opts.props ? opts.props(props) : {});
      const inst = mount(Comp, { target: ref.current, props: state });
      if (!opts.window && !opts.standalone) {
        ensureDefs();
        void boot();
      }
      live.current = { inst, state };
      return () => {
        void unmount(inst);
        live.current = null;
      };
    }, []);
    React.useEffect(() => {
      const l = live.current;
      if (l && opts.props) Object.assign(l.state, opts.props(props));
      if (!opts.standalone) void apply(page);
    });
    const style = { ...(opts.host ?? { display: 'contents' }), ...(props.style ?? {}) };
    // a standalone piece (an icon, the logo) sits inside the caller's markup and takes its
    // colour and font from there, as the app's own icons take the button's
    return React.createElement('div', { ref, className: opts.standalone ? 'tessera-inline' : 'tessera-host', 'data-tessera': name, style, ...(opts.attrs?.(props) ?? {}) });
  }
  Shell.displayName = name;
  return Shell;
}
