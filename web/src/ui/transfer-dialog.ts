// Sending a game to another device, or receiving one: the modal for both
// ends. The app owns the connection (see `platform/transfer.ts`); this shows
// the code and its QR code, the words to compare, what is offered and what
// is already here, and the progress.

import { qrSvg } from "../features/qr";
import type { Item, Offer } from "../platform/transfer";
import { sizeText } from "../platform/transfer";
import type { RomEntry } from "../types";
import { icon } from "./icons";

export interface SendChoice {
  rom: boolean;
  save: boolean;
}

export interface RomCandidate {
  id: string;
  name: string;
}

/** The sender's offer against what the receiving library holds. */
export interface OfferView {
  offer: Offer;
  /** The very same ROM, already here. */
  identical: RomCandidate | null;
  /** Other builds of the same game here (same console, code and title). */
  candidates: RomCandidate[];
  /** This device's own save of the game, when the ROM is here. */
  localSave: { updatedAt: number } | null;
}

export interface ReceiveChoice {
  rom: "skip" | "add" | { update: string };
  save: boolean;
}

const when = (ms: number) => (ms ? new Date(ms).toLocaleString() : "an unknown time");
const STEPS = ["send-items", "code", "connect", "verify", "offer", "progress"] as const;

export class TransferDialog {
  readonly element: HTMLDivElement;
  onSend: (choice: SendChoice) => void = () => {};
  onAccept: () => void = () => {};
  onReject: () => void = () => {};
  onConnect: (code: string) => void = () => {};
  onReceive: (choice: ReceiveChoice) => void = () => {};
  onClose: () => void = () => {};
  private code = "";

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal transfer-dialog" role="dialog" aria-modal="true" aria-labelledby="transfer-title">
        <header class="modal-header">
          <h2 id="transfer-title" class="transfer-title">Send to another device</h2>
          <button class="btn btn-icon" data-action="close" aria-label="Close">${icon("close")}</button>
        </header>
        <section data-step="send-items">
          <p class="muted small">The other device gets the files straight from this one; nothing is uploaded. Open Pipit there and choose "Receive from another device", or scan the code this shows.</p>
          <p class="transfer-game"></p>
          <label class="row"><input type="checkbox" data-item="rom" checked /><span>ROM <span class="muted small transfer-rom-size"></span></span></label>
          <label class="row"><input type="checkbox" data-item="save" /><span>Save <span class="muted small transfer-save-info"></span></span></label>
          <footer class="modal-footer"><span></span><button class="btn btn-primary" data-action="start">Start</button></footer>
        </section>
        <section data-step="code" class="hidden">
          <div class="transfer-qr"></div>
          <div class="link-row"><button class="link-code" type="button" data-action="copy-code" title="Click to copy the code" aria-live="polite"></button></div>
          <p class="muted small">Scan the QR code with the other device's camera, or type the code into "Receive from another device" in its library.</p>
        </section>
        <section data-step="connect" class="hidden">
          <p class="muted small">Type the code the other device shows, or scan its QR code with this device's camera.</p>
          <form class="link-row" data-action="connect-form">
            <input class="link-input transfer-input" type="text" inputmode="latin" autocapitalize="characters" autocomplete="off" spellcheck="false" maxlength="8" placeholder="Code" aria-label="The sender's code" />
            <button class="btn btn-primary" type="submit">Connect</button>
          </form>
        </section>
        <section data-step="verify" class="hidden">
          <p class="muted small">Both devices show the same three words when nobody is in between. Check them before going on.</p>
          <p class="transfer-words" aria-live="polite"></p>
          <div class="link-row transfer-decide hidden">
            <button class="btn btn-primary" data-action="accept">They match, send</button>
            <button class="btn" data-action="reject">They differ</button>
          </div>
        </section>
        <section data-step="offer" class="hidden">
          <div class="transfer-offer"></div>
          <footer class="modal-footer"><span></span><button class="btn btn-primary" data-action="receive">Receive</button></footer>
        </section>
        <section data-step="progress" class="hidden">
          <p class="muted small transfer-progress-label"></p>
          <progress class="transfer-progress" max="100" value="0"></progress>
        </section>
        <p class="link-status transfer-status" aria-live="polite"></p>
        <footer class="modal-footer transfer-done hidden"><span></span><button class="btn btn-primary" data-action="close">Done</button></footer>
      </div>`;
    this.element.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target === this.element) return;
      const button = target.closest<HTMLElement>("[data-action]");
      if (!button || button.tagName === "FORM") return;
      switch (button.dataset["action"]) {
        case "close":
          this.close();
          break;
        case "start":
          this.onSend({ rom: this.checked("rom"), save: this.checked("save") });
          break;
        case "copy-code":
          void this.copyCode();
          break;
        case "accept":
          this.show("progress");
          this.onAccept();
          break;
        case "reject":
          this.onReject();
          break;
        case "receive":
          this.show("progress");
          this.onReceive(this.readChoice());
          break;
      }
    });
    this.element.querySelector<HTMLFormElement>('[data-action="connect-form"]')!.addEventListener("submit", (e) => {
      e.preventDefault();
      const code = this.input().value.trim().toUpperCase();
      if (code.length === 8) this.onConnect(code);
    });
    this.element.querySelector(".transfer-offer")!.addEventListener("change", () => this.updateSaveChoice());
  }

  /** The sender's start: what there is to send. */
  openSend(entry: RomEntry, romSize: number, save: { size: number; updatedAt: number } | null) {
    this.reset("Send to another device");
    this.element.querySelector(".transfer-game")!.textContent = entry.name;
    this.element.querySelector(".transfer-rom-size")!.textContent = `(${sizeText(romSize)})`;
    const saveBox = this.element.querySelector<HTMLInputElement>('[data-item="save"]')!;
    saveBox.checked = save !== null;
    saveBox.disabled = save === null;
    this.element.querySelector(".transfer-save-info")!.textContent = save ? `(${sizeText(save.size)}, from ${when(save.updatedAt)})` : "(none yet)";
    this.show("send-items");
    this.open();
  }

  /** The receiver's start, with a code when the QR code brought one. */
  openReceive(code: string | null) {
    this.reset("Receive from another device");
    this.input().value = code ?? "";
    this.show("connect");
    this.open();
    if (!code) this.input().focus();
  }

  /** The sender's code and its QR code. */
  showCode(code: string, url: string) {
    this.code = code;
    this.element.querySelector(".transfer-qr")!.innerHTML = qrSvg(url, `QR code for ${code}`);
    this.element.querySelector(".link-code")!.textContent = code;
    this.show("code");
    this.setStatus("Waiting for the other device…");
  }

  /** The words both devices show; the sender decides. */
  showWords(words: string, role: "host" | "guest") {
    this.element.querySelector(".transfer-words")!.textContent = words;
    this.element.querySelector(".transfer-decide")!.classList.toggle("hidden", role !== "host");
    this.show("verify");
    this.setStatus(role === "host" ? "Do the words match the other device's?" : "Check the words; the sender decides whether to go on");
  }

  /** What is offered, against what is here. */
  showOffer(view: OfferView) {
    const { offer, identical, candidates } = view;
    const box = this.element.querySelector<HTMLElement>(".transfer-offer")!;
    box.replaceChildren();
    const h3 = document.createElement("h3");
    h3.textContent = `${offer.rom.name} (${sizeText(offer.rom.size)})`;
    box.append(h3);
    const radio = (value: string, label: string, checked = false) => {
      const row = document.createElement("label");
      row.className = "row";
      row.innerHTML = `<input type="radio" name="transfer-rom" /><span></span>`;
      const input = row.querySelector("input")!;
      input.value = value;
      input.checked = checked;
      row.querySelector("span")!.textContent = label;
      return row;
    };
    const note = (text: string) => {
      const p = document.createElement("p");
      p.className = "muted small";
      p.textContent = text;
      return p;
    };
    if (identical) box.append(note(`This ROM is already in your library as "${identical.name}".`));
    else if (!offer.romData) box.append(note("The sender keeps the ROM to itself; you need it here to receive its save."));
    else {
      box.append(radio("add", "Add it to your library", candidates.length === 0));
      candidates.forEach((c, i) => box.append(radio(`update:${c.id}`, `Replace "${c.name}" with it, keeping that one's save and states`, i === 0)));
      box.append(radio("skip", "Skip the ROM"));
    }
    if (offer.save) {
      const row = document.createElement("label");
      row.className = "row";
      row.innerHTML = `<input type="checkbox" data-offer="save" checked /><span></span>`;
      box.append(row);
      this.view = view;
      this.updateSaveChoice();
    } else {
      this.view = view;
      box.append(note("The sender has no save to send."));
    }
    this.show("offer");
    this.setStatus("Choose what to take.");
  }

  private view: OfferView | null = null;

  /** The save needs somewhere to go: the ROM here, or one being received. */
  private updateSaveChoice() {
    const view = this.view;
    const row = this.element.querySelector<HTMLInputElement>('[data-offer="save"]');
    if (!view?.offer.save || !row) return;
    const target = view.identical !== null || (this.romChoice() !== "skip" && view.offer.romData);
    row.disabled = !target;
    if (!target) row.checked = false;
    const sender = view.offer.save;
    const mine = view.localSave;
    const age = mine ? (sender.updatedAt > mine.updatedAt ? "newer than" : "older than") : null;
    row.nextElementSibling!.textContent = !target
      ? "Save (take the ROM to have somewhere to put it)"
      : mine
        ? `Save from ${when(sender.updatedAt)}, ${age} yours from ${when(mine.updatedAt)}: replace yours`
        : `Save from ${when(sender.updatedAt)} (${sizeText(sender.size)}); you have none here yet`;
  }

  private romChoice(): string {
    return this.element.querySelector<HTMLInputElement>('input[name="transfer-rom"]:checked')?.value ?? "skip";
  }

  private readChoice(): ReceiveChoice {
    const rom = this.romChoice();
    return {
      rom: rom === "add" ? "add" : rom.startsWith("update:") ? { update: rom.slice(7) } : "skip",
      save: this.element.querySelector<HTMLInputElement>('[data-offer="save"]')?.checked === true,
    };
  }

  setProgress(item: Item, done: number, total: number) {
    this.show("progress");
    this.setStatus("Transferring…");
    this.element.querySelector(".transfer-progress-label")!.textContent = `${item === "rom" ? "ROM" : "Save"}: ${done} of ${total}`;
    this.element.querySelector<HTMLProgressElement>(".transfer-progress")!.value = total ? (done / total) * 100 : 0;
  }

  setStatus(text: string) {
    this.element.querySelector(".transfer-status")!.textContent = text;
  }

  /** The end, well or badly: only Done is left. */
  finish(text: string) {
    this.show(null);
    this.setStatus(text);
    this.element.querySelector(".transfer-done")!.classList.remove("hidden");
  }

  fail(text: string) {
    this.finish(text);
  }

  close() {
    if (this.element.classList.contains("hidden")) return;
    this.element.classList.add("hidden");
    this.onClose();
  }

  private open() {
    this.element.classList.remove("hidden");
  }

  private reset(title: string) {
    this.element.querySelector(".transfer-title")!.textContent = title;
    this.element.querySelector(".transfer-done")!.classList.add("hidden");
    this.element.querySelector(".transfer-offer")!.replaceChildren();
    this.element.querySelector(".transfer-qr")!.replaceChildren();
    this.setStatus("");
    this.view = null;
    this.code = "";
  }

  private show(step: (typeof STEPS)[number] | null) {
    for (const name of STEPS) {
      this.element.querySelector(`[data-step="${name}"]`)!.classList.toggle("hidden", name !== step);
    }
  }

  private checked(item: string): boolean {
    const box = this.element.querySelector<HTMLInputElement>(`[data-item="${item}"]`)!;
    return box.checked && !box.disabled;
  }

  private input(): HTMLInputElement {
    return this.element.querySelector<HTMLInputElement>(".transfer-input")!;
  }

  private async copyCode() {
    const el = this.element.querySelector<HTMLElement>(".link-code")!;
    let ok = false;
    try {
      await navigator.clipboard.writeText(this.code);
      ok = true;
    } catch {
      ok = false;
    }
    el.classList.toggle("copied", ok);
    this.setStatus(ok ? "Code copied." : "Could not copy; read the code out or scan the QR code.");
    if (ok) setTimeout(() => el.classList.remove("copied"), 1500);
  }
}
