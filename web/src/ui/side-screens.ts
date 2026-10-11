// Partners' screens beside the player's own, in a session: one small canvas per
// other console, drawn from the pictures the worker sends with each frame.

import { SCREEN_HEIGHT, SCREEN_WIDTH } from "../types";

interface Side {
  canvas: HTMLCanvasElement;
  context: CanvasRenderingContext2D;
  image: ImageData;
}

export class SideScreens {
  readonly element: HTMLDivElement;
  private sides = new Map<number, Side>();

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "side-screens hidden";
  }

  get isShown(): boolean {
    return !this.element.classList.contains("hidden");
  }

  /** One screen per console other than `shown` (the one on the main screen). */
  setPlayers(players: number, shown: number, width = SCREEN_WIDTH, height = SCREEN_HEIGHT) {
    this.sides.clear();
    this.element.replaceChildren();
    for (let player = 0; player < players; player++) {
      if (player === shown) continue;
      const figure = document.createElement("figure");
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d", { alpha: false });
      if (!context) continue;
      context.fillStyle = "#000";
      context.fillRect(0, 0, width, height);
      const caption = document.createElement("figcaption");
      caption.textContent = `Player ${player + 1}`;
      figure.append(canvas, caption);
      this.element.append(figure);
      this.sides.set(player, { canvas, context, image: context.createImageData(width, height) });
    }
  }

  draw(player: number, pixels: ArrayBuffer) {
    const side = this.sides.get(player);
    if (!side || pixels.byteLength !== side.image.data.byteLength) return;
    side.image.data.set(new Uint8ClampedArray(pixels));
    side.context.putImageData(side.image, 0, 0);
  }

  show(on: boolean) {
    this.element.classList.toggle("hidden", !on);
  }

  clear() {
    this.sides.clear();
    this.element.replaceChildren();
    this.show(false);
  }
}
