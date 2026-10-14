// Frame pacing locked to the display.
//
// The GBA draws 59.7275 frames per second; a 60 Hz display refreshes 60 times.
// Pacing the emulator by the wall clock therefore shows one frame twice every
// ~3.7 s: a periodic judder. Instead, `requestAnimationFrame` drives emulation:
// one emulated frame per refresh on a 60 Hz display (every second refresh on
// 120 Hz, and so on), which runs the game 0.46 % fast — a difference the audio
// output absorbs by resampling (see `public/audio-worklet.js`). Displays whose
// rate is not a multiple of ~60 Hz fall back to time-based pacing.
//
// The refresh rate is judged from the last dozen refreshes only. Phones and
// laptops with adaptive refresh switch between 60 and 120 Hz as the user
// touches or types, and a judgement that lagged behind would run the game at
// half or double speed until it caught up.

const FRAME_MS = 1000 / (16_777_216 / 280_896); // 59.7275 Hz
/** Refresh intervals remembered: enough to ride out dropped frames, few enough to follow a rate change in 0.2 s. */
const WINDOW = 12;

export class FramePacer {
  private rafId: number | null = null;
  private lastTime = 0;
  private intervals: number[] = [];
  private refreshes = 0;
  private accumulated = 0;
  /** Called once per emulated frame that is due. */
  onFrame: () => void = () => {};
  /** How the pacing stands: locked to every nth refresh, or 0 when timing frames by the clock. */
  locked = 0;
  /** The refresh rate as judged now, in Hz. */
  get refreshHz(): number {
    return 1000 / this.medianInterval();
  }

  start() {
    if (this.rafId !== null) return;
    this.lastTime = 0;
    this.accumulated = 0;
    this.refreshes = 0;
    this.rafId = requestAnimationFrame(this.loop);
  }

  stop() {
    if (this.rafId !== null) cancelAnimationFrame(this.rafId);
    this.rafId = null;
  }

  private loop = (time: number) => {
    this.rafId = requestAnimationFrame(this.loop);
    if (!this.lastTime) {
      this.lastTime = time;
      this.onFrame();
      return;
    }
    const dt = time - this.lastTime;
    this.lastTime = time;
    if (dt <= 0) return;
    if (dt < 200) {
      this.intervals.push(dt);
      if (this.intervals.length > WINDOW) this.intervals.shift();
    }

    const refresh = this.medianInterval();
    const ratio = FRAME_MS / refresh; // 1.005 on 60 Hz, 2.009 on 120 Hz
    const n = Math.round(ratio);
    if (this.intervals.length >= WINDOW * 2 / 3 && n >= 1 && Math.abs(ratio - n) < 0.02 * n) {
      // Display-locked: one emulated frame every n refreshes. The refreshes
      // are counted from the time elapsed, not the callbacks: a callback that
      // came late covers the refreshes missed, and one that came early (the
      // display just sped up, before the judgement above caught up) counts
      // for the fraction it is, so the game's speed never follows the display.
      this.accumulated = 0;
      this.locked = n;
      const k = dt / refresh;
      this.refreshes += k < 0.75 ? k : Math.max(1, Math.round(k));
      let emitted = 0;
      while (this.refreshes >= n && emitted < 2) {
        this.refreshes -= n;
        this.onFrame();
        emitted++;
      }
      if (this.refreshes > n) this.refreshes = n;
      return;
    }
    // Free-running display: emulate by elapsed time, never sprinting after a stall.
    this.locked = 0;
    this.accumulated = Math.min(this.accumulated + dt, FRAME_MS * 2);
    while (this.accumulated >= FRAME_MS) {
      this.accumulated -= FRAME_MS;
      this.onFrame();
    }
  };

  private medianInterval(): number {
    if (this.intervals.length === 0) return 1000 / 60;
    const sorted = [...this.intervals].sort((a, b) => a - b);
    return sorted[sorted.length >> 1]!;
  }
}
