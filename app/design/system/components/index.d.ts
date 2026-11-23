/**
 * Tessera's design-system components: window.Tessera.<Name>, React 18 components that mount
 * the app's real Svelte components on sample data. Generated from github.com/4zuma/tessera
 * (app/design); change the app, not these.
 */

/** A named state of the app, reached through the app's own code. */
export type Scenario =
  | 'map' //       the map of the sample C: drive, nothing selected
  | 'picker' //    the drive picker (before the first scan)
  | 'scanning' //  a scan frozen part way: "not scanned yet" area, Stop button
  | 'selected' //  the biggest file selected, its details in the pane
  | 'folder' //    the biggest folder selected
  | 'menu' //      Tessera's context menu open on the biggest file
  | 'confirm' //   the "Delete permanently?" dialog
  | 'search' //    a search (`query`) dimming everything it does not match
  | 'toast' //     a file just moved to the Recycle Bin, with Undo
  | 'tooltip'; //  the tooltip over a tile

/**
 * Props every app component takes. They set the page, not the component: one app runs per
 * page, so the last component to set a value wins, and a component that leaves one out
 * changes nothing.
 */
export interface PageProps {
  scenario?: Scenario;
  /** follows the page's data-theme when left out */
  scheme?: 'light' | 'dark';
  /** UI language; the app follows Windows: English or Simplified Chinese */
  lang?: 'en' | 'zh-CN';
  /** what the tiles are coloured by, as the pane's "By type" / "By age" switch (app: type) */
  colorBy?: 'type' | 'age';
  /** the search box text for scenario "search" (default "*.vhdx") */
  query?: string;
  /** treemap: inset of children inside an open folder, px (app: 3) */
  pad?: number;
  /** treemap: gap between sibling tiles, px (app: 2) */
  gap?: number;
  /** treemap: open folder title band, px (app: 21) */
  header?: number;
  /** treemap: narrowest folder that gets a title band, px (app: 60) */
  minHeaderW?: number;
  /** treemap: side of the smallest square a folder must exceed to open, px (app: 110) */
  minArea?: number;
  /** treemap: thinnest tile either way; thinner ones join the "N small items" tile, px (app: 24) */
  minSide?: number;
  /** treemap: narrowest folder tile; narrower ones join the "N small items" tile, px (app: 44) */
  minWidth?: number;
  /** treemap: narrowest tile with a label, px (app: 44) */
  labelMinW?: number;
  /** treemap: lowest tile with a label, px (app: 20) */
  labelMinH?: number;
  /** treemap: file and closed-folder corner radius, px (app: 3) */
  radius?: number;
  /** treemap: open-folder corner radius, px (app: 4) */
  folderRadius?: number;
  /**
   * treemap: how folders are drawn. "classic" as shipped (app). "band": the 1.3.1 candidate,
   * one frame for every folder, a closed folder's colour as a band along its bottom; pad and
   * header then follow from the band props and are ignored.
   */
  folderLook?: 'classic' | 'band';
  /** band look: white margin round a folder's content on all four sides, px (4) */
  white?: number;
  /** band look: the title band's fill runs on round the sides and bottom, 1 px inside the outline ("on") */
  ring?: 'on' | 'off';
  /** band look: height of a closed folder's colour band, px (5) */
  foot?: number;
  /** band look: the colour band, 0 = the file colour, 1-2 lighter, 3 = the 1.3.0 closed-folder tint (0) */
  tone?: number;
  /** band look: at the seams where the side ring meets the colour band, the colour runs up the ring, fading out over this many px; 0 = a hard edge (0) */
  fade?: number;
  /** band look: "on": a closed folder's size always sits in its white area, and one too small for it has no white area (grey down to its colour band) ("off") */
  compact?: 'on' | 'off';
  /** band look: the least white area a folder gets, px; the folder minimum height (band + this + foot + 1) follows. 18 holds a closed folder's size line (6) */
  minWhite?: number;
  /** band look: every folder gets room for its own size label, on one line or the number over the unit; its least width and height follow the label (minWidth and minWhite no longer apply to folders) ("off") */
  fitSize?: 'on' | 'off';
  /** the host element's style: where the component sits in its slot */
  style?: Record<string, string | number>;
}

export interface WindowProps extends PageProps {
  /** "window": fills its slot as a Windows 11 window (default). "desk": the browser preview's window on a desk. */
  frame?: 'window' | 'desk';
}
export type TreemapProps = PageProps;
export type TitleBarProps = PageProps;
export interface CommandBarProps extends PageProps {
  /** the details-pane toggle shows pressed (default true) */
  paneOpen?: boolean;
  /** put the search box in the command bar (only where there is no custom title bar) */
  search?: boolean;
}
export type SearchBoxProps = PageProps;
export type StatusBarProps = PageProps;
export type PaneProps = PageProps;
export type DrivePickerProps = PageProps;
export type ContextMenuProps = PageProps;
export type ConfirmDialogProps = PageProps;
export type ToastProps = PageProps;
export type TooltipProps = PageProps;

export type IconName =
  | 'drive' | 'folder' | 'file' | 'refresh' | 'stop' | 'up' | 'back' | 'trash' | 'purge' | 'sweep' | 'copy' | 'reveal' | 'search' | 'zoom'
  | 'lock' | 'undo' | 'chev' | 'chevDown' | 'check' | 'cloud' | 'warn' | 'info' | 'shield' | 'pane' | 'dismiss' | 'more' | 'download' | 'edit' | 'tiles';

export interface IconProps {
  name: IconName;
  /** px, default 16 */
  size?: number;
}
export interface LogoProps {
  /** px, default 32 (the artwork is drawn for 16) */
  size?: number;
}

export interface ButtonProps {
  /** standard (default), accent (the one main action), danger (permanent delete) */
  variant?: 'standard' | 'accent' | 'danger';
  icon?: IconName;
  disabled?: boolean;
  title?: string;
  onClick?: () => void;
  children?: unknown;
}
export interface SubtleButtonProps {
  icon?: IconName;
  /** tooltip, and the accessible name when there is no text */
  label?: string;
  pressed?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  children?: unknown;
}
export interface CardProps {
  /** px, default 16 */
  padding?: number;
  children?: unknown;
}
export interface AcrylicProps {
  /** px, default 4 */
  padding?: number;
  /** px, default 8 */
  radius?: number;
  children?: unknown;
}

export declare const Window: (p: WindowProps) => unknown;
export declare const Treemap: (p: TreemapProps) => unknown;
export declare const TitleBar: (p: TitleBarProps) => unknown;
export declare const CommandBar: (p: CommandBarProps) => unknown;
export declare const SearchBox: (p: SearchBoxProps) => unknown;
export declare const StatusBar: (p: StatusBarProps) => unknown;
export declare const Pane: (p: PaneProps) => unknown;
export declare const DrivePicker: (p: DrivePickerProps) => unknown;
export declare const ContextMenu: (p: ContextMenuProps) => unknown;
export declare const ConfirmDialog: (p: ConfirmDialogProps) => unknown;
export declare const Toast: (p: ToastProps) => unknown;
export declare const Tooltip: (p: TooltipProps) => unknown;
export declare const Button: (p: ButtonProps) => unknown;
export declare const SubtleButton: (p: SubtleButtonProps) => unknown;
export declare const Card: (p: CardProps) => unknown;
export declare const Acrylic: (p: AcrylicProps) => unknown;
export declare const Icon: (p: IconProps) => unknown;
export declare const Logo: (p: LogoProps) => unknown;
