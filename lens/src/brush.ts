// Edit-mode brush (Pro): paint over a snapped selection to grow it, or erase
// to shrink it, like Photoshop's lasso/quick-selection brush.
//
// Like designertoolkit.ts, pointer events are intercepted on the lasso canvas
// in the capture phase while the brush is on, so lasso.ts never sees them and
// needed no changes to coexist. A stroke is previewed here and handed to
// `onStroke` when the pointer lifts; main.ts has Rust (snap.rs) merge it into
// the outline and swaps the new, smooth outline in.

import { frameScale } from "./textselection";

export interface BrushStroke {
  add: boolean;
  /** Frame pixels. */
  radius: number;
  points: Array<[number, number]>;
}

const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;
const lassoCanvas = document.getElementById("lasso") as HTMLCanvasElement;
const brushCanvas = document.getElementById("brush-layer") as HTMLCanvasElement;
const ctx = brushCanvas.getContext("2d");

const MIN_SIZE = 6;
const MAX_SIZE = 160;
const ADD_COLOR = "rgba(70, 200, 120, 0.42)";
const ERASE_COLOR = "rgba(255, 84, 112, 0.42)";

let on = false;
/** The Add/Erase buttons; Alt held while painting erases too. */
let erasing = false;
/** Brush radius in on-screen (CSS) pixels, so it feels the same on any DPI. */
let screenRadius = 22;
let stroke: { add: boolean; points: Array<[number, number]> } | null = null;
let hover: { x: number; y: number } | null = null;
let busy = false;
let handler: ((stroke: BrushStroke) => Promise<void>) | null = null;
let onChange: (() => void) | null = null;

export function isBrushOn(): boolean {
  return on;
}

export function isErasing(): boolean {
  return erasing;
}

export function setBrushHandlers(onStroke: (stroke: BrushStroke) => Promise<void>, changed: () => void): void {
  handler = onStroke;
  onChange = changed;
}

/** Sized with the other frame-pixel layers (main.ts `sizeCanvases`). */
export function resizeBrushLayer(): void {
  brushCanvas.width = frameCanvas.width;
  brushCanvas.height = frameCanvas.height;
  brushCanvas.style.width = `${frameCanvas.width}px`;
  brushCanvas.style.height = `${frameCanvas.height}px`;
  brushCanvas.style.transform = `scale(${frameScale()})`;
}

export function setBrushOn(next: boolean): void {
  on = next;
  stroke = null;
  hover = null;
  brushCanvas.hidden = !next;
  lassoCanvas.style.cursor = next ? "none" : "";
  redraw();
  onChange?.();
}

export function setErasing(next: boolean): void {
  erasing = next;
  redraw();
  onChange?.();
}

export function brushSizeLabel(): string {
  return `${Math.round(screenRadius * 2)}px`;
}

function adjustSize(delta: number): void {
  screenRadius = Math.max(MIN_SIZE / 2, Math.min(MAX_SIZE / 2, screenRadius + delta));
  redraw();
  onChange?.();
}

function framePoint(event: PointerEvent): { x: number; y: number } {
  const box = lassoCanvas.getBoundingClientRect();
  const scaleX = box.width ? lassoCanvas.width / box.width : 1;
  const scaleY = box.height ? lassoCanvas.height / box.height : 1;
  return {
    x: Math.max(0, Math.min(lassoCanvas.width, (event.clientX - box.left) * scaleX)),
    y: Math.max(0, Math.min(lassoCanvas.height, (event.clientY - box.top) * scaleY)),
  };
}

function frameRadius(): number {
  return screenRadius / (frameScale() || 1);
}

function redraw(): void {
  if (!ctx) return;
  ctx.clearRect(0, 0, brushCanvas.width, brushCanvas.height);
  if (!on) return;
  const r = frameRadius();
  const scale = frameScale() || 1;
  if (stroke) {
    ctx.save();
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.lineWidth = r * 2;
    ctx.strokeStyle = stroke.add ? ADD_COLOR : ERASE_COLOR;
    ctx.fillStyle = ctx.strokeStyle;
    ctx.beginPath();
    stroke.points.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    if (stroke.points.length === 1) {
      ctx.arc(stroke.points[0][0], stroke.points[0][1], r, 0, Math.PI * 2);
      ctx.fill();
    } else {
      ctx.stroke();
    }
    ctx.restore();
  }
  if (hover) {
    const adding = stroke ? stroke.add : !erasing;
    ctx.save();
    ctx.lineWidth = 1.5 / scale;
    ctx.strokeStyle = "rgba(255, 255, 255, 0.95)";
    ctx.shadowColor = "rgba(0, 0, 0, 0.7)";
    ctx.shadowBlur = 2 / scale;
    ctx.beginPath();
    ctx.arc(hover.x, hover.y, r, 0, Math.PI * 2);
    ctx.stroke();
    // A plus or minus in the middle says which way the brush will go.
    const arm = Math.min(r * 0.45, 7 / scale);
    ctx.beginPath();
    ctx.moveTo(hover.x - arm, hover.y);
    ctx.lineTo(hover.x + arm, hover.y);
    if (adding) {
      ctx.moveTo(hover.x, hover.y - arm);
      ctx.lineTo(hover.x, hover.y + arm);
    }
    ctx.stroke();
    ctx.restore();
  }
}

function finish(): void {
  if (!stroke) return;
  const done = stroke;
  stroke = null;
  redraw();
  if (!handler) return;
  busy = true;
  void handler({ add: done.add, radius: frameRadius(), points: done.points }).finally(() => {
    busy = false;
    redraw();
  });
}

lassoCanvas.addEventListener(
  "pointerdown",
  (event) => {
    if (!on) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (busy || event.button !== 0) return;
    const p = framePoint(event);
    stroke = { add: !(erasing || event.altKey), points: [[p.x, p.y]] };
    hover = p;
    lassoCanvas.setPointerCapture(event.pointerId);
    redraw();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointermove",
  (event) => {
    if (!on) return;
    event.stopImmediatePropagation();
    const p = framePoint(event);
    hover = p;
    if (stroke) {
      const last = stroke.points[stroke.points.length - 1];
      // Thin the path: a point per couple of pixels is plenty for a brush.
      if (Math.hypot(p.x - last[0], p.y - last[1]) >= 1.5 / (frameScale() || 1)) stroke.points.push([p.x, p.y]);
    }
    redraw();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointerup",
  (event) => {
    if (!on) return;
    event.stopImmediatePropagation();
    if (lassoCanvas.hasPointerCapture(event.pointerId)) lassoCanvas.releasePointerCapture(event.pointerId);
    finish();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointercancel",
  (event) => {
    if (!on) return;
    event.stopImmediatePropagation();
    stroke = null;
    redraw();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointerleave",
  () => {
    if (!on || stroke) return;
    hover = null;
    redraw();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "wheel",
  (event) => {
    if (!on) return;
    event.preventDefault();
    adjustSize(event.deltaY < 0 ? 3 : -3);
  },
  { capture: true, passive: false },
);

// [ and ] resize the brush, as in Photoshop.
document.addEventListener("keydown", (event) => {
  if (!on) return;
  if (event.key === "[") adjustSize(-3);
  else if (event.key === "]") adjustSize(3);
});
