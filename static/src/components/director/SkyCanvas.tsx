import { useEffect, useRef } from 'react';
import type { DirectorSkyPosition } from '../../api/directorTypes';
import { pixelScale, type Stage, type StageView } from './framingModel';

/** A survey image the server rendered: stereographic about its center,
 *  north up and east left, `fov` degrees across its width. */
export interface SkyTileImage {
  key: string;
  url: string;
  center: DirectorSkyPosition;
  fov: number;
  width: number;
  height: number;
}

/** Tiles drawn at once: the view's own, and the ones around and above it
 *  from earlier, so a pan or a zoom step has a picture before its tile lands. */
export const MAX_SKY_TILES = 6;

const DEG = Math.PI / 180;

/** Unit vectors at a sky position: toward it, east, and north. */
function frameOf(position: DirectorSkyPosition): [number[], number[], number[]] {
  const a = position.ra_degrees * DEG;
  const d = position.dec_degrees * DEG;
  return [
    [Math.cos(d) * Math.cos(a), Math.cos(d) * Math.sin(a), Math.sin(d)],
    [-Math.sin(a), Math.cos(a), 0],
    [-Math.sin(d) * Math.cos(a), -Math.sin(d) * Math.sin(a), Math.cos(d)],
  ];
}

const VERTEX = 'attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }';

/** Every pixel of the stage looks along a direction on the sky, found by
 *  undoing the stage's stereographic projection about the view center; that
 *  direction is then projected into each tile's own plane and sampled where
 *  it falls inside. Tiles go coarse to fine, so the finest picture wins. The
 *  same arithmetic the server used to render the tile runs here in reverse,
 *  so the picture sits under the grid and the rectangle exactly, at every
 *  zoom and turn, and a newly landed tile changes nothing but detail. */
function fragmentSource(count: number): string {
  const perTile = Array.from({ length: count }, (_, i) => `
  if (${i} < u_count) {
    float d = dot(v, u_t0[${i}]);
    if (d > 0.0) {
      float k = 2.0 / (1.0 + d);
      vec2 q = vec2(k * dot(v, u_t1[${i}]), k * dot(v, u_t2[${i}])) / DEG;
      vec2 px = u_tsize[${i}] * 0.5 - q / u_tscale[${i}];
      if (px.x >= 0.0 && px.y >= 0.0 && px.x <= u_tsize[${i}].x && px.y <= u_tsize[${i}].y) {
        color = texture2D(u_tex${i}, px / u_tsize[${i}]);
      }
    }
  }`).join('');
  return `
precision highp float;
const float DEG = 0.017453292519943295;
uniform vec2 u_canvas;
uniform vec2 u_stage;
uniform float u_scale;
uniform float u_rotation;
uniform vec3 u_c;
uniform vec3 u_e;
uniform vec3 u_n;
uniform int u_count;
uniform vec3 u_t0[${count}];
uniform vec3 u_t1[${count}];
uniform vec3 u_t2[${count}];
uniform vec2 u_tsize[${count}];
uniform float u_tscale[${count}];
${Array.from({ length: count }, (_, i) => `uniform sampler2D u_tex${i};`).join('\n')}
void main() {
  vec2 st = vec2(gl_FragCoord.x / u_canvas.x * u_stage.x, (u_canvas.y - gl_FragCoord.y) / u_canvas.y * u_stage.y);
  vec2 win = vec2((u_stage.x * 0.5 - st.x) * u_scale, (u_stage.y * 0.5 - st.y) * u_scale);
  float cr = cos(-u_rotation);
  float sr = sin(-u_rotation);
  vec2 pl = vec2(win.x * cr - win.y * sr, win.x * sr + win.y * cr) * DEG;
  float rho = length(pl);
  vec3 v = u_c;
  if (rho > 1e-12) {
    float c = 2.0 * atan(rho * 0.5);
    vec2 u = pl / rho;
    v = cos(c) * u_c + sin(c) * (u.x * u_e + u.y * u_n);
  }
  vec4 color = vec4(0.0, 0.0, 0.0, 0.0);${perTile}
  gl_FragColor = color;
}`;
}

interface Program {
  gl: WebGLRenderingContext;
  program: WebGLProgram;
  uniforms: Record<string, WebGLUniformLocation | null>;
}

function compile(gl: WebGLRenderingContext, type: number, source: string): WebGLShader | null {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) { gl.deleteShader(shader); return null; }
  return shader;
}

function setUp(canvas: HTMLCanvasElement): Program | null {
  // A browser without WebGL at all (and jsdom in tests) is told so without asking the canvas.
  if (typeof WebGLRenderingContext === 'undefined') return null;
  const gl = canvas.getContext('webgl', { alpha: true, antialias: false, premultipliedAlpha: true, preserveDrawingBuffer: false });
  if (!gl) return null;
  const vertex = compile(gl, gl.VERTEX_SHADER, VERTEX);
  const fragment = compile(gl, gl.FRAGMENT_SHADER, fragmentSource(MAX_SKY_TILES));
  const program = gl.createProgram();
  if (!vertex || !fragment || !program) return null;
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
  gl.useProgram(program);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
  const position = gl.getAttribLocation(program, 'a_pos');
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
  const names = ['u_canvas', 'u_stage', 'u_scale', 'u_rotation', 'u_c', 'u_e', 'u_n', 'u_count', 'u_t0', 'u_t1', 'u_t2', 'u_tsize', 'u_tscale', ...Array.from({ length: MAX_SKY_TILES }, (_, i) => `u_tex${i}`)];
  const uniforms = Object.fromEntries(names.map(name => [name, gl.getUniformLocation(program, name)]));
  return { gl, program, uniforms };
}

export interface SkyCanvasProps {
  tiles: SkyTileImage[];
  view: StageView;
  viewFov: number;
  stage: Stage;
  /** Called once when the browser has no WebGL, so the caller can fall back. */
  onUnsupported?: () => void;
  /** Called with true when the GPU takes the canvas's context away, and
   *  with false once it is back, so the caller can draw something else meanwhile. */
  onLost?: (lost: boolean) => void;
  className?: string;
}

/** The survey picture under the framing stage, re-projected on the GPU for
 *  every frame from the tiles fetched so far. */
export default function SkyCanvas({ tiles, view, viewFov, stage, onUnsupported, onLost, className }: SkyCanvasProps) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const program = useRef<Program | null | undefined>(undefined);
  const textures = useRef(new Map<string, { texture: WebGLTexture; width: number; height: number }>());
  const loading = useRef(new Map<string, Promise<void>>());
  // Counts contexts: an image that decodes after its context went away
  // must not be uploaded into the next one.
  const generation = useRef(0);
  const frame = useRef(0);
  const latest = useRef({ tiles, view, viewFov, stage, onUnsupported, onLost });
  latest.current = { tiles, view, viewFov, stage, onUnsupported, onLost };

  const draw = () => {
    const element = canvas.current;
    const p = program.current;
    if (!element || !p) return;
    const { gl, uniforms } = p;
    const { tiles, view, viewFov, stage } = latest.current;
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    const width = Math.max(1, Math.round(element.clientWidth * ratio));
    const height = Math.max(1, Math.round(element.clientHeight * ratio));
    if (element.width !== width || element.height !== height) { element.width = width; element.height = height; }
    gl.viewport(0, 0, width, height);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    const ready = tiles.filter(tile => textures.current.has(tile.key)).sort((a, b) => b.fov - a.fov).slice(-MAX_SKY_TILES);
    gl.uniform2f(uniforms.u_canvas, width, height);
    gl.uniform2f(uniforms.u_stage, stage.width, stage.height);
    gl.uniform1f(uniforms.u_scale, pixelScale(viewFov, stage));
    gl.uniform1f(uniforms.u_rotation, view.rotation * DEG);
    // The stage's window may sit off the plane's center; the framing view keeps it there.
    const center = view.offset[0] === 0 && view.offset[1] === 0 ? view.anchor : view.anchor;
    const [c, e, n] = frameOf(center);
    gl.uniform3fv(uniforms.u_c, c); gl.uniform3fv(uniforms.u_e, e); gl.uniform3fv(uniforms.u_n, n);
    gl.uniform1i(uniforms.u_count, ready.length);
    const t0: number[] = []; const t1: number[] = []; const t2: number[] = []; const sizes: number[] = []; const scales: number[] = [];
    ready.forEach((tile, i) => {
      const [tc, te, tn] = frameOf(tile.center);
      t0.push(...tc); t1.push(...te); t2.push(...tn);
      const entry = textures.current.get(tile.key)!;
      sizes.push(entry.width, entry.height);
      scales.push(tile.fov / entry.width);
      gl.activeTexture(gl.TEXTURE0 + i);
      gl.bindTexture(gl.TEXTURE_2D, entry.texture);
      gl.uniform1i(uniforms[`u_tex${i}`], i);
    });
    for (let i = ready.length; i < MAX_SKY_TILES; i += 1) { t0.push(0, 0, 1); t1.push(1, 0, 0); t2.push(0, 1, 0); sizes.push(1, 1); scales.push(1); }
    gl.uniform3fv(uniforms.u_t0, t0); gl.uniform3fv(uniforms.u_t1, t1); gl.uniform3fv(uniforms.u_t2, t2);
    gl.uniform2fv(uniforms.u_tsize, sizes); gl.uniform1fv(uniforms.u_tscale, scales);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    element.dataset.tiles = String(ready.length);
  };
  const schedule = () => { cancelAnimationFrame(frame.current); frame.current = requestAnimationFrame(draw); };

  // Upload each new tile once its image has decoded; drop textures of tiles that left.
  const upload = () => {
    const p = program.current;
    if (!p) return;
    const { gl } = p;
    const { tiles } = latest.current;
    const started = generation.current;
    const keep = new Set(tiles.map(tile => tile.key));
    for (const [key, entry] of textures.current) {
      if (!keep.has(key)) { gl.deleteTexture(entry.texture); textures.current.delete(key); }
    }
    for (const tile of tiles) {
      if (textures.current.has(tile.key) || loading.current.has(tile.key)) continue;
      const image = new Image();
      image.decoding = 'async';
      image.src = tile.url;
      const task = image.decode().then(() => {
        if (generation.current !== started || program.current !== p || !latest.current.tiles.some(t => t.key === tile.key)) return;
        const texture = gl.createTexture();
        if (!texture) return;
        gl.bindTexture(gl.TEXTURE_2D, texture);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB, gl.RGB, gl.UNSIGNED_BYTE, image);
        textures.current.set(tile.key, { texture, width: image.naturalWidth, height: image.naturalHeight });
        schedule();
      }).catch(() => undefined).finally(() => { if (generation.current === started) loading.current.delete(tile.key); });
      loading.current.set(tile.key, task);
    }
    schedule();
  };
  /** Let go of every texture and the program, in a context that still holds them. */
  const release = () => {
    const p = program.current;
    if (p && !p.gl.isContextLost()) {
      for (const entry of textures.current.values()) p.gl.deleteTexture(entry.texture);
      p.gl.deleteProgram(p.program);
    }
    textures.current.clear();
    loading.current.clear();
    generation.current += 1;
  };

  useEffect(() => {
    const element = canvas.current;
    if (!element || program.current !== undefined) return;
    program.current = setUp(element);
    if (!program.current) { latest.current.onUnsupported?.(); return; }
    const observer = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(schedule) : null;
    observer?.observe(element);
    // A context the GPU takes away takes its textures with it. Asking for it
    // back (preventDefault) lets the browser restore it; the program is then
    // built again and the tiles uploaded anew.
    const lost = (event: Event) => {
      event.preventDefault();
      cancelAnimationFrame(frame.current);
      release();
      program.current = null;
      latest.current.onLost?.(true);
    };
    const restored = () => {
      program.current = setUp(element);
      if (!program.current) { latest.current.onUnsupported?.(); return; }
      upload();
      latest.current.onLost?.(false);
    };
    element.addEventListener('webglcontextlost', lost);
    element.addEventListener('webglcontextrestored', restored);
    return () => {
      observer?.disconnect();
      cancelAnimationFrame(frame.current);
      element.removeEventListener('webglcontextlost', lost);
      element.removeEventListener('webglcontextrestored', restored);
      const gl = program.current?.gl;
      release();
      program.current = undefined;
      // A browser keeps only a few contexts, so a canvas that leaves the
      // page gives its own back at once. React may put the same canvas
      // straight back (development mounts twice), so only a canvas still
      // gone a moment later lets go of it.
      if (gl) setTimeout(() => { if (!element.isConnected) gl.getExtension('WEBGL_lose_context')?.loseContext(); }, 0);
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { upload(); }, [tiles]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { schedule(); }, [view, viewFov, stage]); // eslint-disable-line react-hooks/exhaustive-deps

  return <canvas ref={canvas} className={className} data-testid="framing-sky" data-view={`${view.anchor.ra_degrees.toFixed(4)},${view.anchor.dec_degrees.toFixed(4)},${viewFov.toFixed(3)},${view.rotation.toFixed(1)}`} aria-hidden="true" />;
}
