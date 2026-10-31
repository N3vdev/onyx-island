import test from 'node:test';
import assert from 'node:assert/strict';
import { PreviewPlayer } from '../public/preview-player.js';

class AudioFake extends EventTarget {
  constructor() {
    super();
    this.paused = true;
    this.readyState = 0;
    this.duration = NaN;
    this.currentTime = 0;
    this.error = null;
    this.playCalls = 0;
    this.loadCalls = 0;
    this.playResult = () => Promise.resolve();
  }
  set src(value) {
    this._src = this.currentSrc = value;
    this.currentTime = 0;
    this.duration = NaN;
    this.readyState = 0;
    this.error = null;
  }
  get src() { return this._src; }
  load() { ++this.loadCalls; }
  play() { ++this.playCalls; this.paused = false; return this.playResult(); }
  pause() { this.paused = true; this.fire('pause'); }
  fire(name) { this.dispatchEvent(new Event(name)); }
  metadata(duration = 90) {
    this.duration = duration;
    this.readyState = 4;
    this.fire('loadedmetadata');
  }
  start() { this.paused = false; this.fire('playing'); }
  removeAttribute(name) { if (name === 'src') this.src = ''; }
  remove() {}
}

const tracks = ['pink', 'strangers', 'nights'].map((id) => ({ id, preview: `https://audio.example/${id}.m4a` }));
function setup() {
  const audios = [], frames = new Map(), timers = new Map(), states = [], times = [];
  let id = 0;
  const player = new PreviewPlayer(tracks, {
    createAudio: () => { const audio = new AudioFake(); audios.push(audio); return audio; },
    onState: (state) => states.push(state),
    onTime: (...time) => times.push(time),
    requestFrame: (callback) => { frames.set(++id, callback); return id; },
    cancelFrame: (key) => frames.delete(key),
    setTimer: (callback, delay) => { timers.set(++id, { callback, delay }); return id; },
    clearTimer: (key) => timers.delete(key),
  });
  return { player, audio: audios[0], audios, frames, timers, states, times };
}

test('elapsed time comes from the audio; 90-second source is limited to 30 seconds', async () => {
  const { player, audio, times } = setup();
  await player.play(); audio.metadata(); audio.start();
  audio.currentTime = 12.4; audio.fire('timeupdate');
  assert.equal(player.getState().position, 12.4);
  assert.equal(player.duration, 30);
  assert.deepEqual(times.at(-1), [12.4, 30]);
  audio.currentTime = 30; audio.fire('timeupdate');
  assert.equal(player.index, 1);
  assert.equal(audio.src, tracks[1].preview);
  assert.equal(player.position, 0);
  audio.fire('ended');
  assert.equal(player.index, 1, 'queued old ended event must not skip another song');
});

test('next songs reuse the gesture-authorized audio element and preserve pause intent', async () => {
  const { player, audio } = setup();
  player.next();
  assert.equal(player.index, 1);
  assert.equal(audio.playCalls, 0);
  await player.play(); audio.metadata(); audio.start();
  await player.next();
  assert.equal(player.audio, audio);
  assert.equal(player.index, 2);
  assert.equal(audio.playCalls, 2);
  player.pause(); player.next();
  assert.equal(player.index, 0);
  assert.equal(audio.playCalls, 2);
});

test('a late rejected play from a skipped song cannot overwrite the current song', async () => {
  const { player, audio } = setup();
  let reject;
  audio.playResult = () => new Promise((_, fail) => { reject = fail; });
  const oldRequest = player.play();
  audio.playResult = () => Promise.resolve();
  await player.next(); audio.metadata(); audio.start();
  reject(new Error('old source aborted'));
  await oldRequest;
  assert.equal(player.getState().playing, true);
  assert.equal(player.getState().error, '');
  assert.equal(player.index, 1);
});

test('cancelled loading stays paused even when its old play promise resolves', async () => {
  const { player, audio, frames, timers } = setup();
  let resolve;
  audio.playResult = () => new Promise((done) => { resolve = done; });
  const pending = player.play();
  player.pause(); resolve(); await pending;
  assert.equal(audio.paused, true);
  assert.equal(player.getState().loading, false);
  assert.equal(frames.size, 0);
  assert.equal(timers.size, 0);
});

test('preloading is lazy and warms only the upcoming song', async () => {
  const { player, audio, audios } = setup();
  assert.equal(audios.length, 1);
  assert.equal(audio.preload, 'none');
  assert.equal(audio.loadCalls, 0);
  player.prepare(); player.prepare();
  assert.equal(audio.loadCalls, 1);
  await player.play(); audio.metadata(); audio.start();
  assert.equal(audios.length, 2);
  assert.equal(audios[1].src, tracks[1].preview);
  audio.fire('playing');
  assert.equal(audios.length, 2);
  assert.equal(audios[1].playCalls, 0);
});

test('early seeking waits for metadata, clamps to clip bounds and restarts with Previous', async () => {
  const { player, audio } = setup();
  player.seek(55);
  assert.equal(player.position, 30);
  audio.metadata();
  assert.equal(audio.currentTime, 30);
  player.previous();
  assert.equal(player.index, 0);
  assert.equal(audio.currentTime, 0);
  player.seek(-5);
  assert.equal(audio.currentTime, 0);
  player.seek(NaN);
  assert.equal(audio.currentTime, 0);
});

test('network errors clear loading and retry remains possible', async () => {
  const { player, audio, frames } = setup();
  audio.playResult = () => Promise.reject(new Error('unavailable'));
  await player.play();
  assert.equal(player.wantPlay, false);
  assert.equal(player.getState().loading, false);
  assert.match(player.error, /retry/);
  assert.equal(frames.size, 0);
  audio.playResult = () => Promise.resolve();
  await player.play(); audio.metadata(); audio.start();
  assert.equal(player.error, '');
  assert.equal(player.playing, true);
});

test('volume, mute and restored volume affect the actual audio', () => {
  const { player, audio } = setup();
  player.setVolume(0.25);
  assert.equal(audio.volume, 0.25);
  player.toggleMute(); assert.equal(audio.muted, true);
  player.toggleMute(); assert.equal(audio.muted, false);
  player.setVolume(0); player.toggleMute();
  assert.equal(player.volume, 0.25);
  assert.equal(audio.volume, 0.25);
  player.toggleMute();
  player.setVolume(0);
  player.toggleMute();
  assert.equal(player.volume, 0.25);
  assert.equal(audio.volume, 0.25);
  assert.equal(audio.muted, false);
  player.setVolume(9); assert.equal(player.volume, 1);
  player.setVolume(NaN); assert.equal(player.volume, 1);
});

test('hidden pages stop painting while retaining audio boundary protection; pause parks all work', async () => {
  const { player, audio, frames, timers } = setup();
  await player.play(); audio.metadata(); audio.start();
  assert.equal(frames.size, 1);
  player.setVisible(false);
  assert.equal(frames.size, 0);
  assert.equal(audio.paused, false);
  assert.ok([...timers.values()].some(({ delay }) => delay <= 30000));
  audio.currentTime = 30; audio.fire('timeupdate');
  assert.equal(player.index, 1);
  player.setVisible(true); audio.metadata(); audio.start();
  assert.equal(frames.size, 1);
  player.pause();
  assert.equal(frames.size, 0);
  assert.equal(timers.size, 0);
});
