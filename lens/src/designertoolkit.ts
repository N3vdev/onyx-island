// Designer toolkit (Pro): an eyedropper (exact pixel color under the
// cursor) and a ruler (distance between two clicks). Both read straight
// from `frameCanvas`'s already-decoded pixels client-side — no Rust
// round-trip needed for either.
//
// Pointer events are intercepted on the SAME canvas lasso.ts listens on,
// in the capture phase: while a designer mode is active this stops the
// event before lasso.ts's own (bubble-phase) listeners ever see it, and
// when no mode is active every event passes through completely untouched.
// lasso.ts needed no changes at all for this to coexist with it.

import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { frameScale } from "./textselection";

export type DesignerMode = "eyedropper" | "measure" | null;

const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;
const lassoCanvas = document.getElementById("lasso") as HTMLCanvasElement;
const designerCanvas = document.getElementById("designer-layer") as HTMLCanvasElement;
const designerCtx = designerCanvas.getContext("2d");
const readout = document.getElementById("designer-readout") as HTMLDivElement;

let mode: DesignerMode = null;
/** Captured (physical) pixels per design pixel — see `FramePayload.dpiScale`. */
let dpiScale = 1;
let measureStart: { x: number; y: number } | null = null;
type PointerPosition = Pick<PointerEvent, "clientX" | "clientY">;
let measurePointer: PointerPosition | null = null;

export function currentDesignerMode(): DesignerMode {
  return mode;
}

export function setDesignerDpiScale(scale: number): void {
  dpiScale = scale > 0 ? scale : 1;
}

/** Sized whenever the frame is (re)painted — same lifecycle as the other
 * frame-pixel layers (see main.ts's `sizeCanvases`). */
export function resizeDesignerLayer(): void {
  designerCanvas.width = frameCanvas.width;
  designerCanvas.height = frameCanvas.height;
  designerCanvas.style.width = `${frameCanvas.width}px`;
  designerCanvas.style.height = `${frameCanvas.height}px`;
  designerCanvas.style.transform = `scale(${frameScale()})`;
}

export function setDesignerMode(next: DesignerMode): void {
  mode = next;
  measureStart = null;
  measurePointer = null;
  designerCtx?.clearRect(0, 0, designerCanvas.width, designerCanvas.height);
  designerCanvas.hidden = !next;
  hideReadout();
  lassoCanvas.style.cursor = next ? "crosshair" : "";
}

function framePoint(event: PointerPosition): { x: number; y: number } {
  const box = lassoCanvas.getBoundingClientRect();
  const scaleX = box.width ? lassoCanvas.width / box.width : 1;
  const scaleY = box.height ? lassoCanvas.height / box.height : 1;
  return {
    x: Math.max(0, Math.min(lassoCanvas.width, (event.clientX - box.left) * scaleX)),
    y: Math.max(0, Math.min(lassoCanvas.height, (event.clientY - box.top) * scaleY)),
  };
}

function hexAt(x: number, y: number): string | null {
  const ctx = frameCanvas.getContext("2d");
  if (!ctx || frameCanvas.width === 0 || frameCanvas.height === 0) return null;
  const px = Math.min(frameCanvas.width - 1, Math.max(0, Math.round(x)));
  const py = Math.min(frameCanvas.height - 1, Math.max(0, Math.round(y)));
  const [r, g, b] = ctx.getImageData(px, py, 1, 1).data;
  return `#${[r, g, b].map((channel) => channel.toString(16).padStart(2, "0")).join("")}`;
}

function showColorReadout(event: PointerEvent, hex: string, suffix = ""): void {
  const swatch = document.createElement("span");
  swatch.className = "swatch";
  swatch.style.background = hex;
  readout.replaceChildren(swatch, document.createTextNode(hex.toUpperCase() + suffix));
  positionReadout(event);
  readout.hidden = false;
}

function showTextReadout(event: PointerPosition, text: string): void {
  readout.replaceChildren(document.createTextNode(text));
  positionReadout(event);
  readout.hidden = false;
}

function positionReadout(event: PointerPosition): void {
  const margin = 18;
  const maxLeft = window.innerWidth - readout.offsetWidth - 8;
  const maxTop = window.innerHeight - readout.offsetHeight - 8;
  readout.style.left = `${Math.min(event.clientX + margin, Math.max(8, maxLeft))}px`;
  readout.style.top = `${Math.min(event.clientY + margin, Math.max(8, maxTop))}px`;
}

function hideReadout(): void {
  readout.hidden = true;
  readout.replaceChildren();
}

function drawMeasureLine(from: { x: number; y: number }, to: { x: number; y: number }): void {
  if (!designerCtx) return;
  designerCtx.clearRect(0, 0, designerCanvas.width, designerCanvas.height);
  // Line width/radius compensate for frameScale() so the stroke reads as a
  // constant visual thickness regardless of monitor DPI, the same reason
  // ocr.rs's own geometry gets a dpi-aware tolerance elsewhere.
  const scale = frameScale() || 1;
  designerCtx.strokeStyle = "#ff5470";
  designerCtx.lineWidth = 2 / scale;
  designerCtx.beginPath();
  designerCtx.moveTo(from.x, from.y);
  designerCtx.lineTo(to.x, to.y);
  designerCtx.stroke();
  designerCtx.fillStyle = "#ff5470";
  for (const point of [from, to]) {
    designerCtx.beginPath();
    designerCtx.arc(point.x, point.y, 4 / scale, 0, Math.PI * 2);
    designerCtx.fill();
  }
}

function formatDistance(from: { x: number; y: number }, to: { x: number; y: number }): string {
  const framePx = Math.hypot(to.x - from.x, to.y - from.y);
  const designPx = framePx / dpiScale;
  const width = Math.round(Math.abs(to.x - from.x) / dpiScale);
  const height = Math.round(Math.abs(to.y - from.y) / dpiScale);
  return `${Math.round(designPx)}px  ·  ${width}×${height}`;
}

function updateMeasurement(event: PointerPosition, constrain: boolean): void {
  if (!measureStart) return;
  // Retain the raw cursor position so releasing Shift restores the diagonal
  // immediately, even when the pointer has not moved.
  measurePointer = { clientX: event.clientX, clientY: event.clientY };
  const point = framePoint(event);
  if (constrain) {
    if (Math.abs(point.x - measureStart.x) >= Math.abs(point.y - measureStart.y)) {
      point.y = measureStart.y;
    } else {
      point.x = measureStart.x;
    }
  }
  drawMeasureLine(measureStart, point);
  showTextReadout(event, formatDistance(measureStart, point));
}

function handlePointerMove(event: PointerEvent): void {
  const point = framePoint(event);
  if (mode === "eyedropper") {
    const hex = hexAt(point.x, point.y);
    if (hex) showColorReadout(event, hex);
  } else if (mode === "measure" && measureStart) {
    updateMeasurement(event, event.shiftKey);
  }
}

function handlePointerDown(event: PointerEvent): void {
  const point = framePoint(event);
  if (mode === "eyedropper") {
    const hex = hexAt(point.x, point.y);
    if (!hex) return;
    void writeText(hex).catch((err) => console.error("eyedropper clipboard write failed", err));
    showColorReadout(event, hex, " copied");
  } else if (mode === "measure") {
    // First click starts the ruler; the second finishes it and leaves the
    // line/readout in place — clicking again starts a fresh measurement
    // rather than requiring a mode toggle for each one.
    if (measureStart) {
      updateMeasurement(event, event.shiftKey);
      measureStart = null;
      measurePointer = null;
    } else {
      measureStart = point;
      updateMeasurement(event, event.shiftKey);
    }
  }
}

function handleMeasureModifier(event: KeyboardEvent): void {
  if (event.key !== "Shift" || event.repeat || mode !== "measure" || !measurePointer) return;
  updateMeasurement(measurePointer, event.shiftKey);
}

window.addEventListener("keydown", handleMeasureModifier);
window.addEventListener("keyup", handleMeasureModifier);
window.addEventListener("blur", () => {
  if (mode === "measure" && measurePointer) updateMeasurement(measurePointer, false);
});

lassoCanvas.addEventListener(
  "pointerdown",
  (event) => {
    if (!mode) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    handlePointerDown(event);
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointermove",
  (event) => {
    if (!mode) return;
    event.stopImmediatePropagation();
    handlePointerMove(event);
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointerup",
  (event) => {
    if (!mode) return;
    event.stopImmediatePropagation();
  },
  { capture: true },
);

lassoCanvas.addEventListener(
  "pointerleave",
  () => {
    if (mode === "eyedropper") hideReadout();
  },
  { capture: true },
);
