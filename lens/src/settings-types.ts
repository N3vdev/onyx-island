// The TypeScript mirror of src-tauri/src/settings.rs's `Settings` struct.
// Imported by both main.ts (the overlay) and settings.ts (the settings
// window) instead of each declaring its own copy — every review pass on
// this feature independently flagged the two hand-duplicated interfaces as
// the single biggest drift risk in the diff: a field added/renamed on the
// Rust side has no compiler signal telling you to update both TS copies,
// since two independently-declared structurally-identical interfaces don't
// disagree as far as tsc is concerned. This file is types only (erased at
// build time), so importing it from both of Vite's separate entry points
// (overlay/settings) costs nothing at runtime.

export interface ActionEntry {
  id: string;
  enabled: boolean;
}

export interface ThemeSettings {
  accentColor: string;
  spotlightDim: number;
  cardPadding: number;
  cardRadius: number;
  preset: string;
  lassoStyle: string;
}

export interface CustomAction {
  id: string;
  label: string;
  urlTemplate: string;
}

export interface BehaviorSettings {
  autoCopyOnSelectRelease: boolean;
  translateTargetLang: string;
  hideCursor: boolean;
}

// "chat-gpt" is accepted for backward compatibility with older Rust builds
// that serialized this value differently (see settings.rs's AiProvider);
// never written back out by either window.
export type AiProvider = "chatgpt" | "chat-gpt" | "claude" | "gemini" | "custom";
export type SearchEngine = "google" | "bing" | "duck-duck-go" | "perplexity" | "custom";

export interface Settings {
  version: number;
  hotkey: string;
  theme: ThemeSettings;
  behavior: BehaviorSettings;
  aiProvider: AiProvider;
  customAiUrlTemplate: string;
  searchEngine: SearchEngine;
  customSearchUrlTemplate: string;
  choiceActions: ActionEntry[];
  textActions: ActionEntry[];
  customActions: CustomAction[];
  launchAtStartup: boolean;
}

export interface Entitlements {
  plan: string;
}

// Mirrors entitlements.rs's `LicenseStatus` (`#[serde(tag = "state")]`) —
// richer than `Entitlements` (which only says "is this Pro"), since the
// Plan section needs to tell "never started a trial" apart from "trial
// expired" even though both currently gate the same as Free.
export type LicenseStatus =
  | { state: "noTrial" }
  | { state: "trialing"; daysLeft: number }
  | { state: "trialExpired" }
  | { state: "licensed" }
  | { state: "needsRefresh" };
