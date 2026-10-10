// "Play together": host a game and show the code, or join with a host's code.
// The app owns the connection (see `platform/netplay.ts`); this is the modal,
// with the connection settings folded away under "Advanced".

import type { ConnectionSettings } from "../types";
import { icon } from "./icons";

export class LinkDialog {
  readonly element: HTMLDivElement;
  onHost: () => void = () => {};
  onJoin: (code: string) => void = () => {};
  /** The host starts the game with everyone who is in. */
  onStart: () => void = () => {};
  onCancel: () => void = () => {};
  onSettings: (settings: ConnectionSettings) => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal link-dialog" role="dialog" aria-modal="true" aria-labelledby="link-title">
        <header class="modal-header">
          <h2 id="link-title">Play together</h2>
          <button class="btn btn-icon" data-action="close" aria-label="Close">${icon("close")}</button>
        </header>
        <p class="muted small">
          Trade and battle as with a link cable, over the internet, with up to four players.
          Everyone needs the same ROM, and the game restarts for all from your saves, so save first.
        </p>
        <section class="link-section">
          <h3>Host</h3>
          <div class="link-row">
            <button class="btn btn-primary" data-action="host">Host a game</button>
            <code class="link-code hidden" aria-live="polite"></code>
          </div>
          <div class="link-lobby hidden">
            <p class="muted small link-players" aria-live="polite"></p>
            <button class="btn btn-primary" data-action="start" disabled>Start</button>
          </div>
        </section>
        <section class="link-section">
          <h3>Join</h3>
          <form class="link-row" data-action="join-form">
            <input class="link-input" type="text" inputmode="latin" autocapitalize="characters" autocomplete="off" spellcheck="false" maxlength="6" placeholder="Code" aria-label="Host's code" />
            <button class="btn btn-primary" type="submit">Join</button>
          </form>
        </section>
        <p class="link-status muted small" aria-live="polite"></p>
        <details class="link-advanced">
          <summary class="muted small">Advanced: connection settings</summary>
          <label class="row"><span>Introduction server</span><input type="url" data-connection="server" placeholder="Public PeerJS server" /></label>
          <label class="row"><span>Relay (TURN) URL</span><input type="text" data-connection="relayUrl" placeholder="turn:relay.example.net:3478" /></label>
          <label class="row"><span>Relay username</span><input type="text" data-connection="relayUsername" autocomplete="off" /></label>
          <label class="row"><span>Relay credential</span><input type="password" data-connection="relayCredential" autocomplete="off" /></label>
          <p class="muted small">A relay is only needed when both players sit behind strict NATs. See docs/hosting-online-play.md.</p>
        </details>
        <footer class="modal-footer">
          <button class="btn" data-action="close">Cancel</button>
        </footer>
      </div>`;
    const form = this.element.querySelector<HTMLFormElement>("[data-action=join-form]")!;
    const input = form.querySelector<HTMLInputElement>("input")!;
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const code = input.value.trim().toUpperCase();
      if (code.length === 6) this.onJoin(code);
      else this.setStatus("A code has six letters.");
    });
    for (const field of this.element.querySelectorAll<HTMLInputElement>("[data-connection]")) {
      field.addEventListener("change", () => this.onSettings(this.readSettings()));
    }
    this.element.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target === this.element) {
        this.cancel();
        return;
      }
      const button = target.closest<HTMLElement>("[data-action]");
      if (!button) return;
      switch (button.dataset["action"]) {
        case "close":
          this.cancel();
          break;
        case "host":
          this.onHost();
          break;
        case "start":
          this.onStart();
          break;
      }
    });
  }

  get isOpen(): boolean {
    return !this.element.classList.contains("hidden");
  }

  open(settings: ConnectionSettings) {
    this.setStatus("");
    this.showCode(null);
    this.setPlayers(0);
    this.setBusy(false);
    this.writeSettings(settings);
    this.element.classList.remove("hidden");
    this.element.querySelector<HTMLInputElement>(".link-input")?.focus();
  }

  close() {
    this.element.classList.add("hidden");
  }

  private cancel() {
    this.close();
    this.onCancel();
  }

  /** Shows the host's code (or hides it again). */
  showCode(code: string | null) {
    const el = this.element.querySelector<HTMLElement>(".link-code")!;
    el.textContent = code ?? "";
    el.classList.toggle("hidden", code === null);
  }

  /** Host: who is in; 0 hides the lobby. Start needs at least two. */
  setPlayers(players: number) {
    const lobby = this.element.querySelector<HTMLElement>(".link-lobby")!;
    lobby.classList.toggle("hidden", players === 0);
    this.element.querySelector(".link-players")!.textContent =
      players <= 1 ? "Tell your friends the code. Nobody else is in yet." : `${players} players in. Start when everyone is here.`;
    this.element.querySelector<HTMLButtonElement>("[data-action=start]")!.disabled = players < 2;
  }

  setStatus(text: string) {
    this.element.querySelector(".link-status")!.textContent = text;
  }

  /** Once a session is being set up, the choice has been made: no second one. */
  setBusy(busy: boolean) {
    for (const el of this.element.querySelectorAll<HTMLButtonElement | HTMLInputElement>("[data-action=host], .link-input, [type=submit]")) {
      el.disabled = busy;
    }
  }

  private readSettings(): ConnectionSettings {
    const value = (key: string) => this.element.querySelector<HTMLInputElement>(`[data-connection=${key}]`)!.value.trim();
    return { server: value("server"), relayUrl: value("relayUrl"), relayUsername: value("relayUsername"), relayCredential: value("relayCredential") };
  }

  private writeSettings(settings: ConnectionSettings) {
    for (const field of this.element.querySelectorAll<HTMLInputElement>("[data-connection]")) {
      field.value = settings[field.dataset["connection"] as keyof ConnectionSettings];
    }
  }
}
