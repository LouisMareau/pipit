// A yes/no question in a modal, for choices that affect other people.

export class ConfirmDialog {
  readonly element: HTMLDivElement;
  private resolve: ((answer: boolean) => void) | null = null;

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" aria-describedby="confirm-text">
        <header class="modal-header">
          <h2 id="confirm-title"></h2>
        </header>
        <p id="confirm-text"></p>
        <footer class="modal-footer">
          <button class="btn" data-action="cancel"></button>
          <button class="btn btn-primary" data-action="confirm"></button>
        </footer>
      </div>`;
    this.element.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target === this.element) {
        this.answer(false);
        return;
      }
      const button = target.closest<HTMLElement>("[data-action]");
      if (button) this.answer(button.dataset["action"] === "confirm");
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && this.resolve) this.answer(false);
    });
  }

  /** Shows the question; resolves with the choice (false when dismissed). */
  ask(title: string, text: string, labels: { confirm: string; cancel: string }): Promise<boolean> {
    this.answer(false);
    this.element.querySelector("#confirm-title")!.textContent = title;
    this.element.querySelector("#confirm-text")!.textContent = text;
    this.element.querySelector("[data-action=confirm]")!.textContent = labels.confirm;
    this.element.querySelector("[data-action=cancel]")!.textContent = labels.cancel;
    this.element.classList.remove("hidden");
    this.element.querySelector<HTMLButtonElement>("[data-action=cancel]")!.focus();
    return new Promise((resolve) => {
      this.resolve = resolve;
    });
  }

  private answer(value: boolean) {
    const resolve = this.resolve;
    this.resolve = null;
    this.element.classList.add("hidden");
    resolve?.(value);
  }
}
