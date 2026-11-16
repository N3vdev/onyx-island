// Redact (Pro): an opaque box over each detected sensitive span, laid out
// in captured-frame pixels and scaled as a whole, same trick as
// #text-layer/#translate-layer. A separate layer so it can stack above both
// of those — the box must stay on top of a translation plate too, since
// hiding the sensitive text is the entire point.

import { frameScale } from "./textselection";

export interface RedactBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

const layer = document.getElementById("redact-layer") as HTMLDivElement;
const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;

/** Breathing room around each box, in frame pixels — covers OCR's own
 * bounding-box slack so a redacted word's edges don't peek out. */
const BOX_PADDING = 3;

let lastBoxes: RedactBox[] = [];

export function showRedactions(boxes: RedactBox[]): void {
  clearRedactions();
  if (boxes.length === 0) return;
  layer.style.width = `${frameCanvas.width}px`;
  layer.style.height = `${frameCanvas.height}px`;
  layer.style.transform = `scale(${frameScale()})`;

  for (const box of boxes) {
    const cover = document.createElement("div");
    cover.className = "redact-box";
    cover.style.left = `${box.x - BOX_PADDING}px`;
    cover.style.top = `${box.y - BOX_PADDING}px`;
    cover.style.width = `${box.width + BOX_PADDING * 2}px`;
    cover.style.height = `${box.height + BOX_PADDING * 2}px`;
    layer.appendChild(cover);
  }

  lastBoxes = boxes;
  layer.hidden = false;
}

export function clearRedactions(): void {
  layer.replaceChildren();
  layer.hidden = true;
  lastBoxes = [];
}

export function redactionsActive(): boolean {
  return !layer.hidden && lastBoxes.length > 0;
}

/** In frame pixels, padded the same as the rendered boxes — for baking the
 * cover into the saved image crop (see main.ts's saveImageSelection). */
export function redactedBoxesForExport(): RedactBox[] {
  return lastBoxes.map((box) => ({
    x: box.x - BOX_PADDING,
    y: box.y - BOX_PADDING,
    width: box.width + BOX_PADDING * 2,
    height: box.height + BOX_PADDING * 2,
  }));
}
