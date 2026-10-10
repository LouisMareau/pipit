// The game screen: a canvas at the console's resolution, scaled with crisp pixels.

import { SCREEN_HEIGHT, SCREEN_WIDTH } from "../types";

export class Screen {
  readonly element: HTMLCanvasElement;
  private context: CanvasRenderingContext2D;
  private image: ImageData;
  private integerScale = false;

  constructor() {
    this.element = document.createElement("canvas");
    this.element.className = "screen";
    const context = this.element.getContext("2d", { alpha: false });
    if (!context) throw new Error("2D canvas not supported");
    this.context = context;
    this.image = context.createImageData(SCREEN_WIDTH, SCREEN_HEIGHT);
    this.setSize(SCREEN_WIDTH, SCREEN_HEIGHT);
    new ResizeObserver(() => this.fit()).observe(this.element.parentElement ?? document.body);
  }

  get width(): number {
    return this.element.width;
  }

  get height(): number {
    return this.element.height;
  }

  /** The console's resolution (240×160 for the Advance, 160×144 for the Game Boy). */
  setSize(width: number, height: number) {
    if (this.element.width === width && this.element.height === height) return;
    this.element.width = width;
    this.element.height = height;
    this.image = this.context.createImageData(width, height);
    this.context.fillStyle = "#000";
    this.context.fillRect(0, 0, width, height);
    this.fit();
  }

  draw(pixels: ArrayBuffer) {
    if (pixels.byteLength !== this.image.data.byteLength) return;
    this.image.data.set(new Uint8ClampedArray(pixels));
    this.context.putImageData(this.image, 0, 0);
  }

  setIntegerScale(on: boolean) {
    this.integerScale = on;
    this.fit();
  }

  /** Sizes the canvas to its container, keeping the console's aspect ratio. */
  fit() {
    const parent = this.element.parentElement;
    if (!parent) return;
    const { clientWidth: w, clientHeight: h } = parent;
    let scale = Math.min(w / this.width, h / this.height);
    if (this.integerScale) scale = Math.max(1, Math.floor(scale));
    this.element.style.width = `${Math.floor(this.width * scale)}px`;
    this.element.style.height = `${Math.floor(this.height * scale)}px`;
  }

  /** Current frame as a PNG blob, for screenshots. */
  toBlob(): Promise<Blob | null> {
    return new Promise((resolve) => this.element.toBlob(resolve, "image/png"));
  }
}
