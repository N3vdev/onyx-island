// Translate in place: an opaque plate over each translated paragraph, laid
// out in captured-frame pixels and scaled as a whole like #text-layer, so it
// lines up with the screenshot at any DPI. A separate layer on purpose — the
// selectable .ocr-line elements must stay untouched, since copying reads them.

import { frameScale, type ParagraphBox } from "./textselection";

const layer = document.getElementById("translate-layer") as HTMLDivElement;
const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;

/** Breathing room around each plate, in frame pixels. */
const PLATE_PADDING = 6;
const MIN_FONT_PX = 9;
const MAX_FONT_PX = 44;

let lastTranslations: string[] = [];

/** Shrinks the text until it fits the original paragraph's box, starting
 * from the original line height. Translations often run longer than the
 * source; if even the minimum size overflows, the plate grows downward
 * rather than clipping words. */
function fitText(plate: HTMLDivElement, paragraph: ParagraphBox): void {
  const available = paragraph.height + PLATE_PADDING * 2;
  let size = Math.min(MAX_FONT_PX, Math.max(MIN_FONT_PX, paragraph.lineHeight * 0.82));
  plate.style.fontSize = `${size}px`;
  while (plate.scrollHeight > available + 1 && size > MIN_FONT_PX) {
    size = Math.max(MIN_FONT_PX, size - Math.max(0.5, size * 0.06));
    plate.style.fontSize = `${size}px`;
  }
}

export function showTranslation(paragraphs: ParagraphBox[], translations: string[], lang: string): void {
  clearTranslation();
  layer.style.width = `${frameCanvas.width}px`;
  layer.style.height = `${frameCanvas.height}px`;
  layer.style.transform = `scale(${frameScale()})`;

  paragraphs.forEach((paragraph, index) => {
    const text = translations[index];
    if (!text) return;
    const plate = document.createElement("div");
    plate.className = "translate-plate";
    // Lets the WebView pick correct shaping, quotes and hyphenation.
    plate.lang = lang;
    plate.textContent = text;
    plate.style.left = `${paragraph.x - PLATE_PADDING}px`;
    plate.style.top = `${paragraph.y - PLATE_PADDING}px`;
    plate.style.width = `${paragraph.width + PLATE_PADDING * 2}px`;
    plate.style.minHeight = `${paragraph.height + PLATE_PADDING * 2}px`;
    plate.style.padding = `${PLATE_PADDING}px`;
    plate.style.animationDelay = `${Math.min(index, 8) * 30}ms`;
    layer.appendChild(plate);
    fitText(plate, paragraph);
  });

  lastTranslations = translations;
  layer.hidden = false;
}

export function clearTranslation(): void {
  layer.replaceChildren();
  layer.hidden = true;
  lastTranslations = [];
}

export function translationShown(): boolean {
  return !layer.hidden && lastTranslations.length > 0;
}

/** Translated paragraphs as plain text, blank line between paragraphs. */
export function translatedText(): string {
  return lastTranslations.filter(Boolean).join("\n\n");
}
