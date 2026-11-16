// Shared glyphs for the capture overlay and its settings preview.

export type IconName =
  | "text" | "translate" | "lens" | "ai" | "save" | "copy" | "search" | "pin" | "share" | "code" | "redact"
  | "math" | "sparkle" | "external" | "undo" | "plus"
  | "rectangle" | "lasso" | "fullscreen" | "eyedropper" | "ruler" | "magnet" | "brush" | "eraser";

const ICON_PATHS: Record<IconName, string> = {
  text: "M4 5h16M12 5v14M8 19h8",
  translate: "M3 6h11M8.5 4v2M5 9c1.8 3.5 4.2 5.7 8 7M12 9c-.8 2.1-2.3 4.3-4.4 6M14 19l3-8 3 8M15 17h4",
  lens: "M4 9V6a2 2 0 0 1 2-2h3M15 4h3a2 2 0 0 1 2 2v3M20 15v3a2 2 0 0 1-2 2h-3M9 20H6a2 2 0 0 1-2-2v-3M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  ai: "m12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2ZM19 17l.5 1.5L21 19l-1.5.5L19 21l-.5-1.5L17 19l1.5-.5L19 17Z",
  save: "M12 3v12m-4-4 4 4 4-4M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3",
  copy: "M8 7V5a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-2M5 7h9a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2Z",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14Zm5-2 5 5",
  pin: "M12 2v6.5M8 8.5h8l1.5 5H6.5l1.5-5ZM12 13.5V22",
  share: "M12 3v12M8 7l4-4 4 4M5 13v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6",
  code: "m9 8-4 4 4 4m6-8 4 4-4 4M13.5 5.5l-3 13",
  redact: "M3 7h18M3 12h18M3 17h11",
  math: "M4 4h6M7 4v6M13 6h7M14 12l6 6m0-6-6 6M6 15h4M6 19h4",
  sparkle: "m12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2ZM19 17l.5 1.5L21 19l-1.5.5L19 21l-.5-1.5L17 19l1.5-.5L19 17Z",
  // Distinct from `translate` on purpose: "Translate here" (in place) and
  // "Translate" (opens Google Translate) share a subject but not a result,
  // and icon-only buttons need every glyph to mean one specific thing.
  external: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3",
  undo: "M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11",
  plus: "M12 5v14M5 12h14",
  rectangle: "M7 5h10a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V8a3 3 0 0 1 3-3Z",
  lasso: "M12 4c4.4 0 8 2.5 8 5.5S16.4 15 12 15s-8-2.5-8-5.5S7.6 4 12 4ZM7.5 14c-.8 1.2-.6 2.8.6 3.6 1 .7 2.4.6 3.2-.3M11 17.5 10 21",
  fullscreen: "M4 9V5a1 1 0 0 1 1-1h4M15 4h4a1 1 0 0 1 1 1v4M20 15v4a1 1 0 0 1-1 1h-4M9 20H5a1 1 0 0 1-1-1v-4",
  eyedropper: "m14 7 3 3M4.5 19.5l1-3L15 7l2-2a2.1 2.1 0 0 1 3 3l-2 2-9.5 9.5-3 1Z",
  ruler: "M3 16 16 3l5 5L8 21l-5-5ZM7 12l2 2M10 9l2 2M13 6l2 2",
  magnet: "m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15ZM5 8l4 4M12 15l4 4",
  brush: "M9.5 14.5 18 6a1.9 1.9 0 0 1 2.7 2.7l-8.5 8.5M9.5 14.5c-2 0-3.5 1.5-3.5 3.5 0 1-.7 2-2 2 1 .7 2.3 1 3.5 1 2.5 0 4.2-1.8 4.2-4.3",
  eraser: "M7 20h12M5.5 15.5 14 7a2 2 0 0 1 2.8 0l2.2 2.2a2 2 0 0 1 0 2.8L12 19H8.5l-3-3a1 1 0 0 1 0-1.5ZM10 11l5 5",
};

export function actionIcon(name: IconName): SVGSVGElement {
  const namespace = "http://www.w3.org/2000/svg";
  const icon = document.createElementNS(namespace, "svg");
  icon.setAttribute("viewBox", "0 0 24 24");
  icon.setAttribute("fill", "none");
  icon.setAttribute("stroke", "currentColor");
  icon.setAttribute("stroke-width", "1.8");
  icon.setAttribute("stroke-linecap", "round");
  icon.setAttribute("stroke-linejoin", "round");
  icon.setAttribute("aria-hidden", "true");
  icon.classList.add("action-icon");
  const path = document.createElementNS(namespace, "path");
  path.setAttribute("d", ICON_PATHS[name]);
  icon.append(path);
  return icon;
}

