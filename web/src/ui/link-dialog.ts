// "Play together": host a game and show the code, or join with a partner's code.
// The app owns the connection (see `platform/netplay.ts`); this is only the modal.

import { icon } from "./icons";

export class LinkDialog {
  readonly element: HTMLDivElement;
  onHost: () => void = () => {};
  onJoin: (code: string) => void = () => {};
  onCancel: () => void = () => {};

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
          Trade and battle as with a link cable, over the internet. Both of you need the
          same ROM, and the game restarts for both from your saves, so save first.
        </p>
        <section class="link-section">
          <h3>Host</h3>
          <div class="link-row">
            <button class="btn btn-primary" data-action="host">Host a game</button>
            <code class="link-code hidden" aria-live="polite"></code>
          </div>
          <p class="muted small link-host-hint hidden">Tell your partner this code.</p>
        </section>
        <section class="link-section">
          <h3>Join</h3>
          <form class="link-row" data-action="join-form">
            <input class="link-input" type="text" inputmode="latin" autocapitalize="characters" autocomplete="off" spellcheck="false" maxlength="6" placeholder="Code" aria-label="Partner's code" />
            <button class="btn btn-primary" type="submit">Join</button>
          </form>
        </section>
        <p class="link-status muted small" aria-live="polite"></p>
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
      }
    });
  }

  get isOpen(): boolean {
    return !this.element.classList.contains("hidden");
  }

  open() {
    this.setStatus("");
    this.showCode(null);
    this.setBusy(false);
    this.element.classList.remove("hidden");
    this.element.querySelector<HTMLInputElement>("input")?.focus();
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
    const hint = this.element.querySelector<HTMLElement>(".link-host-hint")!;
    el.textContent = code ?? "";
    el.classList.toggle("hidden", code === null);
    hint.classList.toggle("hidden", code === null);
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
}
