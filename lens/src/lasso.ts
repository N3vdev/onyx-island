// Coalesced pointer samples and one paint per frame for either selection tool.

import { LASSO_STYLES, type LassoStyle } from "./themes";

export interface Point { x: number; y: number }
export interface LassoBox { x: number; y: number; width: number; height: number }
export type SelectionTool = "lasso" | "rectangle";

/** Any *open* stroke — one that ends far from where it started, like a
 * diagonal drag across a picture — selects its bounding box, which then
 * snaps to the real edges underneath (rectsnap.rs). A stroke that comes back
 * round to its start is a loop and stays a freehand lasso. "Open" means the
 * start-to-end gap is at least this share of the box's diagonal; straightness
 * doesn't matter, so a wobbly hand-drawn line still counts. */
const SWIPE_OPENNESS = 0.55;
/** Smallest swipe (box diagonal, CSS px) worth turning into a selection. */
const SWIPE_MIN_DIAGONAL = 40;
export interface LassoHandle {
  resize: () => void;
  setTool: (tool: SelectionTool) => boolean;
  /** Swaps the stroke palette/glow (Pro visual pack); default is rainbow. */
  setStyle: (style: LassoStyle) => void;
  convertToRectangle: () => Promise<void>;
  /** Edit mode: slides the stroke you drew onto `target` (the outline snapped
   * to the object's real edges) and leaves it there as the selection's
   * freehand path. Resolves when the animation has finished. */
  snapTo: (target: Point[]) => Promise<void>;
  /** Swaps the settled freehand outline for another one with no animation —
   * used by the Edit-mode brush, which changes the outline stroke by stroke. */
  setPath: (path: Point[]) => void;
  /** Settles the stroke into a rectangle (animating from wherever it is now)
   * — used when a straight swipe has been turned into the region it spans. */
  setRect: (box: LassoBox) => void;
  /** Finishes a selection instantly, as if the whole of `box` had just been
   * drawn corner-to-corner — used by the full-screen shortcut, which has no
   * actual drag to wait for. Goes through the same onStart/onEnd pair a
   * real drag fires, so main.ts's crop/action-bar logic never needs a
   * second copy for this. */
  selectAll: (box: LassoBox) => void;
  startRecognizing: () => void;
  stopRecognizing: () => void;
  settle: (box: LassoBox) => void;
  indicateTextMode: () => void;
  clear: () => void;
  destroy: () => void;
}
/** How a draw ended: the active tool's own shape, or — in Lasso mode — a
 * quick straight swipe, which selects the rectangle it spans. */
export type SelectionGesture = SelectionTool | "swipe";

export interface LassoCallbacks {
  onStart?: () => void;
  onEnd?: (points: Point[], gesture: SelectionGesture) => void;
  onCancel?: () => void;
  onMorph?: (path: Point[] | null) => void;
  /** Exact painted rectangle, including any in-progress transition. */
  onRectangle?: (box: LassoBox) => void;
}

/** One contour for the visible stroke, reveal mask, and copied PNG mask. */
export function traceLassoPath(ctx: CanvasRenderingContext2D, path: Point[], closed = true): void {
  if (!path.length) return;
  ctx.beginPath();
  ctx.moveTo(path[0].x, path[0].y);
  for (let index = 1; index < path.length - 1; index++) {
    ctx.quadraticCurveTo(
      path[index].x, path[index].y,
      (path[index].x + path[index + 1].x) / 2,
      (path[index].y + path[index + 1].y) / 2,
    );
  }
  const last = path[path.length - 1];
  ctx.lineTo(last.x, last.y);
  if (closed) ctx.closePath();
}

// The rounded card a finished stroke settles into. Settings-driven (see
// main.ts's applyTheme) so the stroke, the spotlight hole, the OCR hit
// surface and the saved crop all agree on one shape — they used to be
// separate hardcoded 10/20s, which is why changing them in Settings only
// moved the dimmed hole and left the stroke behind.
let cardPadding = 10;
let cardRadius = 20;

export function setCardShape(padding: number, radius: number): void {
  cardPadding = padding;
  cardRadius = radius;
}

const RESOLVE_MS = 240;
const RECOGNIZING_DELAY_MS = 260;
const MORPH_SAMPLES = 96;

// `collect()` pushes every coalesced pointer sample above a tiny 0.55px
// threshold straight into `points` — dense, but with no noise reduction, so
// a mouse's own sensor jitter (or an unsteady hand) shows up as visible
// wobble once traced. A short trailing average, applied only at render time
// (never to the raw points sent to the backend for hit-testing, or their
// jagged-ness would matter there too), turns that wobble into a fluid line.
const SMOOTHING_WINDOW = 5;

function smoothPath(path: Point[], closed = false): Point[] {
  if (path.length < 3) return path;
  const smoothed: Point[] = new Array(path.length);
  if (closed) {
    // A finished loop has no "end" to trail behind: the trailing window below
    // drags every point back along the path and leaves the last points out of
    // step with the first, which showed up as a little hook where a snapped
    // outline starts and ends. Centred and wrapping around instead, so the
    // seam is as smooth as anywhere else.
    const half = Math.floor(SMOOTHING_WINDOW / 2);
    const count = 2 * half + 1;
    const n = path.length;
    for (let i = 0; i < n; i++) {
      let sumX = 0, sumY = 0;
      for (let k = -half; k <= half; k++) {
        const p = path[(i + k + n) % n];
        sumX += p.x;
        sumY += p.y;
      }
      smoothed[i] = { x: sumX / count, y: sumY / count };
    }
    return smoothed;
  }
  let sumX = 0, sumY = 0;
  for (let i = 0; i < path.length; i++) {
    sumX += path[i].x;
    sumY += path[i].y;
    const dropped = i - SMOOTHING_WINDOW;
    if (dropped >= 0) {
      sumX -= path[dropped].x;
      sumY -= path[dropped].y;
    }
    const count = Math.min(i + 1, SMOOTHING_WINDOW);
    smoothed[i] = { x: sumX / count, y: sumY / count };
  }
  return smoothed;
}

function bounds(points: Point[]): LassoBox {
  let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
  for (const point of points) {
    left = Math.min(left, point.x);
    top = Math.min(top, point.y);
    right = Math.max(right, point.x);
    bottom = Math.max(bottom, point.y);
  }
  return { x: left, y: top, width: right - left, height: bottom - top };
}

function padded(box: LassoBox, padding: number): LassoBox {
  return {
    x: box.x - padding, y: box.y - padding,
    width: box.width + padding * 2, height: box.height + padding * 2,
  };
}

function resampleClosed(path: Point[], count: number): Point[] {
  const lengths = path.map((point, index) => {
    const next = path[(index + 1) % path.length];
    return Math.hypot(next.x - point.x, next.y - point.y);
  });
  const perimeter = lengths.reduce((sum, length) => sum + length, 0);
  if (!perimeter) return Array.from({ length: count }, () => path[0]);
  const samples: Point[] = [];
  let segment = 0;
  let start = 0;
  for (let index = 0; index < count; index++) {
    const distance = perimeter * index / count;
    while (segment < path.length - 1 && start + lengths[segment] < distance) {
      start += lengths[segment++];
    }
    const from = path[segment];
    const to = path[(segment + 1) % path.length];
    const fraction = lengths[segment] ? (distance - start) / lengths[segment] : 0;
    samples.push({ x: from.x + (to.x - from.x) * fraction, y: from.y + (to.y - from.y) * fraction });
  }
  return samples;
}

function roundedPerimeter(box: LassoBox): Point[] {
  const radius = Math.min(cardRadius, box.width / 2, box.height / 2);
  const corners = [
    { x: box.x + box.width - radius, y: box.y + radius, from: -Math.PI / 2 },
    { x: box.x + box.width - radius, y: box.y + box.height - radius, from: 0 },
    { x: box.x + radius, y: box.y + box.height - radius, from: Math.PI / 2 },
    { x: box.x + radius, y: box.y + radius, from: Math.PI },
  ];
  const path: Point[] = [{ x: box.x + radius, y: box.y }];
  for (const corner of corners) {
    for (let step = 0; step <= 12; step++) {
      const angle = corner.from + Math.PI / 2 * step / 12;
      path.push({ x: corner.x + radius * Math.cos(angle), y: corner.y + radius * Math.sin(angle) });
    }
  }
  return resampleClosed(path, MORPH_SAMPLES);
}

function signedArea(path: Point[]): number {
  return path.reduce((sum, point, index) => {
    const next = path[(index + 1) % path.length];
    return sum + point.x * next.y - next.x * point.y;
  }, 0);
}

function alignContours(source: Point[], target: Point[]): Point[] {
  const oriented = signedArea(source) * signedArea(target) < 0 ? target.slice().reverse() : target;
  let bestOffset = 0;
  let bestDistance = Infinity;
  for (let offset = 0; offset < oriented.length; offset++) {
    let distance = 0;
    for (let index = 0; index < source.length; index++) {
      const from = source[index], to = oriented[(index + offset) % oriented.length];
      distance += (from.x - to.x) ** 2 + (from.y - to.y) ** 2;
    }
    if (distance < bestDistance) {
      bestDistance = distance;
      bestOffset = offset;
    }
  }
  return oriented.map((_, index) => oriented[(index + bestOffset) % oriented.length]);
}

export function initLasso(canvas: HTMLCanvasElement, callbacks: LassoCallbacks): LassoHandle {
  const context = canvas.getContext("2d");
  if (!context) throw new Error("lasso canvas has no 2d context");
  const ctx: CanvasRenderingContext2D = context;
  let points: Point[] = [];
  let tool: SelectionTool = "lasso";
  let selectionPath: Point[] | null = null;
  let drawing = false;
  let activePointer: number | null = null;
  let scaleX = 1, scaleY = 1;
  let rect: LassoBox | null = null;
  let transition: { from: LassoBox; to: LassoBox; start: number } | null = null;
  // `finalPath` set = this morph ends as a freehand outline (snapTo) rather
  // than settling into the rounded rectangle convertToRectangle aims for.
  let morph: { from: Point[]; to: Point[]; target: LassoBox; start: number; done: () => void; finalPath?: Point[] } | null = null;
  let textModeUntil = 0;
  let recognizingSince = 0;
  let recognizingTimer: number | null = null;
  let frame = 0;
  let strokeStyle: LassoStyle = LASSO_STYLES.rainbow;

  function requestPaint(): void {
    if (!frame) frame = requestAnimationFrame(paint);
  }

  function readScale(): void {
    const box = canvas.getBoundingClientRect();
    scaleX = box.width ? canvas.width / box.width : 1;
    scaleY = box.height ? canvas.height / box.height : 1;
    requestPaint();
  }

  function collect(event: PointerEvent): void {
    const box = canvas.getBoundingClientRect();
    const coalesced = event.getCoalescedEvents?.();
    for (const sample of coalesced?.length ? coalesced : [event]) {
      const point = {
        x: Math.max(0, Math.min(canvas.width, (sample.clientX - box.left) * scaleX)),
        y: Math.max(0, Math.min(canvas.height, (sample.clientY - box.top) * scaleY)),
      };
      const last = points[points.length - 1];
      if (tool === "rectangle" && points.length) {
        points[1] = point;
      } else if (!last || Math.hypot(point.x - last.x, point.y - last.y) >= 0.55 * scaleX) {
        points.push(point);
      }
    }
  }

  function pathRounded(box: LassoBox): void {
    const radius = Math.min(cardRadius, box.width / 2, box.height / 2);
    const x = box.x, y = box.y, right = x + box.width, bottom = y + box.height;
    ctx.beginPath();
    ctx.moveTo(x + radius, y);
    ctx.arcTo(right, y, right, bottom, radius);
    ctx.arcTo(right, bottom, x, bottom, radius);
    ctx.arcTo(x, bottom, x, y, radius);
    ctx.arcTo(x, y, right, y, radius);
    ctx.closePath();
  }

  function chromaticStroke(box: LassoBox, phase?: number): CanvasGradient {
    const centerX = box.x + box.width / 2;
    const centerY = box.y + box.height / 2;
    const angle = phase === undefined ? Math.PI / 4 : phase * Math.PI * 2 + Math.PI / 4;
    const reach = Math.max(1, Math.hypot(box.width, box.height) / 2);
    const dx = Math.cos(angle) * reach;
    const dy = Math.sin(angle) * reach;
    const gradient = ctx.createLinearGradient(centerX - dx, centerY - dy, centerX + dx, centerY + dy);
    const palette = strokeStyle.palette;
    palette.forEach((color, index) => gradient.addColorStop(index / (palette.length - 1), color));
    return gradient;
  }

  function strokeRainbow(box: LassoBox, energy = 0, phase?: number): void {
    const gradient = chromaticStroke(box, phase);
    ctx.strokeStyle = gradient;
    ctx.globalAlpha *= 0.18;
    ctx.lineWidth = (3.5 + energy * 0.5) * scaleX;
    ctx.shadowColor = strokeStyle.glow;
    ctx.shadowBlur = 4 * scaleX;
    ctx.stroke();
    ctx.globalAlpha /= 0.18;
    ctx.shadowBlur = 0;
    ctx.lineWidth = (2 + 0.2 * energy) * scaleX;
    ctx.stroke();
  }

  function drawFreehand(path: Point[], opacity: number, closed = false): void {
    if (path.length < 2 || opacity <= 0) return;
    ctx.save();
    ctx.globalAlpha = opacity;
    traceLassoPath(ctx, smoothPath(path, closed), closed);
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    strokeRainbow(bounds(path));
    ctx.restore();
  }

  function paint(now: number): void {
    frame = 0;
    let resolveEnergy = 0;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    if (drawing) {
      if (tool === "rectangle" && points.length > 1) {
        ctx.save();
        const box = bounds(points);
        pathRounded(box);
        strokeRainbow(box);
        ctx.restore();
      } else if (tool === "lasso") drawFreehand(points, 1);
      return;
    }
    if (selectionPath) drawFreehand(selectionPath, 1, true);
    if (morph) {
      const current = morph;
      const t = Math.min(1, (now - current.start) / RESOLVE_MS);
      const eased = t * t * (3 - 2 * t);
      const path = current.from.map((point, index) => ({
        x: point.x + (current.to[index].x - point.x) * eased,
        y: point.y + (current.to[index].y - point.y) * eased,
      }));
      drawFreehand(path, 1, true);
      callbacks.onMorph?.(path);
      if (t < 1) requestPaint();
      else {
        if (current.finalPath) {
          selectionPath = current.finalPath;
          rect = null;
        } else {
          rect = current.target;
        }
        morph = null;
        callbacks.onMorph?.(null);
        current.done();
        requestPaint();
      }
      return;
    }
    if (transition) {
      const t = Math.min(1, (now - transition.start) / RESOLVE_MS);
      resolveEnergy = 1 - t;
      const eased = 1 - Math.pow(1 - t, 3);
      const from = transition.from, to = transition.to;
      rect = {
        x: from.x + (to.x - from.x) * eased,
        y: from.y + (to.y - from.y) * eased,
        width: from.width + (to.width - from.width) * eased,
        height: from.height + (to.height - from.height) * eased,
      };
      if (t < 1) requestPaint();
      else transition = null;
    }
    if (!rect) {
      if (selectionPath && now < textModeUntil) {
        requestPaint();
      }
      return;
    }
    ctx.save();
    callbacks.onRectangle?.(rect);
    pathRounded(rect);
    const recognizing = recognizingSince > 0 && now - recognizingSince >= RECOGNIZING_DELAY_MS;
    const phase = recognizing ? ((now - recognizingSince) / 2400) % 1 : undefined;
    strokeRainbow(rect, resolveEnergy, phase);
    if (recognizing) requestPaint();
    if (now < textModeUntil) {
      // A short travelling dash acknowledges the Select Text choice without
      // leaving a permanent animation running over a static screenshot.
      ctx.globalAlpha = 0.78;
      ctx.lineWidth = 1.25 * scaleX;
      ctx.strokeStyle = "#fff";
      ctx.setLineDash([2.5 * scaleX, 6 * scaleX]);
      ctx.lineDashOffset = -(now * 0.08 * scaleX);
      ctx.stroke();
      ctx.setLineDash([]);
      requestPaint();
    }
    ctx.restore();
  }

  function onDown(event: PointerEvent): void {
    if (event.button !== 0 || drawing) return;
    event.preventDefault();
    callbacks.onStart?.();
    drawing = true;
    activePointer = event.pointerId;
    points = [];
    selectionPath = null;
    rect = null;
    transition = null;
    if (morph) morph.done();
    morph = null;
    textModeUntil = 0;
    stopRecognizing();
    canvas.setPointerCapture(event.pointerId);
    collect(event);
    requestPaint();
  }

  function onMove(event: PointerEvent): void {
    if (!drawing || event.pointerId !== activePointer) return;
    event.preventDefault();
    collect(event);
    requestPaint();
  }

  function onUp(event: PointerEvent): void {
    if (!drawing || event.pointerId !== activePointer) return;
    collect(event);
    drawing = false;
    activePointer = null;
    if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
    const box = bounds(points);
    if (points.length < 2 || box.width < 2 * scaleX || box.height < 2 * scaleY) {
      clear();
      callbacks.onCancel?.();
      return;
    }
    const swipe = tool === "lasso" && isSwipe(points, box);
    if (tool === "lasso" && !swipe) selectionPath = points.slice();
    else rect = box;
    requestPaint();
    callbacks.onEnd?.(tool === "lasso" && !swipe ? points.slice() : [
      { x: box.x, y: box.y }, { x: box.x + box.width, y: box.y },
      { x: box.x + box.width, y: box.y + box.height }, { x: box.x, y: box.y + box.height },
    ], swipe ? "swipe" : tool);
  }

  /** A quick, nearly straight, open stroke that spans some area in both
   * directions — a slash across a picture rather than a loop around it. */
  function isSwipe(path: Point[], box: LassoBox): boolean {
    if (path.length < 2) return false;
    const first = path[0];
    const last = path[path.length - 1];
    const gap = Math.hypot(last.x - first.x, last.y - first.y);
    const diagonal = Math.hypot(box.width, box.height);
    // Frame pixels per CSS pixel, so the size limit feels the same on any DPI.
    const unit = Math.max(scaleX, scaleY);
    return diagonal >= SWIPE_MIN_DIAGONAL * unit && gap / Math.max(diagonal, 1) >= SWIPE_OPENNESS;
  }


  function onCancel(event: PointerEvent): void {
    if (event.pointerId === activePointer) {
      clear();
      callbacks.onCancel?.();
    }
  }

  function clear(): void {
    if (frame) cancelAnimationFrame(frame);
    if (activePointer !== null && canvas.hasPointerCapture(activePointer)) {
      canvas.releasePointerCapture(activePointer);
    }
    frame = 0;
    points = [];
    selectionPath = null;
    drawing = false;
    activePointer = null;
    rect = null;
    transition = null;
    if (morph) morph.done();
    morph = null;
    callbacks.onMorph?.(null);
    textModeUntil = 0;
    stopRecognizing();
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  }

  canvas.addEventListener("pointerdown", onDown);
  canvas.addEventListener("pointermove", onMove);
  canvas.addEventListener("pointerup", onUp);
  canvas.addEventListener("pointercancel", onCancel);
  readScale();

  function convertToRectangle(): Promise<void> {
    if (!selectionPath) return Promise.resolve();
    const path = selectionPath;
    selectionPath = null;
    const target = padded(bounds(path), cardPadding);
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      rect = target;
      callbacks.onMorph?.(null);
      requestPaint();
      return Promise.resolve();
    }
    const from = resampleClosed(path, MORPH_SAMPLES);
    const to = alignContours(from, roundedPerimeter(target));
    return new Promise<void>((done) => {
      morph = { from, to, target, start: performance.now(), done };
      requestPaint();
    });
  }

  function setPath(path: Point[]): void {
    if (path.length < 3) return;
    selectionPath = path.slice();
    rect = null;
    morph = null;
    requestPaint();
  }

  function setRect(box: LassoBox): void {
    selectionPath = null;
    morph = null;
    if (rect && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      transition = { from: rect, to: box, start: performance.now() };
    } else {
      transition = null;
    }
    rect = box;
    requestPaint();
  }

  function snapTo(targetPath: Point[]): Promise<void> {
    if (targetPath.length < 3) return Promise.resolve();
    // What's on screen right now: the freehand loop, or — for the rectangle
    // tool — its rounded box.
    const source = selectionPath ?? (rect ? roundedPerimeter(rect) : null);
    if (!source) return Promise.resolve();
    selectionPath = null;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      selectionPath = targetPath;
      rect = null;
      callbacks.onMorph?.(null);
      requestPaint();
      return Promise.resolve();
    }
    const from = resampleClosed(source, MORPH_SAMPLES);
    const to = alignContours(from, resampleClosed(targetPath, MORPH_SAMPLES));
    return new Promise<void>((done) => {
      morph = { from, to, target: bounds(targetPath), start: performance.now(), done, finalPath: targetPath };
      requestPaint();
    });
  }

  function stopRecognizing(): void {
    const wasRecognizing = recognizingSince > 0;
    recognizingSince = 0;
    if (recognizingTimer !== null) window.clearTimeout(recognizingTimer);
    recognizingTimer = null;
    if (wasRecognizing && (rect || selectionPath)) requestPaint();
  }

  return {
    resize: readScale,
    setStyle: (style) => {
      strokeStyle = style;
      requestPaint();
    },
    setTool: (next) => {
      tool = next;
      if (next !== "rectangle" || !selectionPath) return false;
      void convertToRectangle();
      return true;
    },
    convertToRectangle,
    snapTo,
    setPath,
    setRect,
    selectAll: (box) => {
      clear();
      callbacks.onStart?.();
      rect = box;
      requestPaint();
      callbacks.onEnd?.([
        { x: box.x, y: box.y }, { x: box.x + box.width, y: box.y },
        { x: box.x + box.width, y: box.y + box.height }, { x: box.x, y: box.y + box.height },
      ], "rectangle");
    },
    startRecognizing: () => {
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
      if (recognizingTimer !== null) window.clearTimeout(recognizingTimer);
      recognizingSince = performance.now();
      recognizingTimer = window.setTimeout(() => {
        recognizingTimer = null;
        if (recognizingSince) requestPaint();
      }, RECOGNIZING_DELAY_MS);
    },
    stopRecognizing,
    settle: (box) => {
      if (selectionPath) return;
      const target = padded(box, cardPadding);
      if (rect && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
        transition = { from: rect, to: target, start: performance.now() };
      } else {
        rect = target;
        transition = null;
      }
      requestPaint();
    },
    indicateTextMode: () => {
      if ((!rect && !selectionPath) || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
      textModeUntil = performance.now() + 720;
      requestPaint();
    },
    clear,
    destroy: () => {
      clear();
      canvas.removeEventListener("pointerdown", onDown);
      canvas.removeEventListener("pointermove", onMove);
      canvas.removeEventListener("pointerup", onUp);
      canvas.removeEventListener("pointercancel", onCancel);
    },
  };
}
