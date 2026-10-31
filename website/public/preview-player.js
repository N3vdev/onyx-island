// One active media element retains the browser's user-gesture permission.
// A separate, silent element warms only the next clip. Audio stays at its source.
export const PREVIEW_SECONDS = 30;

const clamp = (value, min, max) => Math.max(min, Math.min(max, value));

export class PreviewPlayer {
  constructor(tracks, {
    createAudio = () => new Audio(),
    onState = () => {},
    onTime = () => {},
    requestFrame = (callback) => requestAnimationFrame(callback),
    cancelFrame = (id) => cancelAnimationFrame(id),
    setTimer = (callback, delay) => setTimeout(callback, delay),
    clearTimer = (id) => clearTimeout(id),
  } = {}) {
    if (!tracks.length) throw new Error('A preview playlist is required.');
    Object.assign(this, { tracks, createAudio, onState, onTime, requestFrame, cancelFrame, setTimer, clearTimer });
    this.index = 0;
    this.volume = 0.3;
    this.lastVolume = this.volume;
    this.muted = false;
    this.wantPlay = false;
    this.playing = false;
    this.loading = false;
    this.error = '';
    this.visible = true;
    this.attempt = 0;
    this.frame = 0;
    this.boundaryTimer = 0;
    this.loadingTimer = 0;
    this.preloader = null;
    this.prepared = false;
    this.pendingSeek = null;
    this.attackAt = 0;
    this.audio = this.createAudio();
    this.audio.crossOrigin = 'anonymous';
    this.audio.preload = 'none';
    this.audio.src = this.tracks[0].preview;
    this.audio.volume = this.volume;
    this._bind();
  }

  get duration() {
    const duration = this.audio.duration;
    return Number.isFinite(duration) && duration > 0 ? Math.min(duration, PREVIEW_SECONDS) : PREVIEW_SECONDS;
  }

  get position() {
    return clamp(this.pendingSeek ?? this.audio.currentTime ?? 0, 0, this.duration);
  }

  getState() {
    return {
      index: this.index, track: this.tracks[this.index],
      playing: this.playing, loading: this.loading, error: this.error,
      volume: this.volume, muted: this.muted || this.volume === 0,
      position: this.position, duration: this.duration,
    };
  }

  prepare() {
    if (this.prepared) return;
    this.prepared = true;
    this.audio.preload = 'auto';
    this.audio.load();
  }

  play() {
    const attempt = ++this.attempt;
    if (this.error) {
      this.prepared = false;
      this.error = '';
    }
    this.prepare();
    if (this.position >= this.duration - 0.02) this.seek(0);
    this.wantPlay = true;
    this.loading = !this.playing;
    this.attackAt = this.audio.currentTime || 0;
    this.audio.volume = 0;
    this._watchLoading();
    this._emit();
    // Called directly from the Play interaction, without awaiting any API.
    try {
      return Promise.resolve(this.audio.play()).then(() => {
        if (!this.wantPlay) this.audio.pause();
      }).catch((error) => {
        if (attempt !== this.attempt || !this.wantPlay) return;
        this._fail(error?.name === 'NotAllowedError'
          ? 'Press Play to start this preview.'
          : 'This preview could not load. Press Play to retry.');
      });
    } catch {
      this._fail('This preview could not load. Press Play to retry.');
      return Promise.resolve();
    }
  }

  pause() {
    ++this.attempt;
    this.wantPlay = this.playing = this.loading = false;
    this._stopClock();
    this.audio.pause();
    this._emit();
  }

  toggle() {
    if (this.wantPlay || this.playing) this.pause();
    else return this.play();
  }

  next(step = 1) {
    const resume = this.wantPlay || this.playing;
    ++this.attempt;
    this._stopClock();
    this.wantPlay = this.playing = this.loading = false;
    this.audio.pause();
    this.index = (this.index + step + this.tracks.length) % this.tracks.length;
    this.pendingSeek = null;
    this.error = '';
    this.audio.src = this.tracks[this.index].preview;
    this.audio.preload = resume ? 'auto' : 'none';
    this.prepared = resume;
    this.audio.load();
    this.onTime(0, PREVIEW_SECONDS);
    this._emit();
    if (resume) return this.play();
  }

  previous() {
    if (this.position > 3) this.seek(0);
    else return this.next(-1);
  }

  seek(position) {
    if (!Number.isFinite(position)) return;
    const value = clamp(position, 0, this.duration);
    this.pendingSeek = value;
    this.prepare();
    if (this.audio.readyState >= 1) {
      this.audio.currentTime = value;
      this.pendingSeek = null;
    }
    this.attackAt = value;
    this.onTime(value, this.duration);
    if (this.playing) this._scheduleBoundary();
  }

  setVolume(volume) {
    if (!Number.isFinite(volume)) return;
    this.volume = clamp(volume, 0, 1);
    if (this.volume > 0) {
      this.lastVolume = this.volume;
      this.muted = false;
    }
    this.audio.muted = this.muted;
    this._gain();
    this._emit();
  }

  toggleMute() {
    if (!this.volume) {
      this.volume = this.lastVolume;
      this.muted = false;
    } else this.muted = !this.muted;
    this.audio.muted = this.muted;
    this._gain();
    this._emit();
  }

  setVisible(visible) {
    this.visible = visible;
    if (!visible && this.frame) {
      this.cancelFrame(this.frame);
      this.frame = 0;
    }
    if (visible && this.playing) this._startFrames();
  }

  _sourceMatches() {
    return !this.audio.currentSrc || this.audio.currentSrc === this.audio.src;
  }

  _bind() {
    const audio = this.audio;
    audio.addEventListener('loadedmetadata', () => {
      if (!this._sourceMatches()) return;
      if (this.pendingSeek !== null) {
        audio.currentTime = clamp(this.pendingSeek, 0, this.duration);
        this.pendingSeek = null;
      }
      this.onTime(this.position, this.duration);
    });
    audio.addEventListener('playing', () => {
      if (!this.wantPlay) { audio.pause(); return; }
      if (audio.paused || !this._sourceMatches()) return;
      this.playing = true;
      this.loading = false;
      this.clearTimer(this.loadingTimer);
      this._gain();
      this._emit();
      this._startFrames();
      this._scheduleBoundary();
      this._preloadNext();
    });
    audio.addEventListener('waiting', () => {
      if (!this.wantPlay || !this._sourceMatches()) return;
      this.playing = false;
      this.loading = true;
      this._stopClock();
      this._watchLoading();
      this._emit();
    });
    audio.addEventListener('pause', () => {
      if (!audio.paused || !this._sourceMatches()) return;
      if (this.wantPlay && this._atEnd()) { this.next(); return; }
      this.playing = false;
      this._stopClock();
      this._emit();
    });
    audio.addEventListener('timeupdate', () => {
      if (!this._sourceMatches()) return;
      this._gain();
      this.onTime(this.position, this.duration);
      if (this.wantPlay && this._atEnd()) this.next();
    });
    audio.addEventListener('ended', () => {
      if (this.wantPlay && this._sourceMatches() && this._atEnd()) this.next();
    });
    audio.addEventListener('error', () => {
      if (audio.error && this._sourceMatches()) this._fail('This preview could not load. Press Play to retry.');
    });
  }

  _atEnd() {
    return this.audio.currentTime >= this.duration - 0.015;
  }

  _emit() {
    this.onState(this.getState());
  }

  _gain() {
    const elapsed = (this.audio.currentTime || 0) - this.attackAt;
    const attack = this.playing && this.visible ? clamp(elapsed / 0.06, 0, 1) : 1;
    const release = clamp((this.duration - (this.audio.currentTime || 0)) / 0.09, 0, 1);
    const gain = this.volume * Math.min(attack, release);
    if (Math.abs(this.audio.volume - gain) > 0.0001) this.audio.volume = gain;
  }

  _startFrames() {
    if (this.frame || !this.visible || !this.playing) return;
    const tick = () => {
      this.frame = 0;
      if (!this.playing || !this.visible) return;
      if (this._atEnd()) { this.next(); return; }
      this._gain();
      this.onTime(this.position, this.duration);
      this.frame = this.requestFrame(tick);
    };
    this.frame = this.requestFrame(tick);
  }

  _scheduleBoundary() {
    this.clearTimer(this.boundaryTimer);
    if (!this.wantPlay) return;
    const source = this.tracks[this.index].preview;
    this.boundaryTimer = this.setTimer(() => {
      if (!this.wantPlay || source !== this.tracks[this.index].preview) return;
      if (this._atEnd()) this.next();
      else if (!this.audio.paused) this._scheduleBoundary();
    }, Math.max(16, (this.duration - this.position) * 1000));
  }

  _watchLoading() {
    this.clearTimer(this.loadingTimer);
    this.loadingTimer = this.setTimer(() => {
      if (this.wantPlay && !this.playing) this._fail('Preview loading timed out. Press Play to retry.');
    }, 12000);
  }

  _preloadNext() {
    const index = (this.index + 1) % this.tracks.length;
    const source = this.tracks[index].preview;
    if (this.preloader?.src === source) return;
    if (this.preloader) {
      this.preloader.removeAttribute('src');
      this.preloader.load();
      this.preloader.remove?.();
    }
    this.preloader = this.createAudio();
    this.preloader.crossOrigin = 'anonymous';
    this.preloader.preload = 'auto';
    this.preloader.src = source;
    this.preloader.load();
  }

  _stopClock() {
    this.cancelFrame(this.frame);
    this.clearTimer(this.boundaryTimer);
    this.clearTimer(this.loadingTimer);
    this.frame = this.boundaryTimer = this.loadingTimer = 0;
  }

  _fail(message) {
    ++this.attempt;
    this.wantPlay = this.playing = this.loading = false;
    this.error = message;
    this._stopClock();
    this.audio.pause();
    this._emit();
  }
}
