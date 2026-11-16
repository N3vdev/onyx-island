// Pin to screen: this page is shared by every pinned window (see
// vite.config.ts) — each instance is told which pin it is only by its own
// Tauri window label ("pin-<uuid>"), which get_pin_image reads server-side.
// There is no per-pin state here at all.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const image = document.getElementById("pin-image") as HTMLImageElement;
const closeButton = document.getElementById("pin-close") as HTMLButtonElement;
const resizeGrip = document.getElementById("pin-resize-grip") as HTMLDivElement;
const appWindow = getCurrentWindow();

void invoke<string>("get_pin_image")
  .then((dataUrl) => { image.src = dataUrl; })
  .catch((err) => {
    console.error("could not load pinned image", err);
    // Nothing useful to show without the image — closing beats leaving a
    // blank floating window the user can't tell is broken versus loading.
    void appWindow.close();
  });

// pin.rs frees this pin's bytes when the window is destroyed, however that
// happens, so closing is all this needs to do.
closeButton.addEventListener("click", () => void appWindow.close());

resizeGrip.addEventListener("pointerdown", (event) => {
  event.preventDefault();
  void appWindow.startResizeDragging("SouthEast");
});
