// DS console settings: the nickname, favourite colour, birthday and language
// that DS games show, kept in the firmware image the app stores (Pipit's own
// image, or one the player imported). Opened from the player menu.

import type { DsUser } from "../types";
import { icon } from "./icons";

const LANGUAGES = ["Japanese", "English", "French", "German", "Italian", "Spanish"];
/** The sixteen favourite colours, in the firmware's order. */
const COLORS = [
  "Grey", "Brown", "Red", "Pink", "Orange", "Yellow", "Lime", "Green",
  "Dark green", "Teal", "Light blue", "Blue", "Purple", "Violet", "Magenta", "Dark pink",
];
const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const options = (labels: string[], from = 0) =>
  labels.map((label, i) => `<option value="${i + from}">${label}</option>`).join("");

export class DsSettings {
  readonly element: HTMLDivElement;
  onSave: (user: DsUser) => void = () => {};
  onClose: () => void = () => {};
  onImport: () => void = () => {};
  onReset: () => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal ds-settings" role="dialog" aria-modal="true" aria-labelledby="ds-settings-title">
        <header class="modal-header">
          <h2 id="ds-settings-title">DS console settings</h2>
          <button class="btn btn-icon" data-action="close" aria-label="Close">${icon("close")}</button>
        </header>
        <p class="muted small">What DS games show as your name, colour and birthday. Changes take effect when a DS game starts.</p>
        <label class="row"><span>Nickname</span><input type="text" maxlength="10" data-ds="nickname" /></label>
        <label class="row"><span>Favourite colour</span><select data-ds="color">${options(COLORS)}</select></label>
        <label class="row"><span>Birthday</span>
          <select data-ds="month">${options(MONTHS, 1)}</select>
          <select data-ds="day">${options(Array.from({ length: 31 }, (_, i) => String(i + 1)), 1)}</select>
        </label>
        <label class="row"><span>Language</span><select data-ds="language">${options(LANGUAGES)}</select></label>
        <p class="muted small ds-firmware-note"></p>
        <footer class="modal-footer">
          <span class="ds-firmware-actions">
            <button class="btn" data-action="import" title="Use your own DS's firmware image (a .bin dump) instead of Pipit's">Import firmware…</button>
            <button class="btn" data-action="reset" title="Back to Pipit's own firmware image and settings">Reset</button>
          </span>
          <button class="btn btn-primary" data-action="save">Save</button>
        </footer>
      </div>`;
    this.element.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target === this.element) {
        this.close();
        return;
      }
      const button = target.closest<HTMLElement>("[data-action]");
      if (!button) return;
      switch (button.dataset["action"]) {
        case "close":
          this.close();
          break;
        case "save":
          this.onSave(this.read());
          this.close();
          break;
        case "import":
          this.onImport();
          break;
        case "reset":
          this.onReset();
          break;
      }
    });
  }

  /** Shows the settings `user` holds; `note` says which firmware image they live in. */
  open(user: DsUser, note: string) {
    this.field("nickname").value = user.nickname;
    this.field("color").value = String(user.color);
    this.field("month").value = String(user.month);
    this.field("day").value = String(user.day);
    this.field("language").value = String(user.language);
    this.element.querySelector(".ds-firmware-note")!.textContent = note;
    this.element.classList.remove("hidden");
    this.field("nickname").focus();
  }

  close() {
    if (this.element.classList.contains("hidden")) return;
    this.element.classList.add("hidden");
    this.onClose();
  }

  private read(): DsUser {
    return {
      nickname: this.field("nickname").value.trim().slice(0, 10),
      color: Number(this.field("color").value),
      month: Number(this.field("month").value),
      day: Number(this.field("day").value),
      language: Number(this.field("language").value),
    };
  }

  private field(name: string): HTMLInputElement {
    return this.element.querySelector<HTMLInputElement>(`[data-ds="${name}"]`)!;
  }
}
