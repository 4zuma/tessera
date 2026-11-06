/**
 * Rounded boxes on the GPU: one instanced WebGL2 draw call paints every folder, header
 * band and tile of the map, in order. A 2D canvas fills each rounded rect on its own; at
 * 150 % on a maximized 1440p window that alone held a full repaint to 30 fps on an
 * RTX 4060, while the GPU path draws the same frame at 60.
 *
 * Each instance is a box in device pixels with a corner radius, a fill, an optional
 * inside stroke and an optional bottom clip (for header bands). The fragment shader turns
 * a signed distance into antialiased coverage, so the boxes match the 2D painter's to
 * within a step of edge antialiasing. Text and textures stay on the 2D canvas above.
 */

const VS = `#version 300 es
layout(location=0) in vec2 corner;
layout(location=1) in vec4 rect;
layout(location=2) in vec4 shape;
layout(location=3) in vec4 fill;
layout(location=4) in vec4 stroke;
uniform vec2 view;
out vec2 vP;
flat out vec4 vRect;
flat out vec4 vShape;
flat out vec4 vFill;
flat out vec4 vStroke;
void main() {
  vec2 p = rect.xy - 1.0 + corner * (rect.zw + 2.0);
  vP = p;
  vRect = rect;
  vShape = shape;
  vFill = fill;
  vStroke = stroke;
  gl_Position = vec4(p / view * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
}`;

const FS = `#version 300 es
precision highp float;
in vec2 vP;
flat in vec4 vRect;
flat in vec4 vShape;
flat in vec4 vFill;
flat in vec4 vStroke;
out vec4 o;
float box(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + r;
  return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
void main() {
  vec2 hb = vRect.zw * 0.5;
  float r = min(vShape.x, min(hb.x, hb.y));
  float d = box(vP - vRect.xy - hb, hb, r);
  float clip = clamp(vShape.z - vP.y + 0.5, 0.0, 1.0);
  float sw = vShape.y;
  float outer = clamp(0.5 - d, 0.0, 1.0) * clip;
  float inner = clamp(0.5 - (d + sw), 0.0, 1.0) * clip;
  // the fill reaches the middle of the stroke, as a canvas fill + stroke of one path does
  float fc = sw > 0.0 ? clamp(0.5 - (d + sw * 0.5), 0.0, 1.0) * clip : outer;
  vec4 s = vStroke * (outer - inner);
  o = s + vFill * fc * (1.0 - s.a);
}`;

/** floats per instance: rect, shape, fill, stroke */
const FLOATS = 16;
export const NONE: RGBA = [0, 0, 0, 0];

/** premultiplied */
export type RGBA = readonly [number, number, number, number];

export type GpuMode = 'auto' | 'off' | 'force';

export class BoxBatch {
  private prog: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  private inst: WebGLBuffer | null = null;
  private quad: WebGLBuffer | null = null;
  private uView: WebGLUniformLocation | null = null;
  private data = new Float32Array(4096 * FLOATS);
  private n = 0;
  private colors = new Map<string, RGBA>();
  private probe: CanvasRenderingContext2D | null = null;

  private constructor(
    readonly canvas: HTMLCanvasElement,
    readonly gl: WebGL2RenderingContext,
  ) {
    this.init();
  }

  /**
   * A batch on `canvas`, or null where WebGL2 is missing or would run in software (the
   * 2D canvas is the faster choice there). `force` takes a software context too (tests).
   */
  static create(canvas: HTMLCanvasElement, mode: GpuMode = 'auto'): BoxBatch | null {
    if (mode === 'off') return null;
    try {
      const gl = canvas.getContext('webgl2', {
        alpha: true,
        premultipliedAlpha: true,
        antialias: false,
        depth: false,
        stencil: false,
        preserveDrawingBuffer: false,
        failIfMajorPerformanceCaveat: mode !== 'force',
      });
      if (!gl) return null;
      if (mode === 'auto' && software(gl)) {
        gl.getExtension('WEBGL_lose_context')?.loseContext();
        return null;
      }
      return new BoxBatch(canvas, gl);
    } catch {
      return null;
    }
  }

  /** (re)create the GPU objects, after construction or a restored context */
  init() {
    const gl = this.gl;
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) || 'shader');
      return s;
    };
    const vs = sh(gl.VERTEX_SHADER, VS);
    const fs = sh(gl.FRAGMENT_SHADER, FS);
    const p = gl.createProgram()!;
    gl.attachShader(p, vs);
    gl.attachShader(p, fs);
    gl.linkProgram(p);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) || 'link');
    this.prog = p;
    this.uView = gl.getUniformLocation(p, 'view');
    this.vao = gl.createVertexArray();
    gl.bindVertexArray(this.vao);
    this.quad = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    this.inst = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.inst);
    for (let i = 0; i < 4; i++) {
      gl.enableVertexAttribArray(1 + i);
      gl.vertexAttribPointer(1 + i, 4, gl.FLOAT, false, FLOATS * 4, i * 16);
      gl.vertexAttribDivisor(1 + i, 1);
    }
    gl.bindVertexArray(null);
  }

  /** a CSS colour as premultiplied RGBA, times `alpha` */
  color(css: string, alpha = 1): RGBA {
    let c = this.colors.get(css);
    if (!c) {
      c = this.parse(css);
      if (this.colors.size > 512) this.colors.clear();
      this.colors.set(css, c);
    }
    if (alpha === 1) return c;
    return [c[0] * alpha, c[1] * alpha, c[2] * alpha, c[3] * alpha];
  }

  begin() {
    this.n = 0;
  }

  /**
   * A box in device px with corner radius `r`. A stroke of width `sw` runs along the inside
   * of the edge with the fill reaching half way into it; `clipY` cuts off what lies below.
   */
  push(x: number, y: number, w: number, h: number, r: number, fill: RGBA, stroke: RGBA | null = null, sw = 0, clipY = 1e9) {
    if ((this.n + 1) * FLOATS > this.data.length) {
      const d = new Float32Array(this.data.length * 2);
      d.set(this.data);
      this.data = d;
    }
    const d = this.data;
    const s = stroke ?? NONE;
    let i = this.n++ * FLOATS;
    d[i++] = x; d[i++] = y; d[i++] = w; d[i++] = h;
    d[i++] = r; d[i++] = stroke ? sw : 0; d[i++] = clipY; d[i++] = 0;
    d[i++] = fill[0]; d[i++] = fill[1]; d[i++] = fill[2]; d[i++] = fill[3];
    d[i++] = s[0]; d[i++] = s[1]; d[i++] = s[2]; d[i++] = s[3];
  }

  /** clear the canvas and draw everything pushed since begin() */
  draw() {
    const gl = this.gl;
    if (gl.isContextLost()) return;
    const W = this.canvas.width;
    const H = this.canvas.height;
    gl.viewport(0, 0, W, H);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!this.n) return;
    gl.useProgram(this.prog);
    gl.uniform2f(this.uView, W, H);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.inst);
    gl.bufferData(gl.ARRAY_BUFFER, this.data.subarray(0, this.n * FLOATS), gl.STREAM_DRAW);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.n);
    gl.bindVertexArray(null);
  }

  /** clear the canvas (the 2D painter has taken over) */
  clear() {
    const gl = this.gl;
    if (gl.isContextLost()) return;
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
  }

  dispose() {
    const gl = this.gl;
    if (!gl.isContextLost()) {
      gl.deleteBuffer(this.inst);
      gl.deleteBuffer(this.quad);
      gl.deleteVertexArray(this.vao);
      gl.deleteProgram(this.prog);
    }
    this.prog = this.vao = this.inst = this.quad = null;
  }

  /** any CSS colour, read back through a 2D context so it arrives as #rrggbb or rgba() */
  private parse(css: string): RGBA {
    let s = css.trim();
    if (!/^#[0-9a-f]{6}$/i.test(s) && !/^rgba?\(/i.test(s)) {
      this.probe ??= document.createElement('canvas').getContext('2d');
      if (this.probe) {
        this.probe.fillStyle = '#000';
        this.probe.fillStyle = s;
        s = String(this.probe.fillStyle);
      }
    }
    let r = 0, g = 0, b = 0, a = 1;
    if (s[0] === '#') {
      let h = s.slice(1);
      if (h.length === 3 || h.length === 4) h = h.split('').map(c => c + c).join('');
      const n = parseInt(h.slice(0, 6), 16);
      r = (n >> 16) & 255;
      g = (n >> 8) & 255;
      b = n & 255;
      if (h.length === 8) a = parseInt(h.slice(6), 16) / 255;
    } else {
      const m = s.match(/rgba?\(([^)]+)\)/i);
      if (m) {
        const p = m[1].split(/[\s,/]+/).filter(Boolean).map(v => parseFloat(v));
        [r, g, b] = p;
        if (p.length > 3) a = p[3];
      }
    }
    // the 2D canvas keeps 8 bits of alpha; matching it keeps translucent fills identical
    a = Math.round(Math.min(1, Math.max(0, a)) * 255) / 255;
    return [(r / 255) * a, (g / 255) * a, (b / 255) * a, a];
  }
}

/**
 * A WebGL that runs on the CPU (SwiftShader, WARP in a VM or a remote session, llvmpipe):
 * there the 2D canvas is several times faster. Not every browser reports such a context
 * as a major performance caveat, so the renderer's name is checked as well.
 */
function software(gl: WebGL2RenderingContext) {
  const ext = gl.getExtension('WEBGL_debug_renderer_info');
  const name = String(gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER) ?? '');
  return /swiftshader|llvmpipe|softpipe|basic render|software/i.test(name);
}
