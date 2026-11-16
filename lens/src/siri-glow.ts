/** Continuous Siri-style perimeter light. The soft border mask is cached;
 * only a small color field changes each frame. No work is scheduled when idle.
 */
export interface SiriGlow {
  resize: () => void;
  start: () => void;
  stop: () => void;
  release: () => void;
}

// Clockwise from the top, with matching endpoints for a seamless color field.
const COLORS = [
  "#a889f5", "#7574ee", "#5488f2", "#59c4dd", "#75d6ae",
  "#d7be91", "#ef977d", "#df8dc1", "#a889f5",
] as const;
const OPEN_MS = 620;
const FIELD_SIZE = 256;

function context2d(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const context = canvas.getContext("2d", { alpha: true });
  if (!context) throw new Error("Siri glow requires a 2D canvas");
  return context;
}

export function initSiriGlow(canvas: HTMLCanvasElement): SiriGlow {
  const ctx = context2d(canvas);
  const mask = document.createElement("canvas");
  const maskCtx = context2d(mask);
  const field = document.createElement("canvas");
  field.width = field.height = FIELD_SIZE;
  const fieldCtx = context2d(field);
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  let active = false;
  let frame = 0;
  let started = 0;
  let width = 0;
  let height = 0;
  let cachedWidth = 0;
  let cachedHeight = 0;

  function resize(): void {
    const nextWidth = Math.max(1, window.innerWidth);
    const nextHeight = Math.max(1, window.innerHeight);
    if (nextWidth === width && nextHeight === height) return;
    width = nextWidth;
    height = nextHeight;
    // The light is intentionally diffuse; a capped backing store avoids
    // allocating a monitor-sized texture on high-DPI / multi-monitor setups.
    const scale = Math.min(1, 1280 / width, 800 / height);
    canvas.width = Math.max(1, Math.round(width * scale));
    canvas.height = Math.max(1, Math.round(height * scale));
    if (active) {
      prepareMask();
      paint(performance.now());
    }
  }

  function prepareMask(): void {
    if (cachedWidth === width && cachedHeight === height) return;
    const w = canvas.width;
    const h = canvas.height;
    mask.width = w;
    mask.height = h;
    const pixels = maskCtx.createImageData(w, h);
    const sx = w / width;
    const sy = h / height;
    const inset = 2;
    const radius = Math.min(26, width / 4, height / 4);
    const near = Math.min(24, Math.max(14, Math.min(width, height) * 0.022));
    const far = near * 2.15;
    const reach = Math.ceil(far * 3);
    const halfWidth = width / 2 - inset;
    const halfHeight = height / 2 - inset;
    const edgeX = Math.ceil(reach * sx);
    const edgeY = Math.ceil(reach * sy);
    // Sample the falloff once at quarter-pixel precision. Mask preparation
    // then needs only distance arithmetic, even on the first hotkey press.
    const falloff = new Uint8Array(Math.ceil((reach + radius + 8) * 4) + 1);
    for (let i = 0; i < falloff.length; i++) {
      const distance = i / 4 - reach;
      const inside = Math.max(0, -distance);
      const outside = Math.exp(-Math.pow(Math.max(0, distance) / 2.2, 2));
      const bloom = 0.45 * Math.exp(-Math.pow(inside / near, 1.25))
        + 0.13 * Math.exp(-Math.pow(inside / far, 2));
      falloff[i] = Math.round(255 * bloom * outside);
    }
    const xDistances = new Float32Array(w);
    for (let px = 0; px < w; px++) {
      xDistances[px] = Math.abs((px + 0.5) / sx - width / 2) - halfWidth + radius;
    }

    // Signed distance to ONE rounded rectangle produces equal falloff on
    // every edge and through the corners, without overlapping strips or blobs.
    for (let py = 0; py < h; py++) {
      const y = (py + 0.5) / sy;
      const centralRow = py > edgeY && py < h - edgeY;
      const qy = Math.abs(y - height / 2) - halfHeight + radius;
      for (let px = 0; px < w; px++) {
        if (centralRow && px > edgeX && px < w - edgeX) {
          px = w - edgeX - 1;
          continue;
        }
        const qx = xDistances[px];
        const distance = (qx > 0 && qy > 0 ? Math.hypot(qx, qy) : Math.max(qx, qy)) - radius;
        const alpha = falloff[Math.round((distance + reach) * 4)] ?? 0;
        const index = (py * w + px) * 4;
        pixels.data[index] = pixels.data[index + 1] = pixels.data[index + 2] = 255;
        pixels.data[index + 3] = alpha;
      }
    }
    maskCtx.putImageData(pixels, 0, 0);
    cachedWidth = width;
    cachedHeight = height;
  }

  function paint(now: number): void {
    const elapsed = Math.max(0, now - started);
    const t = reducedMotion.matches ? 0 : elapsed / 1000;
    const progress = reducedMotion.matches ? 1 : Math.min(1, elapsed / OPEN_MS);
    const enter = 1 - Math.pow(1 - progress, 3);
    const breath = 0.96 + 0.04 * Math.sin(t * 0.8);
    const gradient = fieldCtx.createConicGradient(
      -Math.PI / 2 + Math.sin(t * 0.26) * 0.11,
      FIELD_SIZE / 2,
      FIELD_SIZE / 2,
    );
    for (let i = 0; i < COLORS.length; i++) {
      const position = i / (COLORS.length - 1);
      // Small independent shifts mimic the source mesh's slow color drift.
      // Endpoints stay fixed and neighboring stops can never cross.
      const drift = i === 0 || i === COLORS.length - 1
        ? 0 : Math.sin(t * 0.48 + i * 1.7) * 0.026;
      gradient.addColorStop(position + drift, COLORS[i]);
    }
    fieldCtx.fillStyle = gradient;
    fieldCtx.fillRect(0, 0, FIELD_SIZE, FIELD_SIZE);

    // Stretching a square color field keeps the colors distributed over all
    // four borders on both portrait and landscape screens.
    ctx.globalCompositeOperation = "copy";
    ctx.globalAlpha = enter * breath;
    ctx.drawImage(field, 0, 0, canvas.width, canvas.height);
    ctx.globalCompositeOperation = "destination-in";
    ctx.globalAlpha = 1;
    ctx.drawImage(mask, 0, 0);
    ctx.globalCompositeOperation = "source-over";
  }

  function tick(now: number): void {
    frame = 0;
    if (!active || document.hidden || reducedMotion.matches) return;
    paint(now);
    frame = requestAnimationFrame(tick);
  }

  function syncMotion(): void {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    if (!active || document.hidden) return;
    paint(performance.now());
    if (!reducedMotion.matches) frame = requestAnimationFrame(tick);
  }

  function stop(): void {
    active = false;
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    document.removeEventListener("visibilitychange", syncMotion);
    reducedMotion.removeEventListener("change", syncMotion);
    canvas.classList.remove("glow-active");
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  }

  function start(): void {
    stop();
    resize();
    prepareMask();
    active = true;
    started = performance.now();
    canvas.classList.add("glow-active");
    document.addEventListener("visibilitychange", syncMotion);
    reducedMotion.addEventListener("change", syncMotion);
    syncMotion();
  }

  // Build the reusable mask on first activation, never at idle startup.
  function release(): void {
    stop();
    // Clearing pixels alone keeps both large backing stores allocated.
    canvas.width = canvas.height = 1;
    mask.width = mask.height = 1;
    width = height = cachedWidth = cachedHeight = 0;
  }

  return { resize, start, stop, release };
}
