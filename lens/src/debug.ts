// Ctrl+Shift+D toggles OCR box visualization: word boxes in red, line boxes in blue.

import { listen } from "@tauri-apps/api/event";

interface OcrWord {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface OcrLine {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  words: OcrWord[];
}

interface OcrReadyPayload {
  lines: OcrLine[];
}

const debugCanvas = document.getElementById("debug") as HTMLCanvasElement;
const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;
const ctx = debugCanvas.getContext("2d");

let enabled = false;
let lastLines: OcrLine[] = [];

function render(): void {
  if (!ctx) return;

  debugCanvas.width = frameCanvas.width;
  debugCanvas.height = frameCanvas.height;
  ctx.clearRect(0, 0, debugCanvas.width, debugCanvas.height);

  if (!enabled) return;

  ctx.lineWidth = 1.5;
  for (const line of lastLines) {
    ctx.strokeStyle = "rgba(80, 140, 255, 0.9)";
    ctx.strokeRect(line.x, line.y, line.width, line.height);

    ctx.strokeStyle = "rgba(255, 60, 60, 0.9)";
    for (const word of line.words) {
      ctx.strokeRect(word.x, word.y, word.width, word.height);
    }
  }
}

export function initDebugOverlay(): void {
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "d") {
      e.preventDefault();
      enabled = !enabled;
      console.debug(`OCR debug overlay ${enabled ? "on" : "off"}`);
      render();
    }
  });

  void listen<OcrReadyPayload>("ocr-ready", (event) => {
    lastLines = event.payload.lines;
    console.debug(`ocr-ready: ${lastLines.length} lines`);
    if (enabled) render();
  });
}

export function clearDebugOverlay(): void {
  lastLines = [];
  // Called before the other canvases shrink: render() here would allocate
  // another full-monitor buffer and leave it resident throughout idle.
  debugCanvas.width = debugCanvas.height = 1;
}
