/** What the design runtime tells the mock backend before (and during) a preview. */
export interface DesignConfig {
  /**
   * Freeze the next scan at this share (0..1) and keep it "running", for the scanning state.
   * null: scans finish at once, so the map is there as soon as the preview opens.
   */
  freezeAt: number | null;
  /** UI language for the mock platform (`en`, `zh-CN`); null follows the browser */
  lang: string | null;
}

export const config: DesignConfig = { freezeAt: null, lang: null };
