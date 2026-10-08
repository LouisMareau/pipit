// Audio output through an AudioWorklet. Browsers only start audio after a user
// gesture, so `resume()` is called from the first click or key press.

export class AudioOutput {
  private context: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private pending: Int16Array[] = [];
  private volume = 1;
  private ready: Promise<void> | null = null;

  async start(): Promise<void> {
    if (this.ready) return this.ready;
    this.ready = (async () => {
      const context = new AudioContext({ sampleRate: 32768, latencyHint: "interactive" });
      await context.audioWorklet.addModule(`${import.meta.env.BASE_URL}audio-worklet.js`);
      const node = new AudioWorkletNode(context, "pipit-audio", { outputChannelCount: [2] });
      node.connect(context.destination);
      this.context = context;
      this.node = node;
      node.port.postMessage({ type: "volume", volume: this.volume });
      for (const chunk of this.pending) this.push(chunk);
      this.pending = [];
    })();
    return this.ready;
  }

  /** Resumes a context the browser suspended (must run inside a user gesture). */
  async resume() {
    await this.start();
    if (this.context?.state === "suspended") await this.context.resume();
  }

  push(samples: Int16Array) {
    if (!this.node) {
      if (this.pending.length < 8) this.pending.push(samples);
      return;
    }
    this.node.port.postMessage({ type: "samples", samples }, [samples.buffer]);
  }

  setVolume(volume: number) {
    this.volume = volume;
    this.node?.port.postMessage({ type: "volume", volume });
  }

  clear() {
    this.node?.port.postMessage({ type: "clear" });
  }
}
