// Onyx site. The island at the top of the page is a working music player at
// the app's real size: hover to open it, press play and the song plays (through
// Spotify's own player, loaded the first time the island is touched).

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

/* ---------- Island ---------- */
const island = $('.island');
const body = $('.island__body');
const ISLAND_TOP = 7;
// The app's sizes at 100% display scale.
const SIZES = {
  music:  { w: 222, h: 38,  r: 19 },
  player: { w: 498, h: 122, r: 38 },
};
let current = null;

function fitScale(w) {
  return Math.min(1, (window.innerWidth - 24) / w);
}

function setIsland(state, instant = false) {
  if (state === current) return;
  const size = SIZES[state];
  const prev = current && $('.panel--' + current, island);
  const next = $('.panel--' + state, island);
  current = state;
  island.dataset.state = state;
  const scale = fitScale(size.w);

  if (!hasGsap || reduce || instant) {
    Object.assign(body.style, { width: size.w + 'px', height: size.h + 'px', borderRadius: size.r + 'px' });
    if (hasGsap) gsap.set(island, { scale });
    else island.style.transform = `translateX(-50%) scale(${scale})`;
    $$('.panel', island).forEach((p) => {
      p.classList.toggle('is-on', p === next);
      p.style.opacity = '';
    });
    return;
  }

  gsap.to(body, { width: size.w, height: size.h, borderRadius: size.r, duration: 0.6, ease: 'back.out(1.3)' });
  gsap.to(island, { scale, duration: 0.6, ease: 'expo.out' });
  if (prev) {
    gsap.to(prev, { opacity: 0, filter: 'blur(5px)', scale: 0.97, duration: 0.14, ease: 'power2.in',
      onComplete: () => prev.classList.remove('is-on') });
  }
  next.classList.add('is-on');
  gsap.fromTo(next, { opacity: 0, filter: 'blur(6px)', scale: 1.03 },
    { opacity: 1, filter: 'blur(0px)', scale: 1, duration: 0.42, delay: 0.12, ease: 'power3.out' });
}

// GSAP owns the transform from here on: clear the CSS centring first, so it is
// not read back as a pixel offset and applied twice.
if (hasGsap) {
  island.style.transform = 'none';
  gsap.set(island, { xPercent: -50, x: 0, y: 0 });
}

/* Hover opens the player and leaving folds it back, as in the app. Touch:
   tap to open, tap anywhere else to close. Keyboard: focus opens it. */
let closeTimer = 0;
function openIsland() {
  clearTimeout(closeTimer);
  setIsland('player');
  loadSpotify();
}
function closeIsland(delay = 260) {
  clearTimeout(closeTimer);
  closeTimer = setTimeout(() => setIsland('music'), delay);
}
island.addEventListener('pointerenter', (e) => { if (e.pointerType === 'mouse') openIsland(); });
island.addEventListener('pointerleave', (e) => { if (e.pointerType === 'mouse') closeIsland(); });
island.addEventListener('focusin', openIsland);
island.addEventListener('focusout', (e) => { if (!island.contains(e.relatedTarget)) closeIsland(0); });
island.addEventListener('click', () => { if (current === 'music') openIsland(); });
document.addEventListener('pointerdown', (e) => {
  if (e.pointerType !== 'mouse' && current === 'player' && !island.contains(e.target)) closeIsland(0);
});

/* ---------- Music ---------- */
const TRACKS = [
  { id: '3xKsf9qdS1CyvXSMEid6g8', title: 'Pink + White', artist: 'Frank Ocean', duration: 184, accent: '#d99577',
    art: 'https://i.scdn.co/image/ab67616d0000b273c5649add07ed3720be9d5526' },
  { id: '1au2UduxcvHfa0fZS3Szci', title: 'Lavender Sunflower', artist: 'Tory Lanez', duration: 151, accent: '#d97f2d',
    art: 'https://i.scdn.co/image/ab67616d0000b2730c5f23cbf0b1ab7e37d0dc67' },
  { id: '593knnQN4REEshx8th9zgK', title: 'Strangers', artist: 'Kenya Grace', duration: 172, accent: '#e0857a',
    art: 'https://i.scdn.co/image/ab67616d0000b27324f7e27b268c2777fc367e36' },
  { id: '3taCbWWTilb7eNMsAzOBq4', title: 'Radio', artist: 'Lana Del Rey', duration: 214, accent: '#d9934e',
    art: 'https://i.scdn.co/image/ab67616d0000b273ebc8cfac8b586bc475b04918' },
  { id: '7eqoqGkKwgOaWNNHx90uEZ', title: 'Nights', artist: 'Frank Ocean', duration: 307, accent: '#d99577',
    art: 'https://i.scdn.co/image/ab67616d0000b273c5649add07ed3720be9d5526' },
];
let track = 0;
// Each song plays a 30-second clip, then the next one starts. Signed in to
// Spotify, that is the song's first 30 seconds; otherwise Spotify only sends
// its own 30-second preview, and it chooses which part of the song that is.
const CLIP_MS = 30000;

const player = {
  api: 'idle',          // idle | loading | ready
  controller: null,
  playing: false,
  wantPlay: false,      // play as soon as the current song is loaded
  started: false,       // the current song has been started at least once
  position: 0,          // ms
  duration: CLIP_MS,
  nearEnd: false,
  clipDone: false,      // this song's 30 seconds are up and the next is coming
};

const clock = (ms) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
};

function renderProgress() {
  const { position, duration } = player;
  $('[data-elapsed]').textContent = clock(position);
  $('[data-remaining]').textContent = '-' + clock(duration - position);
  const pct = duration ? Math.min(100, (position / duration) * 100) : 0;
  $('.player__fill').style.width = pct + '%';
  $('[data-seek]').setAttribute('aria-valuenow', String(Math.round(pct)));
}

function renderPlaying() {
  island.classList.toggle('is-playing', player.playing);
  island.classList.toggle('is-loading', player.wantPlay && !player.playing);
  $('[data-toggle]').setAttribute('aria-label', player.playing ? 'Pause' : 'Play');
}

function applyTrack(index) {
  const t = TRACKS[index];
  $$('[data-title]', island).forEach((el) => (el.textContent = t.title));
  $$('[data-artist]', island).forEach((el) => (el.textContent = t.artist));
  $$('[data-art]', island).forEach((el) => (el.style.backgroundImage = `url("${t.art}")`));
  $('[data-track-link]', island).href = 'https://open.spotify.com/track/' + t.id;
  island.style.setProperty('--bar', t.accent);
  player.position = 0;
  player.duration = Math.min(t.duration * 1000, CLIP_MS);
  player.started = false;
  player.nearEnd = false;
  player.clipDone = false;
  renderProgress();
}

function changeTrack(step) {
  track = (track + step + TRACKS.length) % TRACKS.length;
  // The new song starts from nothing now, not when its title fades in.
  Object.assign(player, { position: 0, started: false, nearEnd: false, clipDone: false });
  renderProgress();
  const song = $$('[data-song], [data-art]', island);
  if (!hasGsap || reduce) {
    applyTrack(track);
  } else {
    gsap.to(song, { opacity: 0, y: -6, filter: 'blur(4px)', duration: 0.2, ease: 'power2.in', onComplete: () => {
      applyTrack(track);
      gsap.fromTo(song, { y: 6 }, { opacity: 1, y: 0, filter: 'blur(0px)', duration: 0.45, ease: 'expo.out', stagger: 0.03 });
    } });
  }
  if (player.controller) {
    // Loading a song stops the old one without reporting it, so the island
    // stops believing it plays until Spotify says the new one does.
    player.wantPlay = player.wantPlay || player.playing;
    player.playing = false;
    player.controller.loadUri('spotify:track:' + TRACKS[track].id);
    const uri = TRACKS[track].id;
    [900, 2200, 4000].forEach((ms) => setTimeout(() => {
      if (TRACKS[track].id === uri && player.wantPlay && !player.playing) startPlayback();
    }, ms));
  }
  renderPlaying();
}

function startPlayback() {
  if (!player.controller) return;
  player.started = true;
  player.controller.play();
}

function togglePlay() {
  if (player.api !== 'ready') {
    player.wantPlay = !player.wantPlay;
    loadSpotify();
    renderPlaying();
    return;
  }
  if (player.playing) {
    player.wantPlay = false;
    player.controller.pause();
  } else if (player.wantPlay) {
    // Still starting: a second press cancels it.
    player.wantPlay = false;
  } else {
    player.wantPlay = true;
    if (player.started) player.controller.resume();
    else startPlayback();
  }
  renderPlaying();
}

function onPlayback(e) {
  const d = e.data || {};
  // Updates about the previous song can still arrive after a switch.
  if (d.playingURI && d.playingURI !== 'spotify:track:' + TRACKS[track].id) return;
  const wasPlaying = player.playing;
  player.playing = !d.isPaused;
  if (d.duration) player.duration = Math.min(d.duration, CLIP_MS);
  if (typeof d.position === 'number') player.position = Math.min(d.position, player.duration);
  if (player.playing) player.wantPlay = false;
  renderProgress();
  renderPlaying();
  // At the end of the clip (a full song's first 30 seconds, or the end of
  // Spotify's preview, which keeps reporting "playing"), the next song starts.
  if (player.playing && !player.clipDone && player.duration && d.position >= player.duration - 400) {
    player.clipDone = true;
    player.wantPlay = true;
    changeTrack(1);
    return;
  }
  // A song that played to its end moves on to the next one, still playing.
  if (player.playing && player.duration && player.position >= player.duration - 1500) player.nearEnd = true;
  if (wasPlaying && !player.playing && player.nearEnd) {
    player.wantPlay = true;
    changeTrack(1);
  }
}

function loadSpotify() {
  if (player.api !== 'idle') return;
  player.api = 'loading';
  window.onSpotifyIframeApiReady = (IFrameAPI) => {
    IFrameAPI.createController($('#spotify-embed'),
      { uri: 'spotify:track:' + TRACKS[track].id, width: 300, height: 80 },
      (controller) => {
        player.controller = controller;
        player.api = 'ready';
        controller.addListener('playback_update', onPlayback);
        // 'ready' can fire before this listener exists, so a pending play is
        // also tried once the player has had a moment to load.
        const start = () => { if (player.wantPlay && !player.playing) startPlayback(); };
        controller.addListener('ready', start);
        setTimeout(start, 900);
      });
  };
  const script = document.createElement('script');
  script.src = 'https://open.spotify.com/embed/iframe-api/v1';
  script.async = true;
  script.onerror = () => {
    player.api = 'idle';
    player.wantPlay = false;
    renderPlaying();
  };
  document.body.appendChild(script);
}

$('[data-toggle]').addEventListener('click', (e) => { e.stopPropagation(); togglePlay(); });
$('[data-next]').addEventListener('click', (e) => { e.stopPropagation(); changeTrack(1); });
$('[data-prev]').addEventListener('click', (e) => {
  e.stopPropagation();
  // Like any player: back restarts the song unless it has only just begun.
  if (player.controller && player.position > 3000) player.controller.seek(0);
  else changeTrack(-1);
});
const seekBar = $('[data-seek]');
function seekTo(fraction) {
  const ms = Math.max(0, Math.min(1, fraction)) * player.duration;
  player.position = ms;
  renderProgress();
  if (player.controller) player.controller.seek(ms / 1000);
}
seekBar.addEventListener('click', (e) => {
  e.stopPropagation();
  const r = seekBar.getBoundingClientRect();
  seekTo((e.clientX - r.left) / r.width);
});
seekBar.addEventListener('keydown', (e) => {
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  e.preventDefault();
  seekTo((player.position + (e.key === 'ArrowRight' ? 5000 : -5000)) / player.duration);
});

applyTrack(track);
renderPlaying();
TRACKS.slice(1).forEach((t) => { const i = new Image(); i.src = t.art; });

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
});

intro();
story();
