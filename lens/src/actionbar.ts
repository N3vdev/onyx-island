// Floating action bar near the selection. Buttons act on whatever's
// selected directly on the overlay (native drag-select), falling back to
// the whole circled region if nothing's manually selected.

import { actionIcon, type IconName } from "./icons";
export { actionIcon, type IconName } from "./icons";

import { frameScale } from "./textselection";

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ActionBarAction {
  label: string;
  icon?: IconName;
  primary?: boolean;
  /** A string return overrides `flashLabel` for just that click — e.g.
   * "Copy as code" resolving the detected language into "Copied as Python"
   * rather than a fixed label decided before the work even ran. */
  onClick: () => void | Promise<void | string>;
  description?: string;
  busyLabel?: string;
  /** If set, the button's label briefly switches to this after a click —
   * e.g. "Copied" — before reverting. Ignored for a click whose `onClick`
   * returns its own flash text instead. */
  flashLabel?: string;
}

const actionBar = document.getElementById("action-bar") as HTMLDivElement;

const FLASH_DURATION_MS = 1200;
let anchor: Box | null = null;

/** Re-measure after labels, window size or zoom change, not only on opening. */
function positionActionBar(): void {
  if (!anchor || actionBar.hidden) return;
  const margin = 12;
  const available = Math.max(0, window.innerWidth - margin * 2);
  const count = actionBar.childElementCount;
  const maxColumns = Math.max(1, Math.min(count, 7, Math.floor(available / 104)));
  // Balance wrapped rows: seven actions become four + three, not six + one.
  const columns = Math.ceil(count / Math.ceil(count / maxColumns));
  actionBar.style.setProperty("--action-columns", String(columns));
  actionBar.style.setProperty("--action-width", `${Math.min(available, columns * 104 + 12)}px`);
  const scale = frameScale();
  const gap = actionBar.dataset.layout === "text" ? 24 : 14;
  const below = (anchor.y + anchor.height) * scale + gap;
  const left = (anchor.x + anchor.width / 2) * scale - actionBar.offsetWidth / 2;
  const maxLeft = Math.max(margin, window.innerWidth - actionBar.offsetWidth - margin);
  const maxTop = Math.max(margin, window.innerHeight - actionBar.offsetHeight - margin);
  const toolBar = document.getElementById("tool-bar");
  // With Onyx's island as the toolbar (main.ts), it sits at the top instead.
  const islandBottom = document.body.classList.contains("island-tools") ? 64 : 0;
  const toolBottom = toolBar && !toolBar.hidden && !islandBottom
    ? toolBar.getBoundingClientRect().bottom + margin
    : Math.max(margin, islandBottom);
  const above = anchor.y * scale - actionBar.offsetHeight - gap;
  const top = below + actionBar.offsetHeight <= window.innerHeight - margin
    ? below : above >= toolBottom ? above : maxTop;
  actionBar.style.left = `${Math.min(Math.max(margin, left), maxLeft)}px`;
  actionBar.style.top = `${Math.min(Math.max(margin, top), maxTop)}px`;
}

// Observers are event-driven; the hidden overlay does no polling work.
new ResizeObserver(positionActionBar).observe(actionBar);
window.addEventListener("resize", positionActionBar);
/** Center the controls outside the rounded selection, moving above it when
 * the lower edge is too close to the screen boundary. */
export function showActionBar(box: Box, actions: ActionBarAction[], layout: "text" | "choices" = "text"): void {
  anchor = box;
  actionBar.replaceChildren();
  if (!actions.length) { hideActionBar(); return; }
  actionBar.dataset.layout = layout;
  actionBar.dataset.count = String(actions.length);
  actionBar.setAttribute("role", "group");
  actionBar.setAttribute("aria-label", layout === "choices" ? "Selection actions" : "Text actions");

  for (const [index, action] of actions.entries()) {
    const button = document.createElement("button");
    button.type = "button";
    button.style.setProperty("--i", String(index));
    if (action.icon) button.append(actionIcon(action.icon));
    const label = document.createElement("span");
    label.className = "action-label";
    label.textContent = action.label;
    button.append(label);
    // Every button shows its label; the tooltip adds the longer description.
    button.title = action.description ?? action.label;
    button.setAttribute("aria-label", action.label);
    label.setAttribute("aria-live", "polite");
    if (action.primary) button.classList.add("primary");

    // Clicking the bar should not collapse a refined text selection before
    // its click handler reads the selected subset.
    button.addEventListener("pointerdown", (event) => event.preventDefault());

    button.addEventListener("click", async () => {
      if (action.busyLabel) {
        button.disabled = true;
        button.setAttribute("aria-busy", "true");
        button.classList.add("expanded");
        label.textContent = action.busyLabel;
      }
      // Tracks whether flashButton took over reverting the button, so the
      // busyLabel cleanup below can't stomp the flash text right back to
      // the idle label before FLASH_DURATION_MS ever elapses — that
      // actually happened for any action with both (Redact's "Found N" /
      // "Nothing found" was overwritten within the same microtask).
      let flashing = false;
      try {
        const result = await action.onClick();
        const flashText = typeof result === "string" ? result : action.flashLabel;
        if (flashText) {
          flashing = true;
          flashButton(button, label, action.label, flashText);
        }
      } catch (err) {
        console.error(`action ${action.label} failed`, err);
      } finally {
        button.removeAttribute("aria-busy");
        if (action.busyLabel && button.isConnected && !flashing) {
          button.disabled = false;
          button.classList.remove("expanded");
          label.textContent = action.label;
        }
      }
    });

    actionBar.appendChild(button);
  }

  actionBar.hidden = false;
  positionActionBar();
}

export function hideActionBar(): void {
  anchor = null;
  actionBar.hidden = true;
  actionBar.replaceChildren();
  delete actionBar.dataset.layout;
  delete actionBar.dataset.count;
}

function flashButton(button: HTMLButtonElement, label: HTMLSpanElement, original: string, flashLabel: string): void {
  label.textContent = flashLabel;
  button.disabled = true;
  // Both layouts keep labels visible; expanded styles the result feedback.
  button.classList.add("expanded");
  setTimeout(() => {
    if (!button.isConnected) return;
    label.textContent = original;
    button.disabled = false;
    button.classList.remove("expanded");
  }, FLASH_DURATION_MS);
}
