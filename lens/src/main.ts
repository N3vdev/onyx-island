// Session state machine (mirrors src-tauri/src/state.rs).
// IDLE -> CAPTURING -> ARMED -> DRAWING -> SNAPPING -> SELECTED -> IDLE

// Self-hosted so rendered OCR and toolbar text use one stable face without
// relying on a Windows font installation.
import "@fontsource-variable/inter/wght.css";
import { invoke } from "@tauri-apps/api/core";
import { Image as TauriImage } from "@tauri-apps/api/image";
import { listen } from "@tauri-apps/api/event";
import { writeImage, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type ActionBarAction, actionIcon, type Box, hideActionBar, type IconName, showActionBar } from "./actionbar";
import { clearDebugOverlay, initDebugOverlay } from "./debug";
import { initLasso, setCardShape, traceLassoPath, type Point, type SelectionGesture, type SelectionTool } from "./lasso";
import { initSiriGlow } from "./siri-glow";
import { CARD_PADDING, setCardPadding, clearTextLayer, frameScale, initTextLayer, materializeScreenText, materializeSelection, selectedBounds, paragraphsForTranslation, selectedOcrLines, type SelectionPayload, selectedText, selectAllText } from "./textselection";
import { type ActionEntry, type Entitlements, type SearchEngine, type Settings, type ThemeSettings } from "./settings-types";
import { barAccent, contrastingInk, DEFAULT_ACCENT, effectiveLassoStyle, effectivePreset, THEME_PRESETS } from "./themes";
import { clearTranslation, showTranslation, translatedText } from "./translatelayer";
import { clearRedactions, redactedBoxesForExport, showRedactions } from "./redactlayer";
import { isBrushOn, resizeBrushLayer, setBrushHandlers, setBrushOn, setErasing, isErasing, brushSizeLabel, type BrushStroke } from "./brush";
import { currentDesignerMode, resizeDesignerLayer, setDesignerDpiScale, setDesignerMode } from "./designertoolkit";

if (import.meta.env.DEV) initDebugOverlay();
initTextLayer();

type SessionState = "idle" | "armed" | "drawing" | "choosing" | "recognizing" | "text-ready";

interface FramePayload {
  dataUrl: string;
  width: number;
  height: number;
  /** Onyx's island is on this screen and shows the tools (island.rs). */
  islandTools?: boolean;
  /** Onyx's island look, which the bars match (see overlay-controls.css). */
  islandLook?: { glass: boolean; tint: number; frost: boolean };
  /** Captured (physical) pixels per design pixel — divide a frame-pixel
   * distance by this to report it in design pixels (the designer toolkit's
   * ruler). */
  dpiScale: number;
}

interface PreparedRegion {
  imageDataUrl: string | null;
}

interface TextSelectionResult {
  selections: SelectionPayload[];
}

interface CodeResult {
  code: string;
  language: string;
}

interface RedactResult {
  boxes: Box[];
  text: string;
}

// Mirrors Settings::default() in Rust — used only as the brief fallback
// before the real get_settings() response lands (see below), so the very
// first hotkey press after launch (an unlikely-fast race in practice)
// still behaves exactly like today rather than showing an empty bar.
let settings: Settings = {
  version: 1,
  hotkey: "shift+enter",
  theme: { accentColor: DEFAULT_ACCENT, spotlightDim: 0.42, cardPadding: 10, cardRadius: 20, preset: "classic", lassoStyle: "rainbow" },
  behavior: { autoCopyOnSelectRelease: false, translateTargetLang: "en", hideCursor: false },
  aiProvider: "chatgpt",
  customAiUrlTemplate: "",
  searchEngine: "google",
  customSearchUrlTemplate: "",
  choiceActions: [
    { id: "select-text", enabled: true },
    { id: "translate-here", enabled: true },
    { id: "lens", enabled: true },
    { id: "image-to-ai", enabled: true },
    { id: "save-png", enabled: true },
    { id: "pin", enabled: true },
    { id: "share", enabled: true },
  ],
  textActions: [
    { id: "copy", enabled: true },
    { id: "copy-code", enabled: true },
    { id: "redact", enabled: true },
    { id: "translate-inline", enabled: true },
    { id: "search", enabled: true },
    { id: "ask-ai", enabled: true },
    { id: "translate", enabled: false },
  ],
  customActions: [],
  launchAtStartup: false,
};

// Derived only in Rust (no command can set it) — see entitlements.rs. Gates
// custom actions and the Pro visual pack. Free until get_entitlements lands.
let paid = false;

// Resolved once at module load — long before the hotkey can fire, since the
// overlay window is created hidden at app startup — and refreshed via a
// push event on save, never polled. Awaited together (not as two
// independent .then()s) so applyTheme runs once with the real `paid` value,
// rather than once too early against the still-`false` default and again
// once entitlements land — a preset like Midnight would otherwise flash to
// its Free fallback for a moment on every launch.
void Promise.all([
  invoke<Settings>("get_settings"),
  invoke<Entitlements>("get_entitlements"),
]).then(([loaded, entitlements]) => {
  settings = loaded;
  paid = entitlements.plan !== "free";
  applyTheme(settings.theme);
  applyDesignerToolkitVisibility();
});

void listen<Settings>("settings-changed", (event) => {
  settings = event.payload;
  applyTheme(settings.theme);
});

// Pushed by Rust when a trial starts, a purchase lands, or a trial lapses —
// only ever between capture sessions (see entitlements.rs's `refresh`), so
// no open selection is ever left holding a now-hidden Pro button.
void listen<Entitlements>("entitlements-changed", (event) => {
  paid = event.payload.plan !== "free";
  applyTheme(settings.theme);
  applyDesignerToolkitVisibility();
});

let state: SessionState = "idle";
let lastPlainText = "";
let lensRequestToken = 0;
let selectionToken = 0;
let activeBox: Box | null = null;
let activePath: Point[] | null = null;
let activeOcrPath: Point[] | null = null;
let activeOcrShape: SelectionTool = "lasso";
/** Whether `activeOcrPath`/`activeOcrShape` came from converting a
 * finished freehand selection into a rectangle mid-session (`chooseTool`)
 * rather than being drawn as a rectangle from scratch — only affects the
 * crop's padding (see `saveImageSelection`'s `converted` param), but
 * redoing that exact crop for Redact needs to reproduce it faithfully. */
let activeOcrConverted = false;
let selectionTool: SelectionTool = "lasso";
let clipboardSave: Promise<boolean> = Promise.resolve(true);
let latestPngBlob: Blob | null = null;
// Where latestPngBlob's pixels actually sit in the frame. Not the same as
// activeBox: a converted lasso's crop is padded by 10px, and any crop is
// clamped at the screen edge. Only read while latestPngBlob is non-null,
// and always assigned in the same callback that sets it, so it never needs
// clearing on its own.
let latestCropRect: Box | null = null;
let imageGeneration = 0;
let imagePreparation: Promise<string | null> = Promise.resolve(null);
let dismissing = false;
/** Set once Redact has found something in the current selection; every
 * text action (`currentText()`) then uses this instead of the native
 * selection, not just Copy — once something's flagged sensitive, Search,
 * Ask AI and custom actions shouldn't see it either. Cleared alongside the
 * visual layer in `onStart`/`reset`. */
let redactedText: string | null = null;

// Pro "Snap" toggle: finishing a draw snaps the outline to the object inside
// it (see snapThenFinish). `skipSnapOnce` lets the Full screen button, which
// ends through the same onEnd, opt out of it for that one call.
let editMode = false;
let skipSnapOnce = false;
const frameCanvas = document.getElementById("frame") as HTMLCanvasElement;
const spotlightCanvas = document.getElementById("spotlight") as HTMLCanvasElement;
const lassoCanvas = document.getElementById("lasso") as HTMLCanvasElement;
const toolBar = document.getElementById("tool-bar") as HTMLDivElement;
const overlayClose = document.getElementById("overlay-close") as HTMLButtonElement;
overlayClose.addEventListener("click", () => void dismiss());
const lassoButton = document.getElementById("tool-lasso") as HTMLButtonElement;
const rectangleButton = document.getElementById("tool-rectangle") as HTMLButtonElement;
const fullScreenButton = document.getElementById("tool-full-screen") as HTMLButtonElement;
const eyedropperButton = document.getElementById("tool-eyedropper") as HTMLButtonElement;
const measureButton = document.getElementById("tool-measure") as HTMLButtonElement;
const editButton = document.getElementById("tool-edit") as HTMLButtonElement;
const brushButton = document.getElementById("tool-brush") as HTMLButtonElement;
const brushEraseButton = document.getElementById("tool-brush-erase") as HTMLButtonElement;
const toolBarDivider = document.getElementById("tool-bar-divider") as HTMLSpanElement;

// Every tool gets an icon beside its label, same as the action buttons. Done
// once at load; the buttons' own handlers only ever toggle hidden/pressed.
const TOOL_ICONS: Record<string, IconName> = {
  "tool-rectangle": "rectangle",
  "tool-lasso": "lasso",
  "tool-full-screen": "fullscreen",
  "tool-eyedropper": "eyedropper",
  "tool-measure": "ruler",
  "tool-edit": "magnet",
  "tool-brush": "brush",
  "tool-brush-erase": "eraser",
};
for (const [id, icon] of Object.entries(TOOL_ICONS)) {
  const button = document.getElementById(id);
  if (!button || button.querySelector(".action-icon")) continue;
  const label = document.createElement("span");
  label.className = "action-label";
  label.textContent = button.textContent?.trim() ?? "";
  button.setAttribute("aria-label", label.textContent);
  if (!button.title) button.title = label.textContent;
  button.replaceChildren(actionIcon(icon), label);
}
const openingArcCanvas = document.getElementById("opening-prism") as HTMLCanvasElement;
const messagePanel = document.getElementById("message") as HTMLDivElement;
const hint = document.getElementById("hint") as HTMLDivElement;
const hintLabel = hint.querySelector(".hint-label") as HTMLSpanElement;
const hintShortcut = hint.querySelector(".hint-shortcut") as HTMLSpanElement;
const hintAction = hint.querySelector(".hint-action") as HTMLButtonElement;
let hintActionRun: (() => void) | null = null;
hintAction.addEventListener("pointerdown", (event) => event.stopPropagation());
hintAction.addEventListener("click", () => hintActionRun?.());
const frameCtx = frameCanvas.getContext("2d");
const spotlightCtx = spotlightCanvas.getContext("2d");
const siriGlow = initSiriGlow(openingArcCanvas);
let hintTimeout: number | null = null;

function hideHint(): void {
  if (hintTimeout !== null) window.clearTimeout(hintTimeout);
  hintTimeout = null;
  hint.hidden = true;
}

function showHint(label: string, shortcut: string, durationMs = 0, avoid?: Box, action?: { label: string; run: () => void }): void {
  hideHint();
  hintLabel.textContent = label;
  hintShortcut.textContent = shortcut;
  hintActionRun = action?.run ?? null;
  hintAction.textContent = action?.label ?? "";
  hintAction.hidden = !action;
  hint.classList.toggle("has-action", !!action);
  const placeAbove = avoid && (avoid.y + avoid.height) * frameScale() > window.innerHeight - 86;
  hint.style.top = placeAbove ? (document.body.classList.contains("island-tools") ? "72px" : "16px") : "auto";
  hint.style.bottom = placeAbove ? "auto" : window.innerWidth <= 520 ? "16px" : "24px";
  hint.hidden = false;
  if (durationMs > 0) hintTimeout = window.setTimeout(hideHint, durationMs);
}

document.addEventListener("ocr-selection-refined", () => {
  // Live text: a drag across text on the open screen, no lasso first.
  if (state === "armed" && liveText && selectedText()) {
    const bounds = selectedBounds();
    if (!bounds) return;
    activeBox = bounds;
    liveSelection = true;
    state = "text-ready";
    hideCodeChips();
  }
  if (state !== "text-ready" || !activeBox || !selectedText()) return;
  hideHint();
  showTextActions(activeBox);
  void offerLinkAction();
  if (settings.behavior.autoCopyOnSelectRelease) void copySelection();
});

document.addEventListener("selectionchange", () => {
  if (state !== "text-ready" || selectedText()) return;
  hideActionBar();
  if (liveSelection) {
    // Back to the open screen, ready for another drag or a lasso.
    liveSelection = false;
    activeBox = null;
    state = "armed";
  }
});

// ---------------------------------------------------------------- live text
// After the overlay appears, the whole screen is read in the background:
// text becomes selectable in place (no lasso, no Select Text) and QR codes
// get a chip. Both are best-effort — the overlay never waits for them.
let liveText = false;
let liveSelection = false;
let screenScanToken = 0;
let lastCodes: ScreenCode[] = [];
const codeChips = document.getElementById("code-chips") as HTMLDivElement;

interface ScreenLink { kind: "web" | "email" | "wifi" | "secret" | "text"; label: string; copyText: string; target: string | null }
interface ScreenCode { link: ScreenLink; x: number; y: number; width: number; height: number }

function scanScreen(width: number, height: number): void {
  const token = ++screenScanToken;
  const whole: Array<[number, number]> = [[0, 0], [width, 0], [width, height], [0, height]];
  void invoke<TextSelectionResult>("recognize_text", { points: whole })
    .then((result) => {
      // Only onto an untouched screen: once a lasso has started, that flow owns the text layer.
      if (token !== screenScanToken || state !== "armed" || result.selections.length === 0) return;
      if (!materializeScreenText(result.selections)) return;
      liveText = true;
      syncDrawOverText();
      showHint("Circle anything, or drag across text to select it", "Alt draws over text");
    })
    .catch((err) => console.warn("live text unavailable", err));
  void invoke<ScreenCode[]>("scan_codes")
    .then((codes) => {
      if (token !== screenScanToken || codes.length === 0 || (state !== "armed" && state !== "choosing")) return;
      lastCodes = codes;
      showCodeChips(codes);
    })
    .catch((err) => console.warn("QR scan failed", err));
}

function endLiveText(): void {
  screenScanToken++;
  liveText = false;
  liveSelection = false;
  lastCodes = [];
  hideCodeChips();
}

/** Alt, or a tool that draws (eyedropper, measure, brush), lets a stroke
 * start on top of text instead of selecting it. */
let altHeld = false;
function syncDrawOverText(): void {
  document.body.classList.toggle("draw-over-text", altHeld || currentDesignerMode() !== null || isBrushOn());
}
window.addEventListener("keydown", (event) => {
  if (event.key !== "Alt") return;
  event.preventDefault();
  altHeld = true;
  syncDrawOverText();
});
window.addEventListener("keyup", (event) => {
  if (event.key !== "Alt") return;
  event.preventDefault();
  altHeld = false;
  syncDrawOverText();
});
window.addEventListener("blur", () => {
  altHeld = false;
  syncDrawOverText();
});

function showCodeChips(codes: ScreenCode[]): void {
  const scale = frameScale() || 1;
  codeChips.replaceChildren();
  for (const code of codes) {
    const ring = document.createElement("div");
    ring.className = "code-ring";
    const pad = 6;
    ring.style.left = `${code.x * scale - pad}px`;
    ring.style.top = `${code.y * scale - pad}px`;
    ring.style.width = `${code.width * scale + pad * 2}px`;
    ring.style.height = `${code.height * scale + pad * 2}px`;

    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "code-chip";
    const opens = code.link.target !== null;
    const verb = code.link.kind === "email" ? "Email" : opens ? "Open" : code.link.kind === "wifi" ? "Copy password ·" : "Copy";
    const label = document.createElement("span");
    label.className = "action-label";
    label.textContent = `${verb} ${code.link.label}`;
    chip.append(actionIcon(opens ? "external" : "copy"), label);
    chip.title = opens ? code.link.target ?? "" : "Copy to clipboard";
    chip.addEventListener("pointerdown", (event) => event.stopPropagation());
    chip.addEventListener("click", () => void useLink(code.link, label));
    codeChips.append(ring, chip);
    // Centred under the code, kept on screen.
    const left = (code.x + code.width / 2) * scale - chip.offsetWidth / 2;
    const below = (code.y + code.height) * scale + pad + 10;
    const top = below + chip.offsetHeight > window.innerHeight - 8 ? code.y * scale - pad - 10 - chip.offsetHeight : below;
    chip.style.left = `${Math.max(8, Math.min(window.innerWidth - chip.offsetWidth - 8, left))}px`;
    chip.style.top = `${Math.max(8, top)}px`;
  }
  codeChips.hidden = false;
}

function hideCodeChips(): void {
  codeChips.hidden = true;
  codeChips.replaceChildren();
}

/** Open a web/email link (and get out of the way so it can be seen), or
 * copy the rest — Wi-Fi passwords, 2FA codes, plain text. */
async function useLink(link: ScreenLink, label?: HTMLElement): Promise<void> {
  if (link.target) {
    try {
      await openUrl(link.target);
      void dismiss();
    } catch (err) {
      console.error("could not open link", err);
      showHint("Couldn't open that link", "Esc to close", 2600);
    }
    return;
  }
  try {
    await writeText(link.copyText);
    if (label) label.textContent = "Copied";
    showHint(link.kind === "wifi" ? "Wi-Fi password copied" : "Copied", "Esc to close", 1800);
  } catch (err) {
    console.error("clipboard write failed", err);
  }
}

/** When the selected text is a single link or email address, lead the text
 * actions with opening it. */
async function offerLinkAction(): Promise<void> {
  const text = selectedText();
  if (!text || text.length > 2048) return;
  let link: ScreenLink | null = null;
  try {
    link = await invoke<ScreenLink | null>("classify_text", { text });
  } catch {
    return;
  }
  if (!link?.target || state !== "text-ready" || !activeBox || selectedText() !== text) return;
  const open: ActionBarAction = {
    label: link.kind === "email" ? "Send email" : "Open link",
    icon: "external",
    primary: true,
    description: link.target,
    onClick: () => useLink(link),
  };
  showActionBar(activeBox, [open, ...actionsFrom(settings.textActions, TEXT_ACTION_BUILDERS)]);
}

// Dim the frozen frame, then reveal one rounded selection so the original
// screen pixels remain crisp beneath the transparent selectable OCR layer.
// These start from Settings::default()'s values (see the `settings`
// fallback above) and are refreshed by applyTheme() below.
let spotlightFill = "rgba(0, 0, 0, 0.42)";
// Eyedropper/measure need to actually see the screen to be useful — the
// normal dim (however the user has it configured) is scaled way down
// rather than replaced by a fixed value, so a heavier custom theme still
// reads as "dimmer" than a lighter one even in these modes.
const DESIGNER_MODE_DIM_FACTOR = 0.22;
let designerSpotlightFill = "rgba(0, 0, 0, 0.09)";
// Card padding lives in textselection.ts's CARD_PADDING and the stroke's own
// copy in lasso.ts, both set together from applyTheme — the spotlight hole,
// the settled stroke, the OCR hit surface and the saved crop all have to
// share one shape or a selection's clickable area stops matching what's
// drawn. Settings is a separate window, so a change only lands between
// capture sessions in practice.
let spotlightCardRadius = 20;
let revealBox: Box | null = null;
let revealPath: Point[] | null = null;

/** Applies theme settings live: a CSS custom property for the DOM-styled
 * bits (action-bar/tool-bar primary color, selection-card corner radius),
 * and module state for the canvas-drawn spotlight. Deliberately never
 * touches lasso.ts's fixed rainbow gradient. */
function applyTheme(theme: ThemeSettings): void {
  const root = document.documentElement.style;
  const preset = effectivePreset(theme.preset, paid);
  // Clear every preset's variables first so switching presets never leaves a
  // previous preset's colours behind; the index.html :root defaults show
  // through wherever the active preset doesn't override.
  for (const other of Object.values(THEME_PRESETS)) {
    for (const name of Object.keys(other.vars)) root.removeProperty(name);
  }
  for (const [name, value] of Object.entries(preset.vars)) root.setProperty(name, value);
  // A preset brings its own accent unless the user has chosen one.
  const accent = theme.accentColor === DEFAULT_ACCENT && preset.accent ? preset.accent : theme.accentColor;
  root.setProperty("--accent", accent);
  root.setProperty("--on-accent", contrastingInk(accent));
  const onBars = barAccent(accent);
  root.setProperty("--bar-accent", onBars.fill);
  root.setProperty("--on-bar-accent", onBars.ink);
  lasso.setStyle(effectiveLassoStyle(theme.lassoStyle, paid));
  document.documentElement.style.setProperty("--card-radius", `${theme.cardRadius}px`);
  spotlightFill = `rgba(0, 0, 0, ${theme.spotlightDim})`;
  designerSpotlightFill = `rgba(0, 0, 0, ${theme.spotlightDim * DESIGNER_MODE_DIM_FACTOR})`;
  setCardPadding(theme.cardPadding);
  setCardShape(theme.cardPadding, theme.cardRadius);
  spotlightCardRadius = theme.cardRadius;
  fillSpotlight();
}

function fillSpotlight(): void {
  if (!spotlightCtx) return;
  spotlightCtx.clearRect(0, 0, spotlightCanvas.width, spotlightCanvas.height);
  spotlightCtx.fillStyle = currentDesignerMode() ? designerSpotlightFill : spotlightFill;
  spotlightCtx.fillRect(0, 0, spotlightCanvas.width, spotlightCanvas.height);
  if (!revealBox && !revealPath) return;

  spotlightCtx.save();
  spotlightCtx.globalCompositeOperation = "destination-out";
  spotlightCtx.fillStyle = "#000";
  if (revealPath) {
    traceLassoPath(spotlightCtx, revealPath);
  } else if (revealBox) {
    roundedRectPath(
      spotlightCtx,
      revealBox.x,
      revealBox.y,
      revealBox.width,
      revealBox.height,
      spotlightCardRadius,
    );
  }
  spotlightCtx.fill();
  spotlightCtx.restore();
}

function roundedRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}

/** One continuous reveal is the visual result of a freehand lasso. The
 * original screenshot stays crisp inside; transparent OCR text above it
 * supplies caret/drag selection without repainting the photographed text. */
function clearReveal(): void {
  revealBox = null;
  revealPath = null;
  fillSpotlight();
}

function revealSelectionCard(box: Box): void {
  // This is the actual outline, not content bounds to pad a second time.
  revealBox = box;
  fillSpotlight();
}

const lasso = initLasso(lassoCanvas, {
  onStart: () => {
    setBrushAvailable(false);
    // A press on empty space may only be a click (which cancels) — live text
    // survives until a real stroke finishes; just drop its selection.
    if (liveText) {
      window.getSelection()?.removeAllRanges();
      liveSelection = false;
      hideCodeChips();
    }
    siriGlow.stop();
    selectionToken++;
    lensRequestToken++;
    hideHint();
    messagePanel.hidden = true;
    hideActionBar();
    if (!liveText) clearTextLayer();
    clearTranslation();
    clearRedactions();
    redactedText = null;
    clearReveal();
    lastPlainText = "";
    activeBox = null;
    activePath = null;
    activeOcrPath = null;
    latestPngBlob = null;
    imageGeneration++;
    state = "drawing";
  },
  onEnd: (points: Point[], gesture: SelectionGesture) => {
    // A real selection takes over the text layer from live text.
    if (liveText) {
      endLiveText();
      clearTextLayer();
    }
    // Full screen has nothing to snap to — it's the whole frame by design.
    const snap = editMode && paid && !skipSnapOnce;
    skipSnapOnce = false;
    if (gesture === "swipe") void swipeThenFinish(points, snap);
    else if (snap) void snapThenFinish(points);
    else finishSelection(points, gesture === "lasso" ? "lasso" : "rectangle");
  },
  onCancel: () => {
    clearReveal();
    state = "armed";
    if (liveText) {
      showHint("Circle anything, or drag across text to select it", "Alt draws over text");
      if (lastCodes.length) showCodeChips(lastCodes);
    } else {
      showHint("Draw around text or an image", "Esc to close");
    }
  },
  onMorph: (path) => {
    if (!activeBox || state === "idle" || state === "drawing") return;
    revealPath = path;
    // The next rectangle paint supplies its exact final boundary.
    if (!path) return;
    fillSpotlight();
  },
  onRectangle: (box) => {
    if (!activeBox || state === "idle" || state === "drawing") return;
    // Share the stroke's animation clock; a separate mask animation can
    // expose a bright rim outside the stroke while either one is moving.
    if (!revealPath && revealBox && box.x === revealBox.x && box.y === revealBox.y
      && box.width === revealBox.width && box.height === revealBox.height) return;
    revealPath = null;
    revealSelectionCard(box);
  },
});

/** The shared tail of finishing a draw: a plain loop/rectangle goes through
 * here directly, and Edit mode calls it with the snapped outline (always a
 * freehand-shaped path, whichever tool drew the original). */
function finishSelection(points: Point[], shape: SelectionTool): void {
  const box = boundsOf(points);
  activePath = points;
  activeOcrPath = points;
  activeOcrShape = shape;
  activeOcrConverted = false;
  revealPath = shape === "lasso" ? points : null;
  revealSelectionCard(box);
  activeBox = box;
  const imageReady = saveImageSelection(points, shape);
  imagePreparation = prepareRegion(points, imageReady, box);
}

/** A straight swipe across something selects the rectangle it spans, with
 * each side moved onto the nearest real edge in the screenshot (rectsnap.rs)
 * — so slashing across a picture picks up exactly that picture. */
async function swipeThenFinish(points: Point[], thenSnap: boolean): Promise<void> {
  const token = selectionToken;
  const swiped = boundsOf(points);
  let region: Box = swiped;
  try {
    const found = await invoke<Box | null>("detect_region", { region: swiped });
    if (found) region = found;
  } catch (err) {
    console.error("region detection failed", err);
  }
  if (token !== selectionToken) return;
  lasso.setRect(region);
  const corners = [
    { x: region.x, y: region.y }, { x: region.x + region.width, y: region.y },
    { x: region.x + region.width, y: region.y + region.height }, { x: region.x, y: region.y + region.height },
  ];
  if (thenSnap) void snapThenFinish(corners);
  else finishSelection(corners, "rectangle");
}

/** Pro. Edit mode: the loop you draw is only a rough upper bound — Rust
 * finds the object inside it (snap.rs) and the stroke slides onto that
 * object's real edges, so everything after (Copy, Save, Pin, Share…) works
 * on a cut-out with no background instead of a box.
 *
 * Falls back to the plain outline you drew, with a note, when there's
 * nothing to snap to — a flat area, or an object the colour model can't
 * tell from its surroundings. */
async function snapThenFinish(points: Point[]): Promise<void> {
  const token = selectionToken;
  const drawnBox = boundsOf(points);
  // Show the loop as a normal selection while Rust works, so the spotlight
  // and stroke don't vanish for the length of the request. "recognizing"
  // also parks the tool buttons, which would otherwise fight the morph.
  activeBox = drawnBox;
  revealPath = selectionTool === "lasso" ? points : null;
  revealSelectionCard(drawnBox);
  state = "recognizing";
  showHint("Snapping to edges…", "Esc to cancel", 0, drawnBox);
  // The first snap on a machine fetches the small edge model (4.6 MB, once).
  const stopListening = await listen<string>("snap-status", (event) => {
    if (event.payload === "downloading" && token === selectionToken && state === "recognizing") {
      showHint("Downloading the edge model…", "One time · 4.6 MB", 0, drawnBox);
    }
  });

  let refined: Point[] | null = null;
  try {
    const result = await invoke<Array<[number, number]> | null>("snap_selection", {
      points: points.map((point): [number, number] => [point.x, point.y]),
    });
    if (result && result.length >= 3) refined = result.map(([x, y]) => ({ x, y }));
  } catch (err) {
    console.error("snap to edges failed", err);
  }
  stopListening();
  // Drawn over, dismissed, or reset while Rust was working.
  if (token !== selectionToken) return;
  hideHint();

  if (!refined) {
    finishSelection(points, selectionTool);
    showHint("No clear edge found", "Kept your outline — try circling a bit wider", 3200, drawnBox);
    return;
  }
  await lasso.snapTo(refined);
  if (token !== selectionToken) return;
  // The morph's last frame drops the reveal back to a rounded box (that is
  // right for Select Text, not here) — pin it to the snapped outline.
  revealPath = refined;
  finishSelection(refined, "lasso");
}

/** Eyedropper/Measure/Edit are hidden entirely on Free rather than
 * shown locked — there's no Settings row to explain them the way custom
 * actions have one, so an unexplained disabled toolbar button would just be
 * confusing. */
function applyDesignerToolkitVisibility(): void {
  eyedropperButton.hidden = !paid;
  measureButton.hidden = !paid;
  editButton.hidden = !paid;
  if (!paid) setBrushAvailable(false);
  if (!paid && editMode) {
    editMode = false;
    editButton.setAttribute("aria-pressed", "false");
  }
  toolBarDivider.hidden = !paid;
}

function chooseTool(next: SelectionTool): void {
  if (state === "recognizing" || state === "text-ready") return;
  // Exiting the designer toolkit is unconditional, even if `next` already
  // equals `selectionTool` below — otherwise clicking Lasso while already
  // on the lasso tool (but currently in Eyedropper/Measure mode) would hit
  // the early return before ever leaving designer mode.
  setDesignerMode(null);
  leaveBrush();
  syncDrawOverText();
  fillSpotlight();
  eyedropperButton.setAttribute("aria-pressed", "false");
  measureButton.setAttribute("aria-pressed", "false");
  if (next === selectionTool) return;
  selectionTool = next;
  lassoButton.setAttribute("aria-pressed", String(next === "lasso"));
  rectangleButton.setAttribute("aria-pressed", String(next === "rectangle"));
  if (lasso.setTool(next) && activePath && activeBox) {
    // An already-drawn lasso becomes the padded rounded selection its own
    // stroke morphs to (see ocrHitSurface) — not the unpadded `activeBox`,
    // which would leave this selection's OCR/hit surface narrower than the
    // rectangle now drawn on screen.
    activeOcrPath = ocrHitSurface(activePath, "lasso").path;
    activeOcrShape = "rectangle";
    activeOcrConverted = true;
    const imageReady = saveImageSelection(activePath, "rectangle", true);
    imagePreparation = prepareRegion(activePath, imageReady, activeBox);
  }
  showHint(next === "lasso" ? "Draw a freehand selection" : "Drag to draw a rounded rectangle", "Esc to close", 2200);
}

/** Free. Selects the entire captured frame in one click, as if it had just
 * been drawn corner-to-corner with the rectangle tool — goes through
 * lasso.ts's own `selectAll`, which fires the exact same onStart/onEnd pair
 * a real drag does, so the crop/action-bar logic never needs a second copy
 * for this. */
function captureFullScreen(): void {
  if (state === "recognizing" || state === "text-ready") return;
  // Selects the whole frame as a rectangle without switching the tool, so
  // the next draw is still whatever you had chosen (Lasso by default).
  skipSnapOnce = true;
  lasso.selectAll({ x: 0, y: 0, width: frameCanvas.width, height: frameCanvas.height });
}

/** Pro. Toggles Eyedropper/Measure; picking either always exits the other
 * and suspends lasso.ts's own pointer handling (see designertoolkit.ts's
 * module doc) without touching `selectionTool` — so returning to Lasso or
 * Rectangle resumes exactly the tool that was active before. */
function chooseDesignerTool(next: "eyedropper" | "measure"): void {
  if (state === "recognizing" || state === "text-ready") return;
  const entering = currentDesignerMode() !== next;
  leaveBrush();
  setDesignerMode(entering ? next : null);
  syncDrawOverText();
  fillSpotlight();
  lassoButton.setAttribute("aria-pressed", String(!entering && selectionTool === "lasso"));
  rectangleButton.setAttribute("aria-pressed", String(!entering && selectionTool === "rectangle"));
  eyedropperButton.setAttribute("aria-pressed", String(entering && next === "eyedropper"));
  measureButton.setAttribute("aria-pressed", String(entering && next === "measure"));
  if (entering) {
    showHint(
      next === "eyedropper" ? "Hover to preview, click to copy the color" : "Click two points to measure · Hold Shift for horizontal or vertical",
      "Esc to close",
      2600,
    );
  } else {
    showHint(selectionTool === "lasso" ? "Draw a freehand selection" : "Drag to draw a rounded rectangle", "Esc to close", 2200);
  }
}

lassoButton.addEventListener("click", () => chooseTool("lasso"));
rectangleButton.addEventListener("click", () => chooseTool("rectangle"));
fullScreenButton.addEventListener("click", () => captureFullScreen());
eyedropperButton.addEventListener("click", () => chooseDesignerTool("eyedropper"));
measureButton.addEventListener("click", () => chooseDesignerTool("measure"));

/** Independent of the shape tool — Edit only changes what happens once a
 * draw finishes, so Lasso and Rectangle both keep working as the way you
 * draw the rough loop. */
editButton.addEventListener("click", () => {
  if (state === "recognizing" || state === "text-ready") return;
  editMode = !editMode;
  editButton.setAttribute("aria-pressed", String(editMode));
  showHint(
    editMode ? "Snap on — circle something and it snaps to its edges" : "Snap off",
    "Esc to close",
    editMode ? 2800 : 1800,
  );
});

/** Pro. The Edit-mode brush: paint over the current selection to grow it,
 * erase to shrink it. Each finished stroke goes to Rust (snap.rs), which
 * merges it into the outline and hands back a fresh, smooth one. */
/** Brush edits an existing selection, so its button only appears once
 * something has been drawn (and the plan allows it). */
function setBrushAvailable(available: boolean): void {
  brushButton.hidden = !available;
  if (!available) leaveBrush();
}

function leaveBrush(): void {
  if (!isBrushOn()) return;
  setBrushOn(false);
  setErasing(false);
  syncDrawOverText();
}

function syncBrushButtons(): void {
  const on = isBrushOn();
  brushButton.setAttribute("aria-pressed", String(on));
  brushEraseButton.hidden = !on;
  brushEraseButton.setAttribute("aria-pressed", String(isErasing()));
}

async function applyBrushStroke(stroke: BrushStroke): Promise<void> {
  if (!activePath || state !== "choosing") return;
  const token = selectionToken;
  let refined: Point[] | null = null;
  try {
    const result = await invoke<Array<[number, number]> | null>("brush_selection", {
      points: activePath.map((point): [number, number] => [point.x, point.y]),
      strokes: [stroke],
      frameSize: [frameCanvas.width, frameCanvas.height],
    });
    if (result && result.length >= 3) refined = result.map(([x, y]) => ({ x, y }));
  } catch (err) {
    console.error("brush stroke failed", err);
  }
  if (token !== selectionToken) return;
  if (!refined) {
    showHint("Couldn't apply that stroke", "Keep some of the selection to work with", 2600, activeBox ?? undefined);
    return;
  }
  lasso.setPath(refined);
  finishSelection(refined, "lasso");
}

setBrushHandlers(applyBrushStroke, syncBrushButtons);

brushButton.addEventListener("click", () => {
  if (isBrushOn()) {
    leaveBrush();
    return;
  }
  if (state !== "choosing" || !activePath) {
    showHint("Make a selection first", "Then paint over it to add or erase", 2600, activeBox ?? undefined);
    return;
  }
  setDesignerMode(null);
  eyedropperButton.setAttribute("aria-pressed", "false");
  measureButton.setAttribute("aria-pressed", "false");
  setBrushOn(true);
  syncDrawOverText();
  showHint(`Brush ${brushSizeLabel()} — drag to add`, "Alt or Erase removes · [ ] size · Esc to finish", 4200, activeBox ?? undefined);
});

brushEraseButton.addEventListener("click", () => setErasing(!isErasing()));

/** Text actions are intentionally gated behind a real native drag selection. */
function currentText(): string {
  return redactedText ?? selectedText();
}

interface CornerBounds {
  x: number;
  y: number;
  right: number;
  bottom: number;
}

function boundsOf(points: { x: number; y: number }[]): Box {
  const acc = points.reduce<CornerBounds>(
    (acc, p) => ({
      x: Math.min(acc.x, p.x),
      y: Math.min(acc.y, p.y),
      right: Math.max(acc.right, p.x),
      bottom: Math.max(acc.bottom, p.y),
    }),
    { x: Infinity, y: Infinity, right: -Infinity, bottom: -Infinity },
  );
  return { x: acc.x, y: acc.y, width: acc.right - acc.x, height: acc.bottom - acc.y };
}

function unionBoxOf(selections: SelectionPayload[]): Box {
  return boundsOf(selections.map((sel) => ({ x: sel.x, y: sel.y })).concat(
    selections.map((sel) => ({ x: sel.x + sel.width, y: sel.y + sel.height })),
  ));
}

const EMPTY_MARGIN_THRESHOLD = 64;
const MAX_EMPTY_MARGIN_TRIM = 48;

/**
 * Preserve the size of the freehand gesture. OCR may cautiously trim only a
 * clearly blank outer band, never snap the lasso all the way down to text.
 * The cap makes the final freehand-to-rounded-rectangle transition feel like
 * a refinement rather than a zoom.
 */
function trimSubstantialEmptyMargins(drawn: Box, content: Box): Box {
  const trim = (blank: number): number => blank >= EMPTY_MARGIN_THRESHOLD
    ? Math.min(MAX_EMPTY_MARGIN_TRIM, Math.max(0, blank - CARD_PADDING))
    : 0;
  const left = trim(Math.max(0, content.x - drawn.x));
  const top = trim(Math.max(0, content.y - drawn.y));
  const right = trim(Math.max(0, drawn.x + drawn.width - (content.x + content.width)));
  const bottom = trim(Math.max(0, drawn.y + drawn.height - (content.y + content.height)));
  return {
    x: drawn.x + left,
    y: drawn.y + top,
    width: Math.max(1, drawn.width - left - right),
    height: Math.max(1, drawn.height - top - bottom),
  };
}

/** Returns the label the button flashes, so "Copied" only ever appears
 * once the text is really on the clipboard. */
async function copySelection(): Promise<string | void> {
  const text = currentText();
  if (!text) return;
  try {
    // The Web Clipboard API isn't reliably permitted inside WebView2 without
    // extra setup; the plugin goes through Tauri's own IPC/permission path.
    await clipboardSave;
    await writeText(text);
    console.debug(`copied ${text.length} chars`);
    return "Copied";
  } catch (err) {
    console.error("clipboard write failed", err);
    return "Couldn't copy";
  }
}

/** Unlike every other text action, this reads the full OCR'd selection
 * (`selectedOcrLines`) rather than the native drag-selection — indentation
 * reconstruction needs each word's `x` offset, which a plain-text native
 * selection has already thrown away. Rust does the actual reconstruction
 * and language guess (`codeformat.rs`); this just ships the geometry over
 * and writes the result to the clipboard. */
async function copyAsCode(): Promise<string | void> {
  const lines = selectedOcrLines();
  if (lines.length === 0) return;
  try {
    const result = await invoke<CodeResult>("format_as_code", { lines });
    if (!result.code) return;
    await clipboardSave;
    await writeText(result.code);
    console.debug(`copied ${result.code.length} chars as ${result.language}`);
    return result.language === "Text" ? "Copied" : `Copied as ${result.language}`;
  } catch (err) {
    console.error("copy as code failed", err);
  }
}

/** Pro. Preview-first by design: this only detects and shows covers — it
 * doesn't copy or save text itself. Every text action reads
 * `currentText()` afterward, so Copy/Search/Ask AI honor it without their
 * own onClick changing at all. Reads the full OCR'd selection like
 * copy-code, not the native drag selection — sensitive text can be
 * anywhere in the circled region.
 *
 * It does re-run the image crop (not via `prepareRegion` — that belongs to
 * the choice bar's "choosing" state and would wrongly swap the action bar
 * and bump every staleness token out from under the text bar this is
 * called from). `saveImageSelection` on its own just re-encodes and
 * re-writes the clipboard, which matters even though Save PNG isn't
 * reachable once text actions are showing: `onEnd` already wrote the
 * *unredacted* crop to the clipboard before Redact ever ran, so without
 * this a plain Ctrl+V would still leak the original image. */
async function redactSelection(): Promise<string | void> {
  const lines = selectedOcrLines();
  if (lines.length === 0) return;
  try {
    const result = await invoke<RedactResult>("detect_sensitive", { lines });
    if (result.boxes.length === 0) return "Nothing found";
    showRedactions(result.boxes);
    redactedText = result.text;
    if (activeOcrPath) {
      void saveImageSelection(activeOcrPath, activeOcrShape, activeOcrConverted, redactedBoxesForExport());
    }
    return `Found ${result.boxes.length}`;
  } catch (err) {
    console.error("redact failed", err);
  }
}

interface AiProviderConfig {
  label: string;
  /** Undefined when the provider has no documented/reliable query-prefill
   * parameter — Claude and Gemini fall in this bucket, so they get the
   * clipboard+homepage fallback rather than a guessed `?q=` that wouldn't
   * actually prefill anything. */
  queryUrl?: (text: string) => string;
  homepageUrl: string;
}

const AI_PROVIDERS: Record<"chatgpt" | "claude" | "gemini", AiProviderConfig> = {
  chatgpt: {
    label: "ChatGPT",
    queryUrl: (text) => `https://chatgpt.com/?q=${encodeURIComponent(text)}`,
    homepageUrl: "https://chatgpt.com/",
  },
  claude: { label: "Claude", homepageUrl: "https://claude.ai/new" },
  gemini: { label: "Gemini", homepageUrl: "https://gemini.google.com/app" },
};

function fillTemplate(template: string, text: string): string {
  return template.includes("{q}") ? template.split("{q}").join(encodeURIComponent(text)) : template;
}

function resolveAiProvider(): AiProviderConfig {
  if (settings.aiProvider === "custom") {
    const template = settings.customAiUrlTemplate;
    return {
      label: "AI",
      queryUrl: template.includes("{q}") ? (text) => fillTemplate(template, text) : undefined,
      homepageUrl: template || "about:blank",
    };
  }
  // Older Rust builds serialized ChatGpt as "chat-gpt". Accept it while
  // preserving a safe default for any future/invalid provider value.
  const provider = settings.aiProvider === "chat-gpt" ? "chatgpt" : settings.aiProvider;
  return AI_PROVIDERS[provider] ?? AI_PROVIDERS.chatgpt;
}

async function askAiAboutSelection(): Promise<void> {
  const text = currentText();
  if (!text) return;
  const provider = resolveAiProvider();
  try {
    // Clipboard first: a query-prefill param is a best effort, not a
    // documented/stable contract, so leave a paste-ready fallback on the
    // clipboard regardless of whether the URL actually prefills anything.
    await clipboardSave;
    await writeText(text);
    await openUrl(provider.queryUrl ? provider.queryUrl(text) : provider.homepageUrl);
  } catch (err) {
    console.error("failed to open AI provider", err);
  }
}

/** Copy the captured pixels, not the overlay UI. A freehand crop retains a
 * transparent exterior; the rectangle retains its rounded corners.
 * `redactBoxes` (frame-pixel space, same as everything else here) paints
 * solid covers over the crop before encoding — called again with the
 * current redaction boxes whenever Redact finds something, so a saved or
 * copied image can never leak what's already hidden on screen. */
function saveImageSelection(points: Point[], shape: SelectionTool, converted = false, redactBoxes: Box[] = []): Promise<Blob | null> {
  const region = boundsOf(points);
  const pad = converted ? CARD_PADDING : 0;
  const left = Math.max(0, Math.floor(region.x - pad));
  const top = Math.max(0, Math.floor(region.y - pad));
  const right = Math.min(frameCanvas.width, Math.ceil(region.x + region.width + pad));
  const bottom = Math.min(frameCanvas.height, Math.ceil(region.y + region.height + pad));
  if (right <= left || bottom <= top) return Promise.resolve(null);
  const crop = document.createElement("canvas");
  crop.width = right - left;
  crop.height = bottom - top;
  const ctx = crop.getContext("2d", { alpha: true });
  if (!ctx) return Promise.resolve(null);
  // The offscreen canvas starts transparent. Mask the screenshot with alpha,
  // rather than drawing the dimmed overlay or filling the exterior in black.
  ctx.drawImage(frameCanvas, left, top, crop.width, crop.height, 0, 0, crop.width, crop.height);
  ctx.save();
  ctx.globalCompositeOperation = "destination-in";
  ctx.fillStyle = "#fff";
  ctx.translate(-left, -top);
  if (shape === "lasso") {
    traceLassoPath(ctx, points);
  } else {
    roundedRectPath(ctx, region.x - pad, region.y - pad,
      region.width + pad * 2, region.height + pad * 2, spotlightCardRadius);
  }
  ctx.fill();
  ctx.restore();

  // Painted with the default composite (not destination-in), so it's fully
  // opaque regardless of the mask above or whatever pixels were underneath.
  if (redactBoxes.length > 0) {
    ctx.save();
    ctx.fillStyle = "#0b0b0f";
    for (const box of redactBoxes) {
      ctx.fillRect(box.x - left, box.y - top, box.width, box.height);
    }
    ctx.restore();
  }

  // Encode once for the action buttons, Save PNG, and clipboard. Keep the
  // button path independent of the clipboard plugin, which may fail or lag.
  const generation = ++imageGeneration;
  const cropRect: Box = { x: left, y: top, width: crop.width, height: crop.height };
  const imageReady = new Promise<Blob | null>((resolve, reject) => {
    const encodeFallback = () => {
      try {
        const dataUrl = crop.toDataURL("image/png");
        const encoded = dataUrl.split(",", 2)[1];
        if (!encoded) throw new Error("Canvas returned an empty PNG");
        const bytes = Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));
        resolve(new Blob([bytes], { type: "image/png" }));
      } catch (err) {
        reject(err);
      }
    };
    try {
      crop.toBlob((blob) => blob ? resolve(blob) : encodeFallback(), "image/png");
    } catch {
      encodeFallback();
    }
  });
  void imageReady.then((blob) => {
    if (generation !== imageGeneration) return;
    latestPngBlob = blob;
    latestCropRect = cropRect;
  }, () => {});

  // Serialize clipboard writes so an older selection cannot replace a newer
  // one. Encoding above can still finish while the previous write is pending.
  clipboardSave = clipboardSave.then(async () => {
    const blob = await imageReady;
    if (!blob) throw new Error("PNG encoding failed");
    const image = await TauriImage.fromBytes(new Uint8Array(await blob.arrayBuffer()));
    try {
      await writeImage(image);
    } finally {
      await image.close();
    }
    return true;
  }).catch((err) => {
    console.error("selection clipboard save failed", err);
    return false;
  });
  return imageReady;
}

async function pngDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => typeof reader.result === "string"
      ? resolve(reader.result)
      : reject(new Error("PNG could not be read"));
    reader.onerror = () => reject(reader.error ?? new Error("PNG could not be read"));
    reader.readAsDataURL(blob);
  });
}

async function savePng(): Promise<void> {
  await clipboardSave;
  const blob = latestPngBlob;
  const box = activeBox;
  if (!blob || !box) {
    showHint("PNG isn't ready yet", "Try another selection", 2600, box ?? undefined);
    return;
  }
  try {
    const saved = await invoke<boolean>("save_selection_png", { pngDataUrl: await pngDataUrl(blob) });
    if (saved) {
      showHint("Image saved", "Esc to close", 7000, box, {
        label: "Show in folder",
        run: () => {
          // The overlay is always-on-top and would cover Explorer.
          void invoke("reveal_saved_image").catch((err) => console.error("reveal failed", err));
          void invoke("dismiss_overlay");
        },
      });
    } else {
      showHint("Save cancelled", "Esc to close", 1800, box);
    }
  } catch (err) {
    console.error("PNG save failed", err);
    showHint("Couldn't save the PNG", "Choose a .png file and try again", 3000, box);
  }
}

/** Free. Reuses the exact crop `save-png` does — same `latestPngBlob` — so
 * a pinned window always shows exactly what's currently on screen for this
 * selection, freehand mask and all. Screen
 * placement itself is entirely Rust's call (`pin.rs`'s `create_pin`): this
 * only ever sends frame-relative coordinates, never a screen position.
 *
 * Dismisses the capture overlay on success (unlike save-png/copy, which
 * leave it up) — the overlay is full-screen, always-on-top and still armed
 * for another selection, so leaving it open would sit on top of the new
 * always-on-top pin window and swallow every click meant for it, making
 * the pin look frozen/unresizable even though it isn't. */
async function pinSelection(): Promise<void> {
  await clipboardSave;
  const blob = latestPngBlob;
  const crop = latestCropRect;
  const box = activeBox;
  if (!blob || !crop || !box) {
    showHint("Nothing to pin yet", "Try another selection", 2600, box ?? undefined);
    return;
  }
  try {
    // The crop's own rect, not activeBox — so the pin lands exactly over the
    // pixels it shows, padding and screen-edge clamping included.
    await invoke("create_pin", { imageDataUrl: await pngDataUrl(blob), region: crop });
    await dismiss();
  } catch (err) {
    console.error("pin failed", err);
    showHint("Couldn't pin this selection", "Try again or press Esc", 3000, box);
  }
}

/** Free. Reuses the exact crop save-png/pin do — same `latestPngBlob`, same
 * `activeBox`. The native share sheet is anchored to its own small
 * dedicated window (share.rs's `share-anchor`), not the overlay — Windows
 * Shell surfaces won't attach to the overlay's transparent/always-on-top/
 * skip-taskbar combination — so unlike Pin there's no ordering constraint
 * here: the overlay can dismiss right away. That anchor window is left
 * open for the user to close once they're done (see share.rs's module doc
 * for why it isn't torn down from here). */
async function shareSelection(): Promise<void> {
  await clipboardSave;
  const blob = latestPngBlob;
  const box = activeBox;
  if (!blob || !box) {
    showHint("Nothing to share yet", "Try another selection", 2600, box ?? undefined);
    return;
  }
  try {
    await invoke("share_selection", { imageDataUrl: await pngDataUrl(blob) });
    await dismiss();
  } catch (err) {
    console.error("share failed", err);
    showHint("Couldn't open the share menu", "Try again or press Esc", 3000, box);
  }
}

/** Opens the AI provider as a small chromeless window when the default
 * browser supports it (see aichat.rs's `open_ai_provider`) — that still runs
 * inside the browser's own real profile, so an already-logged-in session
 * carries over the way it would in a normal tab; an app-owned window would
 * start from a separate, empty WebView2 profile and force a fresh login
 * every time. The image is already on the clipboard by the time this runs;
 * Rust takes over from there to paste it in once that window looks focused
 * (schedule_ai_paste — this app has no handle to a window it didn't create,
 * so it's a timed best-effort rather than a guarantee). */
async function askAiAboutImage(): Promise<void> {
  try {
    if (!await clipboardSave) throw new Error("Selection image was not copied");
    const provider = resolveAiProvider();
    await invoke("open_ai_provider", { url: provider.homepageUrl });
    void invoke("schedule_ai_paste", { providerLabel: provider.label });
    await dismiss();
  } catch (err) {
    console.error("failed to open the AI provider", err);
    showHint("Couldn't open the AI provider", "Try again or press Esc", 3000, activeBox ?? undefined);
  }
}

async function translateSelection(): Promise<void> {
  const text = currentText();
  if (!text) return;
  try {
    const lang = encodeURIComponent(settings.behavior.translateTargetLang || "en");
    await openUrl(`https://translate.google.com/?sl=auto&tl=${lang}&text=${encodeURIComponent(text)}&op=translate`);
  } catch (err) {
    console.error("failed to open translate", err);
  }
}

const SEARCH_ENGINE_URLS: Record<Exclude<SearchEngine, "custom">, string> = {
  google: "https://www.google.com/search?q=",
  bing: "https://www.bing.com/search?q=",
  "duck-duck-go": "https://duckduckgo.com/?q=",
  perplexity: "https://www.perplexity.ai/search?q=",
};

async function searchSelection(): Promise<void> {
  const text = currentText();
  if (!text) return;
  try {
    const base = settings.searchEngine === "custom" ? settings.customSearchUrlTemplate : SEARCH_ENGINE_URLS[settings.searchEngine];
    const url = settings.searchEngine === "custom" && base.includes("{q}")
      ? fillTemplate(base, text)
      : `${base}${encodeURIComponent(text)}`;
    await openUrl(url);
  } catch (err) {
    console.error("failed to open search", err);
  }
}

async function reverseImageSearch(): Promise<void> {
  const token = lensRequestToken;
  messagePanel.hidden = true;
  try {
    const imageDataUrl = await imagePreparation;
    if (token !== lensRequestToken) return;
    if (!imageDataUrl) {
      showHint("Image crop unavailable", "Select Text still works; draw again to retry", 3500, activeBox ?? undefined);
      return;
    }
    const resultsUrl = await invoke<string>("search_with_lens", { imageDataUrl });
    if (token !== lensRequestToken) return;
    await openUrl(resultsUrl);
    await dismiss();
  } catch (err) {
    if (token !== lensRequestToken) return;
    console.error("Google Lens search failed", err);
    const detail = typeof err === "string" && err.length <= 120
      ? err
      : "Check your connection and try again.";
    showMessage("Lens search failed.", `${detail} Press Esc to dismiss.`);
  }
}

/** For actions that leave the app (open a URL): run it, then close the
 * overlay — there's nothing left to select once the user's attention has
 * moved to the browser. Copy-style actions skip this so their button can
 * flash "Copied" and the user can still act on the same selection again. */
async function runThenDismiss(action: () => Promise<void>): Promise<void> {
  await action();
  await dismiss();
}

const languageNames = new Intl.DisplayNames(["en"], { type: "language" });
function languageName(tag: string): string {
  try {
    return languageNames.of(tag) ?? tag;
  } catch {
    return tag;
  }
}

void listen<{ phase: string; sourceLang: string }>("translation-status", (event) => {
  if (event.payload.phase === "downloading" && activeBox) {
    showHint(`Downloading the ${languageName(event.payload.sourceLang)} language pack…`, "One-time, stays on this device", 0, activeBox);
  }
});

/** Translates every paragraph of the circled region on-device and paints
 * the result over the original. Leaves the overlay open. */
async function translateInPlace(): Promise<void> {
  const paragraphs = paragraphsForTranslation();
  const box = activeBox;
  if (paragraphs.length === 0 || !box) return;
  const token = selectionToken;
  const target = settings.behavior.translateTargetLang || "en";
  hideActionBar();
  showHint("Translating…", "Esc to cancel", 0, box);
  try {
    const result = await invoke<{ sourceLang: string; targetLang: string; translations: string[] }>(
      "translate_paragraphs",
      { paragraphs: paragraphs.map((paragraph) => paragraph.text), target },
    );
    if (token !== selectionToken) return;
    if (result.sourceLang === result.targetLang) {
      showHint(`This is already in ${languageName(result.targetLang)}`, "Esc to close", 2800, box);
      return;
    }
    showTranslation(paragraphs, result.translations, result.targetLang);
    showHint(`${languageName(result.sourceLang)} → ${languageName(result.targetLang)}`, "Translated on this device", 0, box);
    showActionBar(box, [
      { label: "Copy translation", icon: "copy", primary: true, onClick: copyTranslation },
      {
        label: "Show original",
        icon: "undo",
        onClick: () => {
          clearTranslation();
          hideActionBar();
          showHint("Drag across text to select", "Esc to close", 0, box);
        },
      },
    ]);
  } catch (err) {
    if (token !== selectionToken) return;
    console.error("translation failed", err);
    showHint(typeof err === "string" ? err : "Couldn't translate this", "Esc to close", 4000, box);
  }
}

async function copyTranslation(): Promise<string | void> {
  const text = translatedText();
  if (!text) return;
  try {
    await clipboardSave;
    await writeText(text);
    return "Copied";
  } catch (err) {
    console.error("clipboard write failed", err);
    return "Couldn't copy";
  }
}

const CHOICE_ACTION_BUILDERS: Record<string, () => ActionBarAction | undefined> = {
  "select-text": () => ({
    label: "Select Text",
    icon: "text",
    description: "Find text inside this lasso and let you select exactly what you need",
    onClick: () => void beginTextSelection(),
  }),
  "translate-here": () => ({
    label: "Translate",
    icon: "translate",
    description: "Find the text in this selection and translate it in place, on this device",
    busyLabel: "Translating…",
    onClick: async () => {
      await beginTextSelection();
      if (state === "text-ready") await translateInPlace();
    },
  }),
  lens: () => ({
    label: "Google Lens",
    icon: "lens",
    description: "Search this selected image with Google Lens",
    busyLabel: "Opening Lens…",
    onClick: reverseImageSearch,
  }),
  "image-to-ai": () => ({
    label: `Image to ${resolveAiProvider().label}`,
    icon: "ai",
    description: `Copy the selected image and open ${resolveAiProvider().label}`,
    busyLabel: "Opening…",
    onClick: askAiAboutImage,
  }),
  "save-png": () => ({
    label: paid ? "Save image" : "Save PNG",
    icon: "save",
    description: paid
      ? "Save just the selected pixels as PNG, JPEG or WebP"
      : "Save just the selected pixels as a transparent PNG",
    busyLabel: "Saving…",
    onClick: savePng,
  }),
  pin: () => ({
    label: "Pin to screen",
    icon: "pin",
    description: "Keep this selection floating on top of everything else",
    busyLabel: "Pinning…",
    onClick: pinSelection,
  }),
  share: () => ({
    label: "Share",
    icon: "share",
    description: "Share this selection with another app",
    busyLabel: "Opening…",
    onClick: shareSelection,
  }),
};

const TEXT_ACTION_BUILDERS: Record<string, () => ActionBarAction | undefined> = {
  copy: () => ({ label: "Copy", icon: "copy", primary: true, description: "Copy the selected text", onClick: copySelection }),
  "copy-code": () => ({
    label: "Copy as code",
    icon: "code",
    flashLabel: "Copied",
    description: "Rebuild indentation and copy this selection as code",
    onClick: copyAsCode,
  }),
  redact: () => paid
    ? {
        label: "Redact",
        icon: "redact",
        busyLabel: "Scanning…",
        description: "Find and cover emails, phone numbers, card numbers, API keys and IPs before you copy or share",
        onClick: redactSelection,
      }
    : undefined,
  "translate-inline": () => ({
    label: "Translate here",
    icon: "translate",
    busyLabel: "Translating…",
    description: "Translate this selection in place, on this device",
    onClick: translateInPlace,
  }),
  search: () => ({ label: "Search", icon: "search", description: "Search the web for the selected text", onClick: () => void runThenDismiss(searchSelection) }),
  // Distinct icon from translate-inline on purpose (see actionbar.ts) — this
  // one leaves the overlay for Google Translate, that one stays in place.
  translate: () => ({ label: "Translate", icon: "external", description: "Open this selection in Google Translate", onClick: () => void runThenDismiss(translateSelection) }),
  "ask-ai": () => ({
    label: `Ask ${resolveAiProvider().label}`,
    icon: "ai",
    description: `Ask ${resolveAiProvider().label} about the selected text`,
    onClick: () => void runThenDismiss(askAiAboutSelection),
  }),
};

function hostOf(template: string): string {
  try {
    return new URL(template.split("{q}").join("x")).hostname;
  } catch {
    return "this link";
  }
}

/** A user-defined action (Pro). Returns nothing on the Free plan, so a
 * lapsed subscription hides custom buttons without deleting them. */
function customActionBuilder(id: string): (() => ActionBarAction) | undefined {
  const action = paid ? settings.customActions.find((candidate) => candidate.id === id) : undefined;
  if (!action) return undefined;
  return () => ({
    label: action.label,
    description: `Open ${hostOf(action.urlTemplate)} with the selected text`,
    onClick: () => void runThenDismiss(async () => {
      const text = currentText();
      if (!text) return;
      try {
        await openUrl(fillTemplate(action.urlTemplate, text));
      } catch (err) {
        console.error("custom action failed", err);
      }
    }),
  });
}

// `undefined` is a valid builder result (not just a valid *lookup* result,
// which customActionBuilder's own return type already allowed) so a
// built-in Pro action — not just a user-defined custom one — can hide
// itself entirely on Free rather than only being relabeled, the way
// save-png does. Every Pro-gated built-in (redact, the designer toolkit's
// bar entries, math, inline AI) follows this same pattern.
function actionsFrom(entries: ActionEntry[], builders: Record<string, () => ActionBarAction | undefined>): ActionBarAction[] {
  return entries.flatMap((entry) => {
    if (!entry.enabled) return [];
    try {
      const action = (builders[entry.id] ?? customActionBuilder(entry.id))?.();
      return action ? [action] : [];
    } catch (err) {
      console.error(`could not build action ${entry.id}`, err);
      return [];
    }
  });
}

function showChoiceActions(box: Box): void {
  // A selection exists now, so there is something to brush.
  setBrushAvailable(paid);
  showActionBar(box, actionsFrom(settings.choiceActions, CHOICE_ACTION_BUILDERS), "choices");
}

function showTextActions(box: Box): void {
  showActionBar(box, actionsFrom(settings.textActions, TEXT_ACTION_BUILDERS));
}

async function prepareRegion(points: Point[], imageReady: Promise<Blob | null>, box: Box): Promise<string | null> {
  const token = ++selectionToken;
  lensRequestToken++;
  state = "choosing";
  activeBox = box;
  // Text selection does not require PNG encoding. Show every action as soon as
  // the user releases the pointer; image actions await imagePreparation.
  showChoiceActions(box);
  try {
    const blob = await imageReady;
    if (!blob) throw new Error(`Canvas returned no PNG (${frameCanvas.width}×${frameCanvas.height} frame)`);
    const imageDataUrl = await pngDataUrl(blob);
    return imageDataUrl;
  } catch (browserError) {
    // The native capture is already cached. Use it as a recovery path when
    // WebView2 cannot encode an offscreen canvas, without blocking Select Text.
    try {
      const native = await invoke<PreparedRegion>("prepare_region", {
        points: points.map((point): [number, number] => [point.x, point.y]),
      });
      if (!native.imageDataUrl) throw new Error("No cached frame or valid crop bounds");
      console.warn("Canvas PNG unavailable; using native crop", browserError);
      return native.imageDataUrl;
    } catch (nativeError) {
      if (token !== selectionToken) return null;
      console.error("selection image failed in both paths", { browserError, nativeError });
      const detail = `${String(browserError)}; ${String(nativeError)}`.slice(0, 150);
      showHint("Image unavailable", detail, 5500, box);
      return null;
    }
  }
}

/** A freehand lasso's *stroke* morphs into a CARD_PADDING-padded rounded
 * rectangle on Select Text (lasso.convertToRectangle, purely visual) — this
 * returns the same padded rectangle as an OCR hit path, so anything that
 * runs OCR on a lasso region sees the boundary the user actually
 * ends up looking at, not the narrower original squiggle. The stroke
 * (lasso.ts), `saveImageSelection`'s converted crop and this all take their
 * padding from the same setting, so they stay the same shape. Rectangle-drawn
 * regions pass through unchanged — they already match what's on screen. */
function ocrHitSurface(path: Point[], shape: SelectionTool): { path: Point[]; shape: SelectionTool } {
  if (shape !== "lasso") return { path, shape };
  const bounds = boundsOf(path);
  return {
    path: boxPoints({
      x: bounds.x - CARD_PADDING,
      y: bounds.y - CARD_PADDING,
      width: bounds.width + CARD_PADDING * 2,
      height: bounds.height + CARD_PADDING * 2,
    }),
    shape: "rectangle",
  };
}

async function beginTextSelection(): Promise<void> {
  if (!activeBox || state !== "choosing") return;
  // Until this fix, the actual OCR hit-test and selectable-text clip below
  // still used the original narrow freehand polygon even after the stroke
  // itself morphed into a wider rounded rectangle — the card visually
  // promised everything inside that rectangle was selectable, while only
  // whatever the original squiggle touched actually was. Recomputing
  // before it's read below keeps every consumer (OCR, the selectable card,
  // and any later re-crop e.g. Redact) in sync with what's on screen.
  if (activeOcrPath) {
    const converted = ocrHitSurface(activeOcrPath, activeOcrShape);
    activeOcrPath = converted.path;
    if (converted.shape !== activeOcrShape) {
      activeOcrShape = converted.shape;
      activeOcrConverted = true;
    }
  }
  const box = activeBox;
  const hitPath = activeOcrPath ?? boxPoints(box);
  const hitShape = activeOcrShape;
  const token = ++selectionToken;
  state = "recognizing";
  toolBar.hidden = true;
  setBrushAvailable(false);
  hideActionBar();
  // Morph the current freehand contour into the same rectangle hitPath now
  // uses above. OCR can run in parallel, so the animation adds no OCR wait.
  const morphFinished = lasso.convertToRectangle();
  lasso.startRecognizing();
  showHint("Finding text in your selection…", "Esc to cancel", 0, box);
  try {
    const result = await invoke<TextSelectionResult>("recognize_text", {
      points: hitPath.map((point): [number, number] => [point.x, point.y]),
    });
    await morphFinished;
    if (token !== selectionToken) return;
    lasso.stopRecognizing();
    // Keep the user's drawn area intact. Only a substantial blank band earns
    // a small trim; the lasso never zooms into an OCR paragraph.
    const detectedBox = result.selections.length > 0 ? unionBoxOf(result.selections) : box;
    const textBox = trimSubstantialEmptyMargins(box, detectedBox);
    activeBox = textBox;
    lasso.settle(textBox);
    lasso.indicateTextMode();
    lastPlainText = result.selections.length > 0 ? materializeSelection(result.selections, textBox, hitPath, hitShape === "rectangle") : "";
    if (!lastPlainText) {
      showHint("No text found here", "Draw again or press Esc", 2800, box);
      state = "choosing";
      toolBar.hidden = false;
      showChoiceActions(box);
      return;
    }
    state = "text-ready";
    showHint("Drag across text to select", "Esc to close", 0, textBox);
  } catch (err) {
    await morphFinished;
    if (token !== selectionToken) return;
    lasso.stopRecognizing();
    console.error("selected-region OCR failed", err);
    state = "choosing";
    toolBar.hidden = false;
    showChoiceActions(box);
    showHint("Text selection could not start", "Try again or press Esc", 2800, box);
  }
}

function boxPoints(box: Box): Point[] {
  return [
    { x: box.x, y: box.y },
    { x: box.x + box.width, y: box.y },
    { x: box.x + box.width, y: box.y + box.height },
    { x: box.x, y: box.y + box.height },
  ];
}

function sizeCanvases(width: number, height: number): void {
  frameCanvas.width = width;
  frameCanvas.height = height;
  lassoCanvas.width = width;
  lassoCanvas.height = height;
  // Assigning width/height clears a canvas's backing buffer, so the flat
  // fill has to be redrawn after resizing rather than before.
  spotlightCanvas.width = width;
  spotlightCanvas.height = height;
  fillSpotlight();
  lasso.resize();
  resizeDesignerLayer();
  resizeBrushLayer();
}

let frameGeneration = 0;
let pendingFrame: HTMLImageElement | null = null;
let spotlightFrame = 0;

function cancelPendingFrame(): void {
  frameGeneration++;
  if (spotlightFrame) cancelAnimationFrame(spotlightFrame);
  spotlightFrame = 0;
  if (!pendingFrame) return;
  pendingFrame.onload = pendingFrame.onerror = null;
  pendingFrame.removeAttribute("src");
  pendingFrame = null;
}

function revealSpotlight(): void {
  if (spotlightFrame) cancelAnimationFrame(spotlightFrame);
  const generation = frameGeneration;
  spotlightFrame = requestAnimationFrame(() => {
    spotlightFrame = 0;
    if (generation === frameGeneration && state !== "idle") spotlightCanvas.classList.add("visible");
  });
}

function showFrame(payload: FramePayload): void {
  cancelPendingFrame();
  const generation = frameGeneration;
  setDesignerDpiScale(payload.dpiScale);
  const img = new Image();
  pendingFrame = img;
  img.onload = () => {
    if (generation !== frameGeneration || state === "idle") return;
    pendingFrame = null;
    img.onload = img.onerror = null;
    sizeCanvases(payload.width, payload.height);
    frameCtx?.drawImage(img, 0, 0, payload.width, payload.height);
    img.removeAttribute("src");
    toolBar.hidden = false;
    showHint("Draw around text or an image", "Esc to close");
    siriGlow.start();
    scanScreen(payload.width, payload.height);
    // rAF so the browser paints the frame before the dim transition starts,
    // otherwise the opacity change and the image paint can land in the same
    // frame and read as a flash instead of a fade.
    revealSpotlight();
  };
  img.onerror = () => {
    if (generation !== frameGeneration) return;
    pendingFrame = null;
    img.onload = img.onerror = null;
    img.removeAttribute("src");
    console.error("failed to decode frame image");
    showMessage("Couldn't display this capture.", "Press Esc and try again.");
  };
  img.src = payload.dataUrl;
}

function showMessage(text: string, hint: string): void {
  overlayClose.hidden = false;
  messagePanel.replaceChildren(document.createTextNode(text));
  const hintEl = document.createElement("span");
  hintEl.className = "hint";
  hintEl.textContent = hint;
  messagePanel.appendChild(hintEl);
  messagePanel.hidden = false;
  toolBar.hidden = true;
  siriGlow.start();
  revealSpotlight();
}

function reset(): void {
  cancelPendingFrame();
  endLiveText();
  setBrushAvailable(false);
  selectionToken++;
  lensRequestToken++;
  state = "idle";
  siriGlow.release();
  hideHint();
  messagePanel.hidden = true;
  messagePanel.replaceChildren();
  spotlightCanvas.classList.remove("visible");
  clearReveal();
  lasso.clear();
  selectionTool = "lasso";
  lasso.setTool("lasso");
  lassoButton.setAttribute("aria-pressed", "true");
  rectangleButton.setAttribute("aria-pressed", "false");
  setDesignerMode(null);
  leaveBrush();
  eyedropperButton.setAttribute("aria-pressed", "false");
  measureButton.setAttribute("aria-pressed", "false");
  editMode = false;
  skipSnapOnce = false;
  editButton.setAttribute("aria-pressed", "false");
  toolBar.hidden = true;
  islandTools = false;
  document.body.classList.remove("island-tools");
  clearDebugOverlay();
  overlayClose.hidden = true;
  clearTextLayer();
  clearTranslation();
  clearRedactions();
  redactedText = null;
  hideActionBar();
  lastPlainText = "";
  activeBox = null;
  activePath = null;
  activeOcrPath = null;
  latestPngBlob = null;
  imageGeneration++;
  imagePreparation = Promise.resolve(null);
  activeOcrShape = "lasso";
  activeOcrConverted = false;
  // The overlay sits hidden between sessions; full-monitor canvases would
  // keep tens of MB of backing store alive for nothing. Shrinking them frees
  // it, and the next frame sizes them again anyway (showFrame).
  sizeCanvases(1, 1);
}

async function dismiss(): Promise<void> {
  // A dev hot reload can reset the WebView while its native overlay window is
  // still open. Escape must remain able to dismiss that otherwise empty window.
  if (dismissing) return;
  dismissing = true;
  // Post the native hide first. Clipboard encoding/writes own their crop
  // snapshot and can finish after dismissal; they must never hold it open.
  try {
    const closing = invoke("dismiss_overlay");
    reset();
    await closing;
  } catch (err) {
    console.error("dismiss_overlay failed", err);
  } finally {
    dismissing = false;
  }
}

// Closed from Rust (the hotkey pressed again) — reset exactly as Escape does.
void listen("overlay-dismissed", () => {
  if (state !== "idle") reset();
});

void listen<FramePayload>("frame-ready", (event) => {
  state = "armed";
  setIslandTools(event.payload.islandTools === true);
  overlayClose.hidden = islandTools;
  applyIslandLook(event.payload.islandLook);
  showFrame(event.payload);
});

// The bars wear the island's look: black, or glass with its tint, frosted
// over the frozen screen when Windows' transparency effects are on.
function applyIslandLook(look: FramePayload["islandLook"]): void {
  const glass = look?.glass === true;
  document.body.classList.toggle("island-glass", glass);
  document.body.classList.toggle("island-frost", glass && look?.frost !== false);
  document.body.style.setProperty("--island-tint", String(look?.tint ?? 0.45));
}

// Onyx's island as the toolbar. The island moves to the top of the screen
// and shows these same buttons; this page's own toolbar stays in the DOM
// (hidden by CSS) as the one source of which tools exist and are on, so
// every existing show/hide/press keeps working and is mirrored from here.
const ISLAND_TOOLS = [
  lassoButton, rectangleButton, fullScreenButton, eyedropperButton,
  measureButton, editButton, brushButton, brushEraseButton,
];
const ISLAND_CLOSE = ISLAND_TOOLS.length;
let islandTools = false;
let islandSyncQueued = false;

function setIslandTools(on: boolean): void {
  islandTools = on;
  // The report goes out when the toolbar appears (the observer below), so
  // the island opens straight into the tools.
  document.body.classList.toggle("island-tools", on);
}

/** Reports the toolbar to the island, once per batch of DOM changes. */
function syncIslandTools(): void {
  if (!islandTools || islandSyncQueued) return;
  islandSyncQueued = true;
  queueMicrotask(() => {
    islandSyncQueued = false;
    if (!islandTools) return;
    let visible = 0;
    let pressed = 0;
    if (!toolBar.hidden) {
      ISLAND_TOOLS.forEach((button, index) => {
        if (button.hidden) return;
        visible |= 1 << index;
        if (button.getAttribute("aria-pressed") === "true") pressed |= 1 << index;
      });
    }
    void invoke("island_tools", { visible, pressed }).catch(() => {});
  });
}

new MutationObserver(syncIslandTools).observe(toolBar, {
  subtree: true,
  attributes: true,
  attributeFilter: ["hidden", "aria-pressed"],
});

void listen<number>("island-tool", (event) => {
  if (!islandTools || state === "idle") return;
  if (event.payload === ISLAND_CLOSE) {
    void dismiss();
    return;
  }
  const button = ISLAND_TOOLS[event.payload];
  if (button && !button.hidden && !toolBar.hidden) button.click();
});

// Windows Graphics Capture returns a black frame for protected content
// rather than failing, so there's no frame to paint and nothing to select —
// say why instead of showing a black screen. Still armed, so Esc and
// right-click dismiss it the same as any other session.
void listen("capture-blocked", () => {
  cancelPendingFrame();
  state = "armed";
  setIslandTools(false);
  hideHint();
  showMessage(
    "This screen can't be captured.",
    "Protected content or an exclusive-fullscreen app is blocking it. Press Esc to dismiss.",
  );
});

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    void dismiss();
    return;
  }

  if (e.ctrlKey && e.key.toLowerCase() === "a" && state === "text-ready") {
    e.preventDefault();
    selectAllText();
    return;
  }

  // Always use the same, line-aware text as the Copy button. Native copy of
  // positioned overlay elements can include UI labels or lose line breaks.
  if (e.ctrlKey && e.key.toLowerCase() === "c" && state === "text-ready" && selectedText()) {
    e.preventDefault();
    void copySelection();
  }
});

window.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  // Right-click normally closes the overlay outright, but Measure treats it
  // as "start over" instead — a ruler is disposable by nature (you're not
  // about to lose a selection), and re-arming Escape's own dismiss for
  // every mis-click would make it too easy to lose the whole capture while
  // just trying to redo a measurement.
  if (isBrushOn()) {
    leaveBrush();
    return;
  }
  const designerMode = currentDesignerMode();
  if (designerMode === "measure") {
    setDesignerMode(designerMode);
    return;
  }
  void dismiss();
});
