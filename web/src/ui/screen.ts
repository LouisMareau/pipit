// The game screen: a 240×160 canvas scaled with crisp pixels.

import { SCREEN_HEIGHT, SCREEN_WIDTH } from "../types";

export class Screen {
  readonly element: HTMLCanvasElement;
  private context: CanvasRenderingContext2D;
  private image: ImageData;
  private integerScale = false;

  constructor() {
    this.element = document.createElement("canvas");
    this.element.className = "screen";
    this.element.width = SCREEN_WIDTH;
    this.element.height = SCREEN_HEIGHT;
    const context = this.element.getContext("2d", { alpha: false });
    if (!context) throw new Error("2D canvas not supported");
    this.context = context;
    this.image = context.createImageData(SCREEN_WIDTH, SCREEN_HEIGHT);
    new ResizeObserver(() => this.fit()).observe(this.element.parentElement ?? document.body);
  }

  draw(pixels: ArrayBuffer) {
    this.image.data.set(new Uint8ClampedArray(pixels));
    this.context.putImageData(this.image, 0, 0);
  }

  setIntegerScale(on: boolean) {
    this.integerScale = on;
    this.fit();
  }

  /** Sizes the canvas to its container, keeping the 3:2 aspect ratio. */
  fit() {
    const parent = this.element.parentElement;
    if (!parent) return;
    const { clientWidth: w, clientHeight: h } = parent;
    let scale = Math.min(w / SCREEN_WIDTH, h / SCREEN_HEIGHT);
    if (this.integerScale) scale = Math.max(1, Math.floor(scale));
    this.element.style.width = `${Math.floor(SCREEN_WIDTH * scale)}px`;
    this.element.style.height = `${Math.floor(SCREEN_HEIGHT * scale)}px`;
  }

  /** Current frame as a PNG blob, for screenshots. */
  toBlob(): Promise<Blob | null> {
    return new Promise((resolve) => this.element.toBlob(resolve, "image/png"));
  }
}
