import { PreviewPlayer } from './preview-player.js?v=3';
import { TRACKS } from './tracks.js?v=1';

﻿// Onyx site. The island at the top of the page is a working music player at
// the app's real size: hover to open it, press play and the song plays (through
// direct official audio previews, loaded on interaction).

const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const hasGsap = typeof window.gsap !== 'undefined' && typeof window.ScrollTrigger !== 'undefined';
const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];

if (hasGsap) gsap.registerPlugin(ScrollTrigger);

// The intro plays from the top; a restored mid-page scroll would skip it.
if ('scrollRestoration' in history) history.scrollRestoration = 'manual';
if (!location.hash) window.scrollTo(0, 0);

/* ---------- Smooth scroll ---------- */
let lenis = null;
if (!reduce && hasGsap && typeof window.Lenis !== 'undefined') {
  lenis = new Lenis({ lerp: 0.09, wheelMultiplier: 1 });
  lenis.on('scroll', ScrollTrigger.update);
  gsap.ticker.add((t) => lenis.raf(t * 1000));
  gsap.ticker.lagSmoothing(0);
  $$('a[href^="#"]').forEach((a) =>
    a.addEventListener('click', (e) => {
      const target = $(a.getAttribute('href'));
      if (!target) return;
      e.preventDefault();
      lenis.scrollTo(target, { duration: 1.4, easing: (x) => 1 - Math.pow(1 - x, 4) });
    })
  );
}

/* ---------- Clock (the idle card further down) ---------- */
function tickClock() {
  const parts = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).formatToParts(new Date());
  const time = parts.filter((p) => p.type !== 'dayPeriod').map((p) => p.value).join('').trim();
  const period = parts.find((p) => p.type === 'dayPeriod');
  $$('[data-clock-mini]').forEach((el) => (el.textContent = time + (period ? ' ' + period.value : '')));
}
tickClock();
setInterval(tickClock, 15000);
$$('[data-year]').forEach((el) => (el.textContent = new Date().getFullYear()));

/* ---------- Island: native logical coordinates, plus its 1px edge ---------- */
const island = $('.island');
const body = $('.island__body');
const ISLAND_TOP = 7;
const SIZES = {
  music: { w: 222, h: 38, r: 18 },
  condensed: { w: 110, h: 34, r: 16 },
  player: { w: 498, h: 122, r: 38 },
};
let current = null;
let condensed = false;
let closeTimer = 0;
let openTimer = 0;
let condenseTimer = 0;
const geometry = { ...SIZES.music };
const smooth = (a, b, value) => {
  const t = Math.max(0, Math.min(1, (value - a) / (b - a)));
  return t * t * (3 - 2 * t);
};

// Same cubic handles as islet-render::corner_control/create_squircle.
function squircle(x, y, width, height, radius) {
  const r = Math.min(radius, width / 2, height / 2);
  const k = 0.48 + (0.5522848 - 0.48) * smooth(0.85, 1, r / (Math.min(width, height) / 2));
  const c = r * k;
  const n = (v) => Number(v.toFixed(3));
  return `M${n(x + r)} ${n(y)}H${n(x + width - r)}C${n(x + width - r + c)} ${n(y)} ${n(x + width)} ${n(y + r - c)} ${n(x + width)} ${n(y + r)}V${n(y + height - r)}C${n(x + width)} ${n(y + height - r + c)} ${n(x + width - r + c)} ${n(y + height)} ${n(x + width - r)} ${n(y + height)}H${n(x + r)}C${n(x + r - c)} ${n(y + height)} ${n(x)} ${n(y + height - r + c)} ${n(x)} ${n(y + height - r)}V${n(y + r)}C${n(x)} ${n(y + r - c)} ${n(x + r - c)} ${n(y)} ${n(x + r)} ${n(y)}Z`;
}

function fitScale(w) {
  return Math.min(1, Math.max(0.1, (window.innerWidth - 24) / w));
}

function paintIsland() {
  const { w, h, r } = geometry;
  Object.assign(body.style, { width: w + 'px', height: h + 'px', borderRadius: (r + 1) + 'px', clipPath: `path('${squircle(0, 0, w, h, r + 1)}')` });
  $('[data-outline]').setAttribute('d', squircle(1, 1, w - 2, h - 2, r));
  const smallH = condensed ? 32 : 36;
  const smallW = condensed ? 108 : 220;
  const opening = current !== 'player' && h <= 38 ? 0 : Math.max(0, Math.min(1, (h - 2 - smallH) / (120 - smallH)));
  const small = $('.panel--music');
  const big = $('.panel--player');
  const compactW = h <= 38 ? w - 2 : smallW;
  const compactH = h <= 38 ? h - 2 : smallH;
  const smallAlpha = 1 - smooth(0, 0.35, opening);
  const bigAlpha = smooth(0.5, 0.95, opening);
  Object.assign(small.style, { width: compactW + 'px', height: compactH + 'px', opacity: smallAlpha, visibility: smallAlpha > 0 ? 'visible' : 'hidden', filter: reduce ? 'none' : `blur(${opening * 3}px)` });
  Object.assign(big.style, { opacity: bigAlpha, visibility: bigAlpha > 0 ? 'visible' : 'hidden', filter: reduce ? 'none' : `blur(${(1 - opening) * 3}px)` });
  small.style.pointerEvents = current === 'player' ? 'none' : '';
  big.inert = current !== 'player';
  const artSize = compactH - 8 + (112 - (smallH - 8)) * opening;
  const artRadius = (smallH - 8) / 2 + (34 - (smallH - 8) / 2) * opening;
  const art = $('[data-art]');
  Object.assign(art.style, { width: artSize + 'px', height: artSize + 'px', borderRadius: artRadius + 'px', clipPath: `path('${squircle(0, 0, artSize, artSize, artRadius)}')` });
  const shrunk = 1 - smooth(110, 200, compactW);
  const start = (compactW - 56) * (1 - shrunk) + (compactW - 14 - 34.8) * shrunk;
  Object.assign($('.bars').style, { left: start + 'px', height: (18 - shrunk * 3) + 'px' });
  $('.music__text').style.opacity = smooth(150, 200, compactW);
}

function setIsland(state, instant = false) {
  if (state === current && !instant) return;
  current = state;
  island.dataset.state = state;
  const size = SIZES[state];
  const scale = fitScale(size.w);
  if (!hasGsap || reduce || instant) {
    Object.assign(geometry, size);
    paintIsland();
    if (hasGsap) gsap.set(island, { scale });
    else island.style.transform = `translateX(-50%) scale(${scale})`;
    return;
  }
  gsap.to(geometry, { ...size, duration: 0.55, ease: 'back.out(1.15)', overwrite: true, onUpdate: paintIsland });
  gsap.to(island, { scale, duration: 0.55, ease: 'expo.out', overwrite: true });
}

if (hasGsap) {
  island.style.transform = 'none';
  gsap.set(island, { xPercent: -50, x: 0, y: 0 });
}

function openIsland() {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  audioPlayer.prepare();
  setIsland('player');
}
function closeIsland(delay = 400) {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => {
    if (seeking || adjustingVolume) return;
    setIsland(condensed ? 'condensed' : 'music');
  }, delay);
}
island.addEventListener('pointerenter', (e) => {
  if (e.pointerType !== 'mouse') return;
  clearTimeout(closeTimer);
  audioPlayer.prepare();
  openTimer = setTimeout(openIsland, 250);
});
island.addEventListener('pointerleave', (e) => { if (e.pointerType === 'mouse') closeIsland(); });
island.addEventListener('focusin', openIsland);
island.addEventListener('focusout', (e) => { if (!island.contains(e.relatedTarget)) closeIsland(0); });
island.addEventListener('click', (e) => { if (current !== 'player') { e.preventDefault(); openIsland(); } });
island.addEventListener('keydown', (e) => {
  if ((e.key === 'Enter' || e.key === ' ') && e.target === body) {
    e.preventDefault(); openIsland(); $('[data-toggle]').focus();
  } else if (e.key === 'Escape') {
    body.focus({ preventScroll: true });
    setIsland(condensed ? 'condensed' : 'music');
  }
});
document.addEventListener('pointerdown', (e) => {
  if (e.pointerType !== 'mouse' && current === 'player' && !island.contains(e.target)) closeIsland(0);
});

/* ---------- Direct, official music previews ---------- */
let shownTrack = -1;
let wasPlaying = false;
let seeking = false;
let adjustingVolume = false;
let seekPosition = 0;
const seekBar = $('[data-seek]');
const volume = $('[data-volume]');
let lastElapsed = '', lastRemaining = '', lastSpokenTime = '', lastValue = '', lastDuration = '';
const clock = (seconds) => {
  const s = Math.max(0, Math.floor(seconds));
  return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
};

function renderProgress(position, duration) {
  if (seeking) position = seekPosition;
  const progress = duration ? Math.max(0, Math.min(1, position / duration)) : 0;
  $('.player__fill').style.transform = `scaleX(${progress})`;
  seekBar.style.setProperty('--progress', progress);
  const elapsed = clock(position);
  const remaining = '-' + clock(Math.max(0, duration - position));
  const value = String(Number(position.toFixed(1)));
  const spokenTime = `${elapsed} of ${clock(duration)}`;
  if (lastElapsed !== elapsed) $('[data-elapsed]').textContent = lastElapsed = elapsed;
  if (lastRemaining !== remaining) $('[data-remaining]').textContent = lastRemaining = remaining;
  if (lastDuration !== String(duration)) seekBar.setAttribute('aria-valuemax', lastDuration = String(duration));
  if (lastValue !== value) seekBar.setAttribute('aria-valuenow', lastValue = value);
  if (lastSpokenTime !== spokenTime) seekBar.setAttribute('aria-valuetext', lastSpokenTime = spokenTime);
}

function renderPlayer(state) {
  const changed = shownTrack !== state.index;
  if (changed) {
    shownTrack = state.index;
    clearTimeout(condenseTimer);
    condensed = false;
    wasPlaying = false;
    seeking = false;
    const t = state.track;
    island.dataset.track = t.id;
    $$('[data-title]', island).forEach((el) => (el.textContent = t.title));
    $$('[data-artist]', island).forEach((el) => (el.textContent = t.artist));
    $('[data-art]').style.backgroundImage = `url("${t.art}")`;
    $$('[data-song-link]', island).forEach((el) => { el.href = 'https://open.spotify.com/track/' + t.id; });
    $('[data-track-link]').href = t.sourceUrl;
    $('[data-provider-link]').href = t.sourceUrl;
    island.style.setProperty('--bar', t.accent);
    if (current === 'condensed') setIsland('music');
    if (hasGsap && !reduce) gsap.fromTo($$('[data-song]', island), { opacity: 0.5 }, { opacity: 1, duration: 0.18, overwrite: true });
  }
  island.classList.toggle('is-playing', state.playing);
  island.classList.toggle('is-loading', state.loading);
  $('[data-toggle]').setAttribute('aria-label', state.loading ? 'Cancel loading preview' : state.playing ? 'Pause preview' : 'Play preview');
  $('[data-toggle]').setAttribute('aria-pressed', String(state.playing));
  $('[data-mute]').setAttribute('aria-label', state.muted ? 'Unmute preview' : 'Mute preview');
  $('[data-mute]').setAttribute('aria-pressed', String(state.muted));
  island.classList.toggle('is-muted', state.muted);
  volume.value = state.volume;
  volume.style.setProperty('--volume', state.volume);
  const error = $('[data-player-status]');
  error.textContent = state.error;
  error.hidden = !state.error;
  if (state.playing && !wasPlaying) {
    clearTimeout(condenseTimer);
    const index = state.index;
    condenseTimer = setTimeout(() => {
      if (audioPlayer.index !== index || !audioPlayer.playing) return;
      condensed = true;
      if (current === 'music') setIsland('condensed');
    }, 5000);
  } else if (!state.playing && !state.loading) {
    clearTimeout(condenseTimer);
    condensed = false;
    if (current === 'condensed') setIsland('music');
  }
  wasPlaying = state.playing;
  renderProgress(state.position, state.duration);
}

const audioPlayer = new PreviewPlayer(TRACKS, {
  createAudio: () => {
    const audio = document.createElement('audio');
    $('[data-preview-audio]').append(audio);
    return audio;
  },
  onState: renderPlayer,
  onTime: renderProgress,
});
renderPlayer(audioPlayer.getState());

$('[data-toggle]').addEventListener('click', (e) => { e.stopPropagation(); audioPlayer.toggle(); });
$('[data-next]').addEventListener('click', (e) => { e.stopPropagation(); audioPlayer.next(); });
$('[data-prev]').addEventListener('click', (e) => { e.stopPropagation(); audioPlayer.previous(); });
$('[data-mute]').addEventListener('click', (e) => { e.stopPropagation(); audioPlayer.toggleMute(); });
volume.addEventListener('input', (e) => { audioPlayer.setVolume(Number(e.target.value)); });
volume.addEventListener('pointerdown', () => { adjustingVolume = true; });
function closeAfterDrag(e) {
  if (e.pointerType !== 'mouse') return;
  const r = island.getBoundingClientRect();
  if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) closeIsland();
}
volume.addEventListener('pointerup', (e) => { adjustingVolume = false; closeAfterDrag(e); });
volume.addEventListener('pointercancel', (e) => { adjustingVolume = false; closeAfterDrag(e); });

function previewSeek(e) {
  const rect = seekBar.getBoundingClientRect();
  seekPosition = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)) * audioPlayer.duration;
  renderProgress(seekPosition, audioPlayer.duration);
}
seekBar.addEventListener('pointerdown', (e) => {
  e.stopPropagation(); e.preventDefault();
  seekBar.focus({ preventScroll: true });
  seeking = true;
  seekBar.setPointerCapture(e.pointerId);
  previewSeek(e);
});
seekBar.addEventListener('pointermove', (e) => { if (seeking) previewSeek(e); });
seekBar.addEventListener('pointerup', (e) => {
  if (!seeking) return;
  previewSeek(e);
  seeking = false;
  audioPlayer.seek(seekPosition);
  closeAfterDrag(e);
});
seekBar.addEventListener('pointercancel', () => { seeking = false; renderProgress(audioPlayer.position, audioPlayer.duration); });
seekBar.addEventListener('click', (e) => e.stopPropagation());
seekBar.addEventListener('keydown', (e) => {
  const offset = { ArrowLeft: -5, ArrowRight: 5, ArrowDown: -1, ArrowUp: 1 }[e.key];
  if (offset === undefined && e.key !== 'Home' && e.key !== 'End') return;
  e.preventDefault();
  audioPlayer.seek(e.key === 'Home' ? 0 : e.key === 'End' ? audioPlayer.duration : audioPlayer.position + offset);
});
document.addEventListener('visibilitychange', () => audioPlayer.setVisible(!document.hidden));
window.addEventListener('pagehide', () => audioPlayer.pause());

/* ---------- Stats specimen ---------- */
const samples = { cpu: Array.from({ length: 24 }, () => 30 + Math.random() * 20), gpu: Array.from({ length: 24 }, () => 10 + Math.random() * 10) };
function sparkPath(values) {
  const step = 100 / (values.length - 1);
  return values.map((v, i) => (i ? 'L' : 'M') + (i * step).toFixed(1) + ' ' + (28 - (v / 100) * 26).toFixed(1)).join(' ');
}
function tickStats() {
  for (const key of ['cpu', 'gpu']) {
    const list = samples[key];
    const last = list[list.length - 1];
    const range = key === 'cpu' ? [8, 96] : [4, 70];
    list.push(Math.max(range[0], Math.min(range[1], last + (Math.random() - 0.45) * 18)));
    list.shift();
    const path = $(`[data-spark="${key}"]`);
    if (path) path.setAttribute('d', sparkPath(list));
    const label = $(`[data-${key}]`);
    if (label) label.textContent = Math.round(list[list.length - 1]) + '%';
  }
  const net = $('[data-net]');
  if (net) net.firstChild.textContent = (1 + Math.random() * 4).toFixed(1);
}
tickStats();
setInterval(tickStats, 1000);

/* ---------- Intro ---------- */
function intro() {
  const introEl = $('.intro');
  const pill = $('.intro__pill');
  setIsland('music', true);
  if (!hasGsap || reduce) {
    introEl.remove();
    return;
  }
  gsap.set(island, { autoAlpha: 0 });
  gsap.set('.hero .line .word', { yPercent: 110 });
  gsap.set(['.hero__kicker', '.hero__foot', '.nav', '.hero__wordmark', '.hero__hint'], { autoAlpha: 0, y: 20 });

  // While the page loads: a small pill that breathes in the middle.
  gsap.to(pill, { width: 128, height: 36, duration: 0.9, ease: 'back.out(1.6)', delay: 0.2 });
  const breathe = gsap.to(pill, { scale: 1.06, duration: 0.9, ease: 'sine.inOut', yoyo: true, repeat: -1, delay: 1.1 });

  // Loaded: fonts, the page and the first song's cover, or 4s at most.
  const cover = new Promise((resolve) => {
    const img = new Image();
    img.onload = img.onerror = resolve;
    img.src = TRACKS[0].art;
  });
  const page = document.readyState === 'complete'
    ? Promise.resolve()
    : new Promise((resolve) => window.addEventListener('load', resolve, { once: true }));
  const minimum = new Promise((resolve) => setTimeout(resolve, 1300));
  const cap = new Promise((resolve) => setTimeout(resolve, 4000));
  Promise.race([Promise.all([document.fonts.ready, page, cover, minimum]), cap]).then(reveal);

  // Then it rises to the top and stretches into the island's real size.
  function reveal() {
    breathe.kill();
    const dy = -(window.innerHeight / 2) + ISLAND_TOP + SIZES.music.h / 2;
    const tl = gsap.timeline({ defaults: { ease: 'expo.out' } });
    tl.to(pill, { scale: 1, duration: 0.25, ease: 'power2.out' })
      .to(pill, { y: dy, duration: 0.9, ease: 'expo.inOut' })
      .to(pill, { width: SIZES.music.w, height: SIZES.music.h, duration: 0.7, ease: 'back.out(1.4)' }, '-=0.35')
      .to(introEl, { backgroundColor: 'rgba(0,0,0,0)', duration: 0.6 }, '<0.1')
      .set(island, { autoAlpha: 1 })
      .set(introEl, { display: 'none' })
      .to('.hero .line .word', { yPercent: 0, duration: 1.2, stagger: 0.08 }, '-=0.2')
      .to(['.hero__kicker', '.nav'], { autoAlpha: 1, y: 0, duration: 0.9, stagger: 0.05 }, '<0.1')
      .to(['.hero__foot', '.hero__hint'], { autoAlpha: 1, y: 0, duration: 1 }, '<0.3')
      .to('.hero__wordmark', { autoAlpha: 1, y: 0, duration: 1.6 }, '<0.1');
  }
}

/* ---------- Scroll story ---------- */
function story() {
  if (!hasGsap) return;

  $$('.chapter').forEach((chapter, i) => {
    chapter.setAttribute('data-index', String(i + 1).padStart(2, '0'));
    if (chapter.hasAttribute('data-dock')) {
      ScrollTrigger.create({
        trigger: chapter, start: 'top 55%', end: 'bottom 45%',
        onToggle: (self) => dock(self.isActive),
      });
    }
    if (!reduce) {
      gsap.from($$(':scope > :not(.dock-window)', chapter), {
        y: 40, autoAlpha: 0, duration: 1, ease: 'expo.out', stagger: 0.07,
        scrollTrigger: { trigger: chapter, start: 'top 70%' },
      });
    }
  });

  // Manifesto: words light up as you read down the paragraph.
  const text = $('[data-words]');
  if (text) {
    const words = text.textContent.trim().split(/\s+/);
    text.textContent = '';
    words.forEach((w, i) => {
      const span = document.createElement('span');
      span.className = 'w';
      span.textContent = w;
      text.append(span, i < words.length - 1 ? ' ' : '');
    });
    const spans = $$('.w', text);
    if (!reduce) {
      ScrollTrigger.create({
        trigger: text, start: 'top 75%', end: 'bottom 45%', scrub: true,
        onUpdate: (self) => {
          const lit = Math.round(self.progress * spans.length);
          spans.forEach((w, i) => (w.style.color = i < lit ? 'var(--t1)' : ''));
        },
      });
    }
  }

  if (reduce) return;

  gsap.to('.hero__wordmark', { yPercent: 18, opacity: 0.35, ease: 'none',
    scrollTrigger: { trigger: '.hero', start: 'top top', end: 'bottom top', scrub: true } });

  const reel = $('.reel__track');
  gsap.fromTo(reel, { x: () => window.innerWidth * 0.15 },
    { x: () => -(reel.scrollWidth - window.innerWidth * 0.85), ease: 'none',
      scrollTrigger: { trigger: '.reel', start: 'top bottom', end: 'bottom top', scrub: 0.6, invalidateOnRefresh: true } });

  gsap.fromTo('.settings__frame img', { rotateX: 22, y: 80, scale: 0.92, opacity: 0.4 },
    { rotateX: 0, y: 0, scale: 1, opacity: 1, ease: 'none',
      scrollTrigger: { trigger: '.settings', start: 'top bottom', end: 'center 60%', scrub: 0.8 } });

  gsap.from('.cell', { y: 60, autoAlpha: 0, duration: 1.1, ease: 'expo.out', stagger: 0.08,
    scrollTrigger: { trigger: '.bento', start: 'top 75%' } });

  gsap.from('.get__title .word', { yPercent: 110, duration: 1.3, ease: 'expo.out', stagger: 0.1,
    scrollTrigger: { trigger: '.get', start: 'top 65%' } });

  gsap.from('.footer__mark', { yPercent: 40, ease: 'none',
    scrollTrigger: { trigger: '.footer', start: 'top bottom', end: 'bottom bottom', scrub: true } });
}

/* Dynamic placement: a window takes the top of the screen and the island
   glides to the bottom edge; when it leaves, the island comes home. */
const dockWindow = $('.dock-window');
document.body.appendChild(dockWindow);
function dock(on) {
  const bottom = window.innerHeight - SIZES.music.h - 12;
  if (on) closeIsland(0);
  if (!hasGsap || reduce) {
    island.style.top = on ? bottom + 'px' : '';
    dockWindow.style.opacity = on ? 1 : 0;
    dockWindow.style.visibility = on ? 'visible' : 'hidden';
    return;
  }
  gsap.to(dockWindow, { autoAlpha: on ? 1 : 0, y: on ? 0 : -30, duration: 0.6, ease: 'expo.out' });
  gsap.to('.nav', { autoAlpha: on ? 0 : 1, duration: 0.3 });
  gsap.to(island, { top: on ? bottom : ISLAND_TOP, duration: 1.1, ease: 'expo.inOut', delay: on ? 0.15 : 0 });
}

/* ---------- Settings tabs ---------- */
const shot = $('[data-settings]');
$$('.tabs button').forEach((btn) =>
  btn.addEventListener('click', () => {
    $$('.tabs button').forEach((b) => b.setAttribute('aria-selected', String(b === btn)));
    const src = `/assets/shots/settings-${btn.dataset.shot}.webp`;
    const label = btn.textContent.trim();
    if (!hasGsap || reduce) {
      shot.src = src;
      shot.alt = `Onyx Settings, ${label} page`;
      return;
    }
    gsap.to(shot, { opacity: 0, y: 10, duration: 0.18, ease: 'power2.in', onComplete: () => {
      shot.src = src;
      shot.alt = `Onyx Settings, ${label} page`;
      gsap.to(shot, { opacity: 1, y: 0, duration: 0.5, ease: 'expo.out' });
    } });
  })
);
['media', 'dictation', 'widgets'].forEach((p) => { const i = new Image(); i.src = `/assets/shots/settings-${p}.webp`; });

/* ---------- Pointer details (fine pointers only, so nothing sticks on touch) ---------- */
if (window.matchMedia('(hover: hover) and (pointer: fine)').matches && !reduce) {
  const mark = $('.hero__wordmark span');
  window.addEventListener('pointermove', (e) => {
    if (!mark) return;
    const r = mark.getBoundingClientRect();
    mark.style.setProperty('--sx', ((e.clientX - r.left) / r.width) * 100 + '%');
    mark.style.setProperty('--sy', ((e.clientY - r.top) / r.height) * 100 + '%');
  }, { passive: true });

  $$('.cell').forEach((cell) =>
    cell.addEventListener('pointermove', (e) => {
      const r = cell.getBoundingClientRect();
      cell.style.setProperty('--cx', e.clientX - r.left + 'px');
      cell.style.setProperty('--cy', e.clientY - r.top + 'px');
    })
  );

  $$('.btn--primary').forEach((btn) => {
    btn.addEventListener('pointermove', (e) => {
      const r = btn.getBoundingClientRect();
      btn.style.setProperty('--mx', e.clientX - r.left + 'px');
      btn.style.setProperty('--my', e.clientY - r.top + 'px');
      if (btn.classList.contains('magnetic') && hasGsap) {
        gsap.to(btn, { x: (e.clientX - r.left - r.width / 2) * 0.2, y: (e.clientY - r.top - r.height / 2) * 0.3, duration: 0.5, ease: 'expo.out' });
      }
    });
    btn.addEventListener('pointerleave', () => hasGsap && gsap.to(btn, { x: 0, y: 0, duration: 0.8, ease: 'elastic.out(1, 0.4)' }));
  });
}

window.addEventListener('resize', () => {
  if (!current) return;
  const s = fitScale(SIZES[current].w);
  if (hasGsap) gsap.set(island, { scale: s });
  else island.style.transform = `translateX(-50%) scale(${s})`;
});

intro();
story();
