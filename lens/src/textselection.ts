// OCR text stays transparent over the captured image. A line is one text
// node, so native dragging, double-click, and keyboard selection can move
// through continuous text instead of hopping between absolute word nodes.

interface OcrWordPayload {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface OcrLinePayload {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  words: OcrWordPayload[];
}

export interface SelectionPayload {
  kind: "block" | "line";
  x: number;
  y: number;
  width: number;
  height: number;
  lines: OcrLinePayload[];
}

const textLayer = document.getElementById("text-layer") as HTMLDivElement;
const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;
const measureContext = document.createElement("canvas").getContext("2d");

/** Breathing room around a selection card. Live (a setting), so importers
 * read it through the module binding — never copy it into a constant. */
export let CARD_PADDING = 10;

export function setCardPadding(px: number): void {
  CARD_PADDING = px;
}

/** CSS pixels per captured pixel, measured from the rendered frame. */
export function frameScale(): number {
  return frameCanvas.width > 0 ? frameCanvas.getBoundingClientRect().width / frameCanvas.width : 1;
}

function lineText(line: OcrLinePayload): string {
  return line.words.length > 0 ? line.words.map((word) => word.text).join(" ") : line.text;
}

function makeRegion(selection: SelectionPayload, cardX: number, cardY: number, paragraphIndex = 0): HTMLDivElement {
  const region = document.createElement("div");
  region.className = "ocr-region";
  region.style.left = `${selection.x - CARD_PADDING - cardX}px`;
  region.style.top = `${selection.y - CARD_PADDING - cardY}px`;
  region.style.width = `${selection.width + CARD_PADDING * 2}px`;
  region.style.height = `${selection.height + CARD_PADDING * 2}px`;

  for (const [lineIndex, line] of selection.lines.entries()) {
    const content = lineText(line);
    if (!content) continue;

    const element = document.createElement("div");
    element.dataset.line = `${paragraphIndex}:${lineIndex}`;
    element.className = "ocr-line";
    element.textContent = content;
    // OCR may split a screen row into several contained word runs. Position
    // each run at its photographed coordinates so skipped outside words do
    // not reflow the next run or stretch the blue browser selection.
    element.style.top = `${line.y - (selection.y - CARD_PADDING)}px`;
    element.style.left = `${line.x - (selection.x - CARD_PADDING)}px`;
    element.style.height = `${line.height}px`;
    element.style.lineHeight = `${line.height}px`;

    // Keep the glyph height exactly equal to the OCR line box. In particular,
    // do not impose a minimum or multiplier here: both made small text grow
    // and normal text shift when text mode was enabled.
    const fontSize = Math.max(1, line.height);
    element.style.fontSize = `${fontSize}px`;
    element.style.fontWeight = "500";
    if (measureContext) {
      measureContext.font = `500 ${fontSize}px "Inter Variable", "Segoe UI Variable", "Segoe UI", sans-serif`;
      const measuredWidth = measureContext.measureText(content).width;
      if (measuredWidth > 0 && line.width > 0) {
        element.style.width = `${measuredWidth}px`;
        // Width is fitted independently; scaleX never changes font height.
        element.style.transform = `scaleX(${line.width / measuredWidth})`;
      }
    }

    region.appendChild(element);
  }

  return region;
}

/**
 * Windows OCR occasionally returns each line separately. Fuse only the
 * naturally stacked lines back into a paragraph before painting, so a
 * paragraph gets one continuous shape instead of a card per line.
 */
function paragraphGroups(selections: SelectionPayload[]): SelectionPayload[] {
  const blocks = selections.filter((selection) => selection.kind === "block");
  const lines = selections
    .filter((selection) => selection.kind === "line")
    .sort((a, b) => a.y - b.y || a.x - b.x);
  const groups: SelectionPayload[] = [];

  for (const line of lines) {
    const previous = groups[groups.length - 1];
    const previousLine = previous?.lines[previous.lines.length - 1];
    const firstLine = line.lines[0];
    const verticalGap = previousLine && firstLine ? firstLine.y - (previousLine.y + previousLine.height) : Infinity;
    const aligned = previousLine && firstLine
      ? Math.abs(firstLine.x - previousLine.x) <= Math.max(32, previousLine.height * 2.5)
      : false;
    const closeEnough = previousLine && firstLine
      ? verticalGap >= -2 && verticalGap <= Math.max(previousLine.height, firstLine.height) * 1.15
      : false;

    if (previous && aligned && closeEnough) {
      previous.lines.push(...line.lines);
      const right = Math.max(previous.x + previous.width, line.x + line.width);
      const bottom = Math.max(previous.y + previous.height, line.y + line.height);
      previous.x = Math.min(previous.x, line.x);
      previous.y = Math.min(previous.y, line.y);
      previous.width = right - previous.x;
      previous.height = bottom - previous.y;
    } else {
      groups.push({ ...line, kind: "block", lines: [...line.lines] });
    }
  }

  return mergeTouchingParagraphs(blocks.concat(groups));
}

/** A region's visible shape extends CARD_PADDING on all four sides. If two
 * such shapes would touch, turn them into one flow before painting so borders
 * can never stack over neighbouring text. */
function shapesTouch(a: SelectionPayload, b: SelectionPayload): boolean {
  const horizontalGap = Math.max(a.x - (b.x + b.width), b.x - (a.x + a.width), 0);
  const verticalGap = Math.max(a.y - (b.y + b.height), b.y - (a.y + a.height), 0);
  return horizontalGap <= CARD_PADDING * 2 && verticalGap <= CARD_PADDING * 2;
}

function mergeSelections(selections: SelectionPayload[]): SelectionPayload {
  const left = Math.min(...selections.map((selection) => selection.x));
  const top = Math.min(...selections.map((selection) => selection.y));
  const right = Math.max(...selections.map((selection) => selection.x + selection.width));
  const bottom = Math.max(...selections.map((selection) => selection.y + selection.height));
  return {
    kind: "block",
    x: left,
    y: top,
    width: right - left,
    height: bottom - top,
    lines: selections
      .flatMap((selection) => selection.lines)
      .sort((a, b) => a.y - b.y || a.x - b.x),
  };
}

function mergeTouchingParagraphs(paragraphs: SelectionPayload[]): SelectionPayload[] {
  const flows: SelectionPayload[] = [];
  for (const paragraph of paragraphs.sort((a, b) => a.y - b.y || a.x - b.x)) {
    let merged = mergeSelections([paragraph]);
    // A newly widened flow can meet another neighbour, so keep merging until
    // every padded shape is disjoint. OCR regions are few, making this cheap.
    for (;;) {
      const connected = flows.filter((flow) => shapesTouch(flow, merged));
      if (connected.length === 0) break;
      for (const flow of connected) flows.splice(flows.indexOf(flow), 1);
      merged = mergeSelections([merged, ...connected]);
    }
    flows.push(merged);
  }
  return flows.sort((a, b) => a.y - b.y || a.x - b.x);
}

let paintScheduled = false;
let selectionWasRefined = false;
// Kept for Translate in place, which needs each paragraph's text and box.
let currentParagraphs: SelectionPayload[] = [];

export interface ParagraphBox {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
  /** Median OCR line height, a proxy for the original font size. */
  lineHeight: number;
}

/** Joins a paragraph's OCR lines into running text for translation,
 * mending words hyphenated across a line break ("trans-" + "lation"). */
function runningText(lines: OcrLinePayload[]): string {
  return lines.map(lineText).reduce((text, line) => {
    if (!text) return line;
    return /\p{L}-$/u.test(text) && /^\p{Ll}/u.test(line) ? text.slice(0, -1) + line : `${text} ${line}`;
  }, "");
}

/** In live text the "selection" is the whole screen's text, so actions
 * that read OCR lines must narrow to the lines the drag actually touched.
 * Returns those lines' keys, or `null` for a drawn selection (where every
 * line inside it is in scope, as before). */
function liveScope(): Set<string> | null {
  if (!textLayer.querySelector(".ocr-screen")) return null;
  const keys = new Set<string>();
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return keys;
  const range = selection.getRangeAt(0);
  for (const line of textLayer.querySelectorAll<HTMLElement>(".ocr-screen .ocr-line")) {
    if (line.dataset.line && range.intersectsNode(line)) keys.add(line.dataset.line);
  }
  return keys;
}

/** The paragraphs actions should work on, narrowed to the live selection
 * when there is one, with each paragraph's box shrunk to its kept lines. */
function scopedParagraphs(): SelectionPayload[] {
  const scope = liveScope();
  if (!scope) return currentParagraphs;
  return currentParagraphs.flatMap((paragraph, p) => {
    const lines = paragraph.lines.filter((_, l) => scope.has(`${p}:${l}`));
    if (lines.length === 0) return [];
    const x = Math.min(...lines.map((line) => line.x));
    const y = Math.min(...lines.map((line) => line.y));
    const right = Math.max(...lines.map((line) => line.x + line.width));
    const bottom = Math.max(...lines.map((line) => line.y + line.height));
    return [{ ...paragraph, lines, x, y, width: right - x, height: bottom - y }];
  });
}

export function paragraphsForTranslation(): ParagraphBox[] {
  return scopedParagraphs().map((paragraph) => {
    const heights = paragraph.lines.map((line) => line.height).sort((a, b) => a - b);
    return {
      text: runningText(paragraph.lines),
      x: paragraph.x,
      y: paragraph.y,
      width: paragraph.width,
      height: paragraph.height,
      lineHeight: heights[Math.floor(heights.length / 2)] ?? 16,
    };
  });
}

export interface CodeWord {
  text: string;
  x: number;
  width: number;
}

export interface CodeLine {
  y: number;
  height: number;
  words: CodeWord[];
}

/** Every OCR line across the whole selection, with per-word geometry, for
 * "Copy as code" — which needs word `x` offsets to rebuild indentation, so
 * (unlike every other text action) it reads this instead of the native
 * drag-selection. Mirrors Rust's `codeformat::CodeLine`/`CodeWord` shape
 * exactly, since it's sent straight to `format_as_code`. */
export function selectedOcrLines(): CodeLine[] {
  return scopedParagraphs().flatMap((paragraph) =>
    paragraph.lines.map((line) => ({
      y: line.y,
      height: line.height,
      words: line.words.map((word) => ({ text: word.text, x: word.x, width: word.width })),
    })),
  );
}

export function materializeSelection(
  selections: SelectionPayload[],
  cardBox: { x: number; y: number; width: number; height: number },
  boundary: Array<{ x: number; y: number }>,
  roundedRectangle: boolean,
): string {
  clearTextLayer();
  if (selections.length === 0) return "";
  const paragraphs = paragraphGroups(selections);
  currentParagraphs = paragraphs;

  textLayer.style.width = `${frameCanvas.width}px`;
  textLayer.style.height = `${frameCanvas.height}px`;
  textLayer.style.transform = `scale(${frameScale()})`;

  // A single hit surface covers the entire rounded selection, including
  // gaps between OCR blocks. Dragging across those gaps must stay a native
  // text selection instead of falling through to a new lasso stroke.
  const left = cardBox.x - CARD_PADDING;
  const top = cardBox.y - CARD_PADDING;
  const right = cardBox.x + cardBox.width + CARD_PADDING;
  const bottom = cardBox.y + cardBox.height + CARD_PADDING;
  const card = document.createElement("div");
  card.className = "ocr-selection-card";
  card.style.left = `${left}px`;
  card.style.top = `${top}px`;
  card.style.width = `${right - left}px`;
  card.style.height = `${bottom - top}px`;
  if (roundedRectangle) {
    const minX = Math.min(...boundary.map((point) => point.x));
    const minY = Math.min(...boundary.map((point) => point.y));
    const maxX = Math.max(...boundary.map((point) => point.x));
    const maxY = Math.max(...boundary.map((point) => point.y));
    card.style.clipPath = `inset(${Math.max(0, minY - top)}px ${Math.max(0, right - maxX)}px ${Math.max(0, bottom - maxY)}px ${Math.max(0, minX - left)}px round 20px)`;
  } else {
    // The card is padded for usability, but native ::selection paint must
    // stay within the actual freehand path that was drawn before text mode.
    card.style.clipPath = `polygon(${boundary.map((point) => `${point.x - left}px ${point.y - top}px`).join(", ")})`;
  }
  for (const selection of paragraphs) {
    card.appendChild(makeRegion(selection, left, top));
  }
  textLayer.appendChild(card);

  return paragraphs.map((selection) => selection.lines.map(lineText).join("\n")).join("\n\n");
}

/** Live text: every line OCR found on the screen, selectable in place with
 * no card and no visible boxes. Only the lines themselves take the pointer,
 * so a drag that starts on text selects it and a drag anywhere else still
 * draws a lasso. */
export function materializeScreenText(selections: SelectionPayload[]): string {
  clearTextLayer();
  if (selections.length === 0) return "";
  const paragraphs = paragraphGroups(selections);
  currentParagraphs = paragraphs;
  textLayer.style.width = `${frameCanvas.width}px`;
  textLayer.style.height = `${frameCanvas.height}px`;
  textLayer.style.transform = `scale(${frameScale()})`;
  const screen = document.createElement("div");
  screen.className = "ocr-screen";
  for (const [index, selection] of paragraphs.entries()) screen.appendChild(makeRegion(selection, 0, 0, index));
  textLayer.appendChild(screen);
  return paragraphs.map((selection) => selection.lines.map(lineText).join("\n")).join("\n\n");
}

/** The current native selection's bounds in frame pixels, if any. */
export function selectedBounds(): { x: number; y: number; width: number; height: number } | null {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const rect = selection.getRangeAt(0).getBoundingClientRect();
  if (rect.width === 0 && rect.height === 0) return null;
  const scale = frameScale() || 1;
  return { x: rect.left / scale, y: rect.top / scale, width: rect.width / scale, height: rect.height / scale };
}

export function clearTextLayer(): void {
  window.getSelection()?.removeAllRanges();
  textLayer.replaceChildren();
  currentParagraphs = [];
  selectionWasRefined = false;
}

function lineRanges(): Array<{ region: Element; range: Range; text: string }> {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return [];
  const active = selection.getRangeAt(0);
  const lines: Array<{ region: Element; range: Range; text: string }> = [];

  for (const region of textLayer.querySelectorAll(".ocr-region")) {
    for (const line of region.children) {
      if (!active.intersectsNode(line)) continue;
      const range = document.createRange();
      range.selectNodeContents(line);
      if (line.contains(active.startContainer)) range.setStart(active.startContainer, active.startOffset);
      if (line.contains(active.endContainer)) range.setEnd(active.endContainer, active.endOffset);
      const text = range.toString();
      if (text) lines.push({ region, range, text });
    }
  }
  return lines;
}

/** Plain text from the currently dragged subset, in OCR reading order. */
export function selectedText(): string {
  const lines = lineRanges();
  let lastRegion: Element | null = null;
  let text = "";
  for (const line of lines) {
    if (text) text += line.region === lastRegion ? "\n" : "\n\n";
    text += line.text;
    lastRegion = line.region;
  }
  return text.trim();
}

function renderHighlights(): void {
  const ranges = lineRanges();
  const refined = ranges.length > 0;
  if (refined && !selectionWasRefined) document.dispatchEvent(new Event("ocr-selection-refined"));
  selectionWasRefined = refined;
}

export function initTextLayer(): void {
  document.addEventListener("selectionchange", () => {
    if (paintScheduled) return;
    paintScheduled = true;
    requestAnimationFrame(() => {
      paintScheduled = false;
      renderHighlights();
    });
  });
}

/** Select OCR text only, excluding action buttons and other UI. */
export function selectAllText(): void {
  const first = textLayer.querySelector(".ocr-line")?.firstChild;
  const last = textLayer.querySelector(".ocr-region:last-child .ocr-line:last-child")?.firstChild;
  if (!first || !last) return;
  const range = document.createRange();
  range.setStart(first, 0);
  range.setEnd(last, last.textContent?.length ?? 0);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  renderHighlights();
}
