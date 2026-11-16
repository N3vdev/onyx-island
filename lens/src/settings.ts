// Settings window: loads persisted Settings from Rust, lets the user edit
// them, and saves back through the same get_settings/set_settings commands
// the overlay uses. No local defaults are hardcoded here beyond display
// labels — Settings::default() in Rust is the single source of truth.

import "@fontsource-variable/inter/wght.css";
import { isTauri, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type ActionEntry, type AiProvider, type CustomAction, type Entitlements, type LicenseStatus, type SearchEngine, type Settings } from "./settings-types";
import { barAccent, LASSO_STYLES, THEME_PRESETS } from "./themes";
import { actionIcon, type IconName } from "./icons";

document.querySelectorAll<HTMLElement>(".preview-actions [data-icon]").forEach(control => {
  control.prepend(actionIcon(control.dataset.icon as IconName));
});

const ACTION_ICONS: Record<string, IconName> = {
  "select-text": "text", "translate-here": "translate", lens: "lens", "image-to-ai": "ai",
  "save-png": "save", pin: "pin", share: "share", copy: "copy", "copy-code": "code",
  redact: "redact", "translate-inline": "translate", search: "search", translate: "external", "ask-ai": "ai",
};

const CHOICE_ACTION_LABELS: Record<string, string> = {
  "select-text": "Select Text",
  "translate-here": "Translate",
  lens: "Search using Google Lens",
  "image-to-ai": "Image to AI",
  "save-png": "Save PNG",
  pin: "Pin to screen",
  share: "Share",
};

const TEXT_ACTION_LABELS: Record<string, string> = {
  copy: "Copy",
  "copy-code": "Copy as code",
  redact: "Redact",
  "translate-inline": "Translate here",
  search: "Search",
  translate: "Open in Google Translate",
  "ask-ai": "Ask AI",
};

const shortcutCard = document.getElementById("shortcut-card") as HTMLDivElement;
const shortcutHint = document.getElementById("shortcut-hint") as HTMLSpanElement;
const shortcutBadgeText = document.getElementById("shortcut-badge-text") as HTMLSpanElement;
const DEFAULT_SHORTCUT_HINT = "Click to change your shortcut.";
const DEFAULT_SHORTCUT_BADGE = "Global";
const accentColor = document.getElementById("accent-color") as HTMLInputElement;
const spotlightDim = document.getElementById("spotlight-dim") as HTMLInputElement;
const spotlightDimValue = document.getElementById("spotlight-dim-value") as HTMLSpanElement;
const cardPadding = document.getElementById("card-padding") as HTMLInputElement;
const cardRadius = document.getElementById("card-radius") as HTMLInputElement;
const aiProviderSelect = document.getElementById("ai-provider") as HTMLSelectElement;
const aiCustomUrlRow = document.getElementById("ai-custom-url-row") as HTMLDivElement;
const aiCustomUrl = document.getElementById("ai-custom-url") as HTMLInputElement;
const searchEngineSelect = document.getElementById("search-engine") as HTMLSelectElement;
const searchCustomUrlRow = document.getElementById("search-custom-url-row") as HTMLDivElement;
const searchCustomUrl = document.getElementById("search-custom-url") as HTMLInputElement;
const choiceActionsList = document.getElementById("choice-actions-list") as HTMLOListElement;
const textActionsList = document.getElementById("text-actions-list") as HTMLOListElement;
const autoCopy = document.getElementById("auto-copy") as HTMLInputElement;
const hideCursor = document.getElementById("hide-cursor") as HTMLInputElement;
const translateLang = document.getElementById("translate-lang") as HTMLSelectElement;
const themePreset = document.getElementById("theme-preset") as HTMLSelectElement;
const lassoStyleSelect = document.getElementById("lasso-style") as HTMLSelectElement;
const customList = document.getElementById("custom-actions-list") as HTMLUListElement;
const customLabel = document.getElementById("custom-label") as HTMLInputElement;
const customUrl = document.getElementById("custom-url") as HTMLInputElement;
const customAdd = document.getElementById("custom-add") as HTMLButtonElement;
const launchAtStartup = document.getElementById("launch-at-startup") as HTMLInputElement;
const resetButton = document.getElementById("reset-button") as HTMLButtonElement;
const status = document.getElementById("status") as HTMLSpanElement;
const planHeading = document.getElementById("plan-heading") as HTMLHeadingElement;
const planSubheading = document.getElementById("plan-subheading") as HTMLParagraphElement;
const planEmail = document.getElementById("plan-email") as HTMLInputElement;
const planEmailHint = document.getElementById("plan-email-hint") as HTMLSpanElement;
const planStartTrial = document.getElementById("plan-start-trial") as HTMLButtonElement;
const planBuy = document.getElementById("plan-buy") as HTMLButtonElement;
const planRestore = document.getElementById("plan-restore") as HTMLButtonElement;
const planWaiting = document.getElementById("plan-waiting") as HTMLParagraphElement;
const planError = document.getElementById("plan-error") as HTMLParagraphElement;

// The IPC bridge is the reliable sign that this WebView can change the
// running desktop app. isTauri() alone can be absent in some dev WebViews.
const native = isTauri() || Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
const PREVIEW_STORAGE_KEY = "circle-search-settings-preview-v1";
// Browser previews have their own editable sample; Rust remains the source of
// defaults and persistence in the installed desktop app.
const previewDefaults: Settings = {
  version: 1,
  hotkey: "shift+enter",
  theme: { accentColor: "#080808", spotlightDim: 0.42, cardPadding: 10, cardRadius: 20, preset: "classic", lassoStyle: "rainbow" },
  behavior: { autoCopyOnSelectRelease: false, translateTargetLang: "en", hideCursor: false },
  aiProvider: "chatgpt",
  customAiUrlTemplate: "",
  searchEngine: "google",
  customSearchUrlTemplate: "",
  choiceActions: ["select-text", "translate-here", "lens", "image-to-ai", "save-png", "pin", "share"].map(id => ({ id, enabled: true })),
  textActions: ["copy", "copy-code", "redact", "translate-inline", "search", "ask-ai", "translate"].map(id => ({ id, enabled: id !== "translate" })),
  customActions: [],
  launchAtStartup: false,
};
function previewSettings(): Settings {
  try {
    const saved = localStorage.getItem(PREVIEW_STORAGE_KEY);
    if (saved) return { ...structuredClone(previewDefaults), ...JSON.parse(saved) as Partial<Settings> };
  } catch (error) {
    console.warn("Could not read browser preview settings", error);
  }
  return structuredClone(previewDefaults);
}
let current: Settings | null = null;
const saveState = document.getElementById("save-state")!;
resetButton.disabled = true;
let saveTimer: number | undefined;
let saveInFlight = false;
let currentSave: Promise<void> | null = null;
let changeVersion = 0;
let closing = false;
function scheduleSave(): void {
  if (saveTimer !== undefined) window.clearTimeout(saveTimer);
  if (closing) { saveTimer = undefined; return; }
  saveTimer = window.setTimeout(() => {
    saveTimer = undefined;
    void triggerSave();
  }, 650);
}
// There's no manual Save button anymore, so the debounce timer is the only
// thing that ever calls save() - which means closing the window inside that
// 650ms window would otherwise destroy the WebView (and the pending
// setTimeout with it) before the last edit ever reached disk. Routing every
// save through this shared promise lets the close handler below flush and
// await it instead of racing it.
function triggerSave(): Promise<void> {
  if (!currentSave) currentSave = save().finally(() => { currentSave = null; });
  return currentSave;
}
function markDirty(): void {
  if (!current) return;
  changeVersion += 1;
  saveState.classList.add("dirty");
  saveState.innerHTML = '<span class="save-dot"></span>Unsaved changes';
  scheduleSave();
}
function markSaved(): void {
  saveState.classList.remove("dirty");
  saveState.innerHTML = `<span class="save-dot"></span>${native ? "All changes saved" : "Preview saved in this browser"}`;
}
function renderKeys(parts: string[], trailingEllipsis = false): void {
  const keys = document.getElementById("shortcut-preview")!;
  keys.replaceChildren();
  for (const [index, part] of parts.entries()) {
    if (index) { const plus = document.createElement("span"); plus.textContent = "+"; keys.append(plus); }
    const key = document.createElement("kbd");
    key.textContent = part.replace(/^Key/, "").replace(/^Digit/, "").replace(/^./, c => c.toUpperCase());
    keys.append(key);
  }
  if (trailingEllipsis) { const more = document.createElement("span"); more.textContent = parts.length ? "+ …" : "…"; keys.append(more); }
}

function heldModifiers(event: KeyboardEvent): string[] {
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  return parts;
}

function updatePreview(): void {
  renderKeys(pendingHotkey.split("+"));
  const preview = document.querySelector<HTMLElement>(".appearance-preview")!;
  // The same mapping main.ts's applyTheme() uses for the overlay's dark
  // bars, so the preview can never show a different highlight than the
  // real overlay would for the same accent.
  const onBars = barAccent(accentColor.value);
  preview.style.setProperty("--bar-accent", onBars.fill);
  preview.style.setProperty("--on-bar-accent", onBars.ink);
  preview.style.setProperty("--preview-dim", spotlightDim.value);
  preview.style.setProperty("--preview-padding", `${Math.max(0, Math.min(60, Number(cardPadding.value)))}px`);
  preview.style.setProperty("--preview-radius", `${Math.max(0, Math.min(60, Number(cardRadius.value)))}px`);
}

let recordingHotkey = false;
let pendingHotkey = "";

function renderActionList(
  container: HTMLOListElement,
  entries: ActionEntry[],
  labels: Record<string, string>,
  locked: (id: string) => boolean = () => false,
): void {
  container.replaceChildren();
  entries.forEach((entry, index) => {
    const li = document.createElement("li");
    li.className = entry.enabled ? "action-row" : "action-row disabled";
    const isLocked = locked(entry.id);
    if (isLocked) li.classList.add("locked");

    const checkboxId = `${container.id}-${entry.id}`;
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.id = checkboxId;
    checkbox.checked = entry.enabled;
    // Still read back on save (readActionList), so a lapsed plan keeps the
    // user's order and on/off choice for when Pro returns.
    checkbox.disabled = isLocked;
    checkbox.addEventListener("change", () => {
      entry.enabled = checkbox.checked;
      markDirty();
      li.classList.toggle("disabled", !entry.enabled);
    });

    const label = document.createElement("label");
    label.htmlFor = checkboxId;
    if (isLocked) label.dataset.pro = "Pro";
    label.textContent = labels[entry.id] ?? entry.id;

    const reorder = document.createElement("div");
    reorder.className = "reorder";
    const up = document.createElement("button");
    up.type = "button";
    up.className = "icon";
    up.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M12 19V5m-6 6 6-6 6 6"/></svg>';
    up.setAttribute("aria-label", `Move ${labels[entry.id] ?? entry.id} up`);
    up.title = "Move up";
    up.disabled = index === 0;
    up.addEventListener("click", () => {
      [entries[index - 1], entries[index]] = [entries[index], entries[index - 1]];
      renderActionList(container, entries, labels, locked);
      markDirty();
    });
    const down = document.createElement("button");
    down.type = "button";
    down.className = "icon";
    down.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M12 5v14m-6-6 6 6 6-6"/></svg>';
    down.setAttribute("aria-label", `Move ${labels[entry.id] ?? entry.id} down`);
    down.title = "Move down";
    down.disabled = index === entries.length - 1;
    down.addEventListener("click", () => {
      [entries[index + 1], entries[index]] = [entries[index], entries[index + 1]];
      renderActionList(container, entries, labels, locked);
      markDirty();
    });
    reorder.append(up, down);

    const icon = actionIcon(ACTION_ICONS[entry.id] ?? "external");
    li.append(icon, label, reorder, checkbox);
    container.appendChild(li);
  });
}

// Plan comes only from Rust (get_entitlements); nothing here can grant it.
let paid = false;
let customActions: CustomAction[] = [];
const MAX_CUSTOM_ACTIONS = 24;
const isCustom = (id: string) => id.startsWith("custom-");
// Built-in (non-custom) actions that are part of Pro — locked in this list
// the same way a custom action is, rather than just being relabeled the
// way save-png is, since the button itself doesn't exist at all on Free
// (see actionsFrom/TEXT_ACTION_BUILDERS in main.ts).
const PRO_BUILT_IN_TEXT_ACTIONS = new Set(["redact"]);

function textLabels(): Record<string, string> {
  return { ...TEXT_ACTION_LABELS, ...Object.fromEntries(customActions.map((action) => [action.id, action.label])) };
}

function renderTextActions(entries: ActionEntry[]): void {
  renderActionList(textActionsList, entries, textLabels(), (id) => (isCustom(id) || PRO_BUILT_IN_TEXT_ACTIONS.has(id)) && !paid);
}

// Languages with an English-to-X model in Firefox Translations, shown by name.
const TRANSLATION_LANGUAGES = [
  "ar", "bg", "ca", "cs", "da", "de", "el", "en", "es", "et", "fa", "fi", "fr", "he", "hi", "hr",
  "hu", "id", "it", "ja", "ko", "lt", "lv", "nb", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sr",
  "sv", "th", "tr", "uk", "vi", "zh",
];

function fillOptions(select: HTMLSelectElement, options: Array<[string, string, boolean]>): void {
  select.replaceChildren(
    ...options.map(([value, label, pro]) => {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = pro ? `${label} (Pro)` : label;
      option.dataset.pro = String(pro);
      return option;
    }),
  );
}

const languageNames = new Intl.DisplayNames(["en"], { type: "language" });
fillOptions(
  translateLang,
  TRANSLATION_LANGUAGES
    .map((tag): [string, string, boolean] => [tag, languageNames.of(tag) ?? tag, false])
    .sort((a, b) => a[1].localeCompare(b[1])),
);
fillOptions(themePreset, Object.entries(THEME_PRESETS).map(([key, preset]) => [key, preset.label, preset.pro]));
fillOptions(lassoStyleSelect, Object.entries(LASSO_STYLES).map(([key, style]) => [key, style.label, style.pro]));

/** Locks Pro-only choices on the Free plan. Stored Pro values are kept (the
 * overlay falls back to the free default), so nothing is lost if Pro lapses. */
function applyPlan(): void {
  for (const select of [themePreset, lassoStyleSelect]) {
    for (const option of Array.from(select.options)) option.disabled = option.dataset.pro === "true" && !paid;
  }
  for (const control of [customLabel, customUrl, customAdd]) control.disabled = !paid;
  document.querySelector<HTMLElement>(".pro-note")!.hidden = paid;
  document.body.classList.toggle("is-pro", paid);
}

// ---------- Plan: trial, purchase, restore --------------------------------
let pollTimer: number | undefined;
let pollAttemptsLeft = 0;
let pollGeneration = 0;
let polling = false;
const POLL_INTERVAL_MS = 2000;
const MAX_POLL_ATTEMPTS = 150; // ~5 minutes, matching begin_purchase's own bounded-polling design

function stopPolling(): void {
  pollGeneration++;
  polling = false;
  if (pollTimer !== undefined) { window.clearTimeout(pollTimer); pollTimer = undefined; }
  planWaiting.hidden = true;
}

function showPlanError(message: string): void {
  planError.textContent = message;
  planError.hidden = false;
}

function clearPlanError(): void {
  planError.hidden = true;
}

/** Reflects license_status (richer than `paid`, which only distinguishes
 * Free/Pro — this also tells "never tried" apart from "trial ended"). */
function renderPlanSection(licenseStatus: LicenseStatus): void {
  planStartTrial.hidden = true;
  planBuy.hidden = true;
  planRestore.hidden = false;

  switch (licenseStatus.state) {
    case "noTrial":
      planHeading.textContent = "Try Onyx Lens Pro, free for 7 days.";
      planSubheading.textContent = "Custom actions and the visual pack unlock instantly. No card required to start.";
      planEmailHint.textContent = "Used to start your trial and to recover your purchase later.";
      planStartTrial.hidden = false;
      break;
    case "trialing": {
      const days = licenseStatus.daysLeft;
      planHeading.textContent = days > 0 ? `${days} day${days === 1 ? "" : "s"} left in your trial.` : "Your trial ends today.";
      planSubheading.textContent = "Buy now to keep custom actions and the visual pack after your trial ends — one time, no subscription.";
      planEmailHint.textContent = "The email your trial started with.";
      planBuy.hidden = false;
      break;
    }
    case "trialExpired":
      planHeading.textContent = "Your trial has ended.";
      planSubheading.textContent = "Buy Onyx Lens Pro once to keep custom actions and the visual pack — no subscription, yours for good.";
      planEmailHint.textContent = "The email your trial started with.";
      planBuy.hidden = false;
      break;
    case "needsRefresh":
      planHeading.textContent = "Reconnect to refresh your license.";
      planSubheading.textContent = "Onyx Lens couldn't confirm your Pro license recently. Connect to the internet, then use Restore purchase below.";
      planEmailHint.textContent = "The email you purchased Pro with.";
      break;
    case "licensed":
      planHeading.textContent = "You're on Onyx Lens Pro.";
      planSubheading.textContent = "Thanks for buying Onyx Lens Pro — it's yours for good.";
      planRestore.hidden = true;
      break;
  }
}

async function refreshPlanFromLicenseStatus(): Promise<void> {
  const licenseStatus = await invoke<LicenseStatus>("get_license_status");
  paid = licenseStatus.state === "trialing" || licenseStatus.state === "licensed";
  applyPlan();
  renderPlanSection(licenseStatus);
}

function requireEmail(): string | null {
  clearPlanError();
  if (!planEmail.value.trim() || !planEmail.checkValidity()) {
    planEmail.reportValidity();
    return null;
  }
  return planEmail.value.trim();
}

function startPolling(): void {
  stopPolling();
  if (closing || document.hidden || !document.getElementById("section-plan")?.classList.contains("active")) return;
  polling = true;
  const generation = pollGeneration;
  pollAttemptsLeft = MAX_POLL_ATTEMPTS;
  planWaiting.hidden = false;
  const poll = async () => {
    pollTimer = undefined;
    if (generation !== pollGeneration) return;
    pollAttemptsLeft -= 1;
    try {
      const entitlements = await invoke<Entitlements>("poll_purchase_status");
      if (generation !== pollGeneration) return;
      if (entitlements.plan !== "free") {
        stopPolling();
        await refreshPlanFromLicenseStatus();
        showStatus("Purchase complete — welcome to Pro.");
        return;
      }
    } catch {
      // Restore purchase remains available after a network failure.
    }
    if (generation !== pollGeneration) return;
    if (pollAttemptsLeft <= 0) stopPolling();
    else pollTimer = window.setTimeout(poll, POLL_INTERVAL_MS);
  };
  pollTimer = window.setTimeout(poll, POLL_INTERVAL_MS);
}

// A minimized/hidden settings page must not keep contacting checkout.
document.addEventListener("visibilitychange", () => {
  if (document.hidden && polling) stopPolling();
});
window.addEventListener("pagehide", stopPolling);

planStartTrial.addEventListener("click", () => {
  const email = requireEmail();
  if (!email) return;
  planStartTrial.disabled = true;
  invoke("start_trial", { email })
    .then(() => refreshPlanFromLicenseStatus())
    .catch((err) => showPlanError(typeof err === "string" ? err : "Could not start your trial. Check your connection and try again."))
    .finally(() => { planStartTrial.disabled = false; });
});

planBuy.addEventListener("click", () => {
  const email = requireEmail();
  if (!email) return;
  planBuy.disabled = true;
  invoke<string>("begin_purchase", { email })
    .then((checkoutUrl) => openUrl(checkoutUrl).then(startPolling))
    .catch((err) => showPlanError(typeof err === "string" ? err : "Could not start checkout. Check your connection and try again."))
    .finally(() => { planBuy.disabled = false; });
});

planRestore.addEventListener("click", () => {
  const email = requireEmail();
  if (!email) return;
  planRestore.disabled = true;
  invoke("restore_purchase", { email })
    .then(async () => {
      await refreshPlanFromLicenseStatus();
      showStatus(paid ? "Restored — welcome back to Pro." : "No trial or purchase found for that email yet.");
    })
    .catch((err) => showPlanError(typeof err === "string" ? err : "Could not check that email. Try again."))
    .finally(() => { planRestore.disabled = false; });
});

function renderCustomActions(): void {
  customList.replaceChildren(
    ...customActions.map((action) => {
      const item = document.createElement("li");
      const text = document.createElement("div");
      const name = document.createElement("strong");
      name.textContent = action.label;
      const url = document.createElement("span");
      url.textContent = action.urlTemplate;
      text.append(name, url);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "icon";
      remove.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M5 7h14M10 11v6m4-6v6M9 7l1-3h4l1 3M7 7l1 13h8l1-13"/></svg>';
      remove.setAttribute("aria-label", `Delete ${action.label}`);
      remove.title = "Delete";
      // Deleting your own data is always allowed, even on Free.
      remove.addEventListener("click", () => {
        customActions = customActions.filter((candidate) => candidate.id !== action.id);
        renderTextActions(readActionList(textActionsList).filter((entry) => entry.id !== action.id));
        renderCustomActions();
        markDirty();
      });
      item.append(text, remove);
      return item;
    }),
  );
}

customAdd.addEventListener("click", () => {
  const label = customLabel.value.trim();
  const urlTemplate = customUrl.value.trim();
  if (!label) {
    customLabel.focus();
    showStatus("Give the button a label.");
    return;
  }
  if (!isValidCustomUrl(urlTemplate) || !urlTemplate.includes("{q}")) {
    customUrl.focus();
    showStatus("Enter an http(s) link containing {q} where the selected text should go.");
    return;
  }
  if (customActions.length >= MAX_CUSTOM_ACTIONS) {
    showStatus(`You can have up to ${MAX_CUSTOM_ACTIONS} custom actions.`);
    return;
  }
  const action = { id: `custom-${crypto.randomUUID().slice(0, 8)}`, label, urlTemplate };
  customActions.push(action);
  renderTextActions([...readActionList(textActionsList), { id: action.id, enabled: true }]);
  renderCustomActions();
  customLabel.value = "";
  customUrl.value = "";
  markDirty();
  showStatus(`Added \u201c${label}\u201d to the text actions.`);
});

function populateForm(settings: Settings): void {
  current = settings;
  pendingHotkey = settings.hotkey;
  accentColor.value = settings.theme.accentColor;
  spotlightDim.value = String(settings.theme.spotlightDim);
  spotlightDimValue.textContent = settings.theme.spotlightDim.toFixed(2);
  cardPadding.value = String(settings.theme.cardPadding);
  cardRadius.value = String(settings.theme.cardRadius);
  aiProviderSelect.value = settings.aiProvider === "chat-gpt" ? "chatgpt" : settings.aiProvider;
  aiCustomUrl.value = settings.customAiUrlTemplate;
  aiCustomUrlRow.hidden = settings.aiProvider !== "custom";
  searchEngineSelect.value = settings.searchEngine;
  searchCustomUrl.value = settings.customSearchUrlTemplate;
  searchCustomUrlRow.hidden = settings.searchEngine !== "custom";
  autoCopy.checked = settings.behavior.autoCopyOnSelectRelease;
  hideCursor.checked = settings.behavior.hideCursor;
  translateLang.value = settings.behavior.translateTargetLang;
  launchAtStartup.checked = settings.launchAtStartup;
  themePreset.value = settings.theme.preset;
  lassoStyleSelect.value = settings.theme.lassoStyle;
  customActions = settings.customActions.map((action) => ({ ...action }));
  renderActionList(choiceActionsList, settings.choiceActions.map((a) => ({ ...a })), CHOICE_ACTION_LABELS);
  renderTextActions(settings.textActions.map((a) => ({ ...a })));
  renderCustomActions();
  updatePreview();
}

// The rendered <li> rows mutate their own local copies of the entries
// (via the closures above) rather than `current` directly, so reading the
// form back out at save time walks the DOM order instead of trusting
// `current` to have tracked every reorder.
function readActionList(container: HTMLOListElement): ActionEntry[] {
  return Array.from(container.querySelectorAll<HTMLInputElement>("input[type=checkbox]")).map((checkbox) => ({
    id: checkbox.id.slice(container.id.length + 1),
    enabled: checkbox.checked,
  }));
}

spotlightDim.addEventListener("input", () => {
  spotlightDimValue.textContent = Number(spotlightDim.value).toFixed(2);
});

aiProviderSelect.addEventListener("change", () => {
  aiCustomUrlRow.hidden = aiProviderSelect.value !== "custom";
});

searchEngineSelect.addEventListener("change", () => {
  searchCustomUrlRow.hidden = searchEngineSelect.value !== "custom";
});

const MODIFIER_CODES = new Set([
  "ControlLeft", "ControlRight", "AltLeft", "AltRight",
  "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight",
]);

let capturedFlashTimer: number | undefined;

function startRecordingHotkey(): void {
  if (capturedFlashTimer !== undefined) { window.clearTimeout(capturedFlashTimer); capturedFlashTimer = undefined; }
  recordingHotkey = true;
  shortcutCard.classList.remove("captured");
  shortcutCard.classList.add("recording");
  shortcutBadgeText.textContent = "Recording";
  shortcutHint.textContent = "Press your new shortcut… (Esc to cancel)";
}

function stopRecordingHotkey(captured: boolean): void {
  recordingHotkey = false;
  if (!captured) updatePreview();
  shortcutCard.classList.remove("recording");
  shortcutBadgeText.textContent = DEFAULT_SHORTCUT_BADGE;
  if (captured) {
    // A brief, unmistakable "it worked" moment — otherwise the card just
    // silently drops back to its idle look and a fast typist could miss
    // that the new combo actually registered.
    shortcutCard.classList.add("captured");
    shortcutHint.textContent = "Shortcut updated ✓";
    capturedFlashTimer = window.setTimeout(() => {
      shortcutCard.classList.remove("captured");
      shortcutHint.textContent = DEFAULT_SHORTCUT_HINT;
      capturedFlashTimer = undefined;
    }, 1400);
  } else {
    shortcutHint.textContent = DEFAULT_SHORTCUT_HINT;
  }
}

shortcutCard.addEventListener("click", () => { if (!recordingHotkey) shortcutCard.focus(); });
shortcutCard.addEventListener("focus", startRecordingHotkey);
shortcutCard.addEventListener("blur", () => {
  if (recordingHotkey) stopRecordingHotkey(false);
});
// Windows treats a lone Alt press-and-release as "activate the window menu",
// which pulls focus off the card and silently ends the recording — so the
// recording swallows Alt's keyup too, and listens in the capture phase.
shortcutCard.addEventListener("keyup", (event) => {
  if (!recordingHotkey) return;
  event.preventDefault();
  event.stopPropagation();
  const held = heldModifiers(event);
  if (held.length) renderKeys(held, true);
  else updatePreview();
}, true);
shortcutCard.addEventListener("keydown", (event) => {
  if (!recordingHotkey) return;
  if (event.code === "Tab") { stopRecordingHotkey(false); return; }
  event.preventDefault();
  event.stopPropagation();
  if (event.code === "Escape") { stopRecordingHotkey(false); updatePreview(); return; }
  if (MODIFIER_CODES.has(event.code)) {
    // Show what is held so far, so it is obvious Ctrl/Alt were noticed and
    // that the combo is waiting for its last key.
    renderKeys(heldModifiers(event), true);
    return;
  }

  // `KeyboardEvent.code` (e.g. "KeyA", "Digit1", "Enter", "ArrowUp", "F5")
  // already matches the naming tauri-plugin-global-shortcut expects — the
  // existing hardcoded default "shift+enter" round-trips through this same
  // convention, just lowercased.
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  parts.push(event.code);
  pendingHotkey = parts.join("+");
  stopRecordingHotkey(true);
  updatePreview();
  markDirty();
  // The shortcut is the one setting people test straight away — don't make
  // them wait out the autosave delay for it to start working.
  if (saveTimer !== undefined) window.clearTimeout(saveTimer);
  void triggerSave();
}, true);

function showStatus(text: string): void {
  status.textContent = text;
  status.classList.add("visible");
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => status.classList.remove("visible"), 2400);
}
let toastTimer: number | undefined;

function isValidCustomUrl(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return (url.protocol === "https:" || url.protocol === "http:") && Boolean(url.hostname);
  } catch {
    return false;
  }
}

async function save(): Promise<void> {
  if (!current || saveInFlight || !saveState.classList.contains("dirty")) return;
  const invalid = Array.from(document.querySelectorAll<HTMLInputElement>("input")).find(input => !input.validity.valid);
  // Autosave only — never steals focus or switches sections while the user
  // may still be mid-edit elsewhere. An invalid field just leaves the footer
  // on "Unsaved changes" until it's fixed, instead of forcing a jump.
  if (invalid) return;
  if (aiProviderSelect.value === "custom" && !isValidCustomUrl(aiCustomUrl.value)) return;
  if (searchEngineSelect.value === "custom" && (!isValidCustomUrl(searchCustomUrl.value) || !searchCustomUrl.value.includes("{q}"))) return;
  const startedAtVersion = changeVersion;
  const next: Settings = {
    ...current,
    hotkey: pendingHotkey,
    theme: {
      accentColor: accentColor.value,
      spotlightDim: Number(spotlightDim.value),
      cardPadding: Number(cardPadding.value),
      cardRadius: Number(cardRadius.value),
      preset: themePreset.value,
      lassoStyle: lassoStyleSelect.value,
    },
    behavior: {
      autoCopyOnSelectRelease: autoCopy.checked,
      hideCursor: hideCursor.checked,
      translateTargetLang: translateLang.value || "en",
    },
    aiProvider: aiProviderSelect.value as AiProvider,
    customAiUrlTemplate: aiCustomUrl.value.trim(),
    searchEngine: searchEngineSelect.value as SearchEngine,
    customSearchUrlTemplate: searchCustomUrl.value.trim(),
    choiceActions: readActionList(choiceActionsList),
    textActions: readActionList(textActionsList),
    customActions,
    launchAtStartup: launchAtStartup.checked,
  };
  saveInFlight = true;
  saveState.innerHTML = '<span class="save-dot"></span>Saving…';
  try {
    if (native) await invoke("set_settings", { settings: next });
    else localStorage.setItem(PREVIEW_STORAGE_KEY, JSON.stringify(next));
    current = next;
    if (changeVersion === startedAtVersion) markSaved();
    showStatus(native ? "Saved" : "Preview saved in this browser");
  } catch (err) {
    console.error("failed to save settings", err);
    saveState.innerHTML = '<span class="save-dot"></span>Could not save';
    showStatus(typeof err === "string" ? err : "Could not save. Check the shortcut and try again.");
  } finally {
    saveInFlight = false;
    if (changeVersion !== startedAtVersion) scheduleSave();
  }
}

// Reset uses Rust defaults in the desktop app and saves them automatically.
resetButton.addEventListener("click", () => {
  void (native ? invoke<Settings>("default_settings") : Promise.resolve(structuredClone(previewDefaults))).then((defaults) => {
    populateForm(defaults);
    markDirty();
    showStatus("Restoring defaults…");
  }).catch(() => showStatus("Could not load defaults. Please try again."));
});

async function load(): Promise<void> {
  const [settings, entitlements, licenseStatus] = await Promise.all([
    invoke<Settings>("get_settings"),
    invoke<Entitlements>("get_entitlements"),
    invoke<LicenseStatus>("get_license_status"),
  ]);
  paid = entitlements.plan !== "free";
  applyPlan();
  renderPlanSection(licenseStatus);
  populateForm(settings);
  markSaved();
  resetButton.disabled = false;
}

if (native) {
  void load().catch(() => { showStatus("Could not load settings. Close and reopen this window to retry."); saveState.textContent = "Settings unavailable"; });
} else {
  // Browser preview only (never the desktop app, whose plan comes from Rust):
  // `?plan=pro` previews the paid layout for design review and UI tests.
  // There's no backend to preview trial/purchase against here, so the Plan
  // section just explains that rather than pretending to work.
  paid = new URLSearchParams(location.search).get("plan") === "pro";
  populateForm(previewSettings());
  markSaved();
  resetButton.disabled = false;
  applyPlan();
  planHeading.textContent = paid ? "Previewing the Pro plan." : "Previewing the Free plan.";
  planSubheading.textContent = "Trials and purchases only work in the installed desktop app.";
  planEmail.disabled = true;
  planStartTrial.hidden = true;
  planBuy.hidden = true;
  planRestore.hidden = true;
  document.querySelector<HTMLElement>(".workspace-tag")!.textContent = "Browser preview · saves here only";
}
for (const control of document.querySelectorAll("input:not([data-draft]), select")) {
  control.addEventListener("input", () => { markDirty(); updatePreview(); });
  control.addEventListener("change", () => { markDirty(); updatePreview(); });
}

// ---------- Sidebar navigation ------------------------------------------
// One section visible at a time; all of them stay mounted in the DOM (just
// display:none), so switching never loses in-progress edits in another tab.
const navItems = Array.from(document.querySelectorAll<HTMLButtonElement>(".nav-item"));
const sections = Array.from(document.querySelectorAll<HTMLElement>(".settings-section"));

function showSection(name: string): void {
  // The purchase-polling interval is owned by the Plan section being
  // visible — leaving it (same as closing the window) stops it rather than
  // letting it keep running against a screen the user isn't looking at.
  if (name !== "plan") stopPolling();
  for (const item of navItems) {
    item.classList.toggle("active", item.dataset.section === name);
    item.setAttribute("aria-current", item.dataset.section === name ? "page" : "false");
  }
  document.getElementById("content")!.scrollTop = 0;
  for (const section of sections) section.classList.toggle("active", section.id === `section-${name}`);
}

// Opened straight to a section when Rust has a reason to (the trial-expired
// paywall, see lib.rs's show_settings_window) — a URL hash if this is a
// fresh window, or this event if the window already existed and just got
// refocused instead of rebuilt.
if (location.hash === "#plan") showSection("plan");
if (native) void listen<string>("navigate-section", (event) => showSection(event.payload));

for (const item of navItems) {
  item.addEventListener("click", () => {
    const name = item.dataset.section;
    if (name) showSection(name);
  });
}

// ---------- Custom titlebar ----------------------------------------------
// Frameless window (see lib.rs) — these replace the native minimize/
// maximize/close buttons the OS would otherwise draw in a bright chrome
// that clashed with the rest of the app.
const appWindow = native ? getCurrentWindow() : null;
const minimizeButton = document.getElementById("win-minimize") as HTMLButtonElement;
const maximizeButton = document.getElementById("win-maximize") as HTMLButtonElement;
const closeButton = document.getElementById("win-close") as HTMLButtonElement;

function windowAction(action: (() => Promise<void>) | undefined): void {
  void action?.().catch((error) => {
    console.error("window control failed", error);
    showStatus("Could not change this window. Please try again.");
  });
}
minimizeButton.addEventListener("click", () => windowAction(appWindow ? () => appWindow.minimize() : undefined));
maximizeButton.addEventListener("click", () => windowAction(appWindow ? () => appWindow.toggleMaximize() : undefined));
closeButton.addEventListener("click", () => windowAction(appWindow ? () => appWindow.close() : undefined));

// Closing destroys this window's WebView outright (it's recreated fresh next
// time, unlike the overlay's hide-and-reuse), which would silently drop the
// last edit if it landed inside the 650ms autosave debounce. Every close
// path (this button, Alt+F4, the taskbar) fires the same close-requested
// event, so intercepting it here covers all of them.
if (appWindow) {
  const windowToClose = appWindow;
  void windowToClose.onCloseRequested((event) => {
    if (closing) { event.preventDefault(); return; }
    stopPolling();
    if (!currentSave && !saveState.classList.contains("dirty")) return;
    event.preventDefault();
    closing = true;
    closeButton.disabled = true;
    void (async () => {
      if (saveTimer !== undefined) { window.clearTimeout(saveTimer); saveTimer = undefined; }
      // Await the active save, then save a newer edit once. Unchanged invalid
      // input / a failed write cannot improve through five immediate retries.
      if (currentSave) await currentSave;
      if (saveState.classList.contains("dirty")) {
        await triggerSave();
      }
      // destroy(), not close(): close() re-emits this same close-requested
      // event, so if a field is left invalid (out-of-range number input,
      // say) save() silently no-ops and "dirty" never clears - calling
      // close() again here would re-enter this handler forever and the
      // window would never actually close. destroy() forces it unconditionally
      // once we've made a bounded, best-effort attempt to save first.
      await windowToClose.destroy();
    })().catch((error) => {
      closing = false;
      closeButton.disabled = false;
      console.error("could not close settings", error);
      showStatus("Could not close Settings. Please try again.");
    });
  }).catch((error) => console.error("could not register settings close handler", error));
}

// A double-click anywhere on the drag region should maximize/restore, same
// as any normal titlebar — data-tauri-drag-region alone doesn't give you
// that for free.
document.getElementById("titlebar")?.addEventListener("dblclick", (event) => {
  if (event.target instanceof HTMLElement && event.target.closest(".titlebar-btn")) return;
  windowAction(appWindow ? () => appWindow.toggleMaximize() : undefined);
});

if (!native) { for (const button of [minimizeButton, maximizeButton, closeButton]) button.hidden = true; }
