// Top-level UI: library screen and player screen, wired to the emulator worker.

import { exportSave, pickFile, pickSaveFile } from "../features/saves";
import { takeScreenshot } from "../features/screenshots";
import { AudioOutput } from "../platform/audio";
import { EmulatorClient } from "../platform/emulator-client";
import { Input, keyLabel } from "../platform/input";
import { linkLog, linkLogText } from "../platform/link-log";
import type { Pending, Role, SessionStart } from "../platform/netplay";
import { connect, generateCode, hashRom, LinkSession, listen } from "../platform/netplay";
import { FramePacer } from "../platform/pacer";
import * as storage from "../platform/storage";
import type { ControllerMapping, DsUser, ResolvedTouchLayout, RomEntry, Settings, Slot2Load, TouchLayout } from "../types";
import { DEFAULT_CONNECTION, DEFAULT_KEYBOARD, DEFAULT_MAPPING, Key, PEN_DOWN, PEN_INCLUDED, SAVE_TYPES, STATE_SLOTS } from "../types";
import { DsSettings } from "./ds-settings";
import { TransferDialog } from "./transfer-dialog";
import type { OfferView, ReceiveChoice, SendChoice } from "./transfer-dialog";
import { TransferGuest, TransferHost } from "../platform/transfer";
import type { Items, Offer, Received } from "../platform/transfer";
import { ConfirmDialog } from "./confirm-dialog";
import { ControllerSettings } from "./controller-settings";
import { ControllerToggle } from "./controller-toggle";
import { icon } from "./icons";
import { KeyboardSettings } from "./keyboard-settings";
import { LayoutEditor } from "./layout-editor";
import { Library } from "./library";
import { LinkDialog } from "./link-dialog";
import { Screen } from "./screen";
import { SideScreens } from "./side-screens";
import { TouchControls } from "./touch-controls";

/** `window.pipit`, for the end-to-end scripts in `scripts/`. */
interface TestHooks {
  /** Keys per game frame on a link, as `frame=KEYS,…` (see `parseKeyScript`). */
  keyScript: string | null;
  save: () => Promise<ArrayBuffer>;
  /** The session log so far (what the browser console shows under "[pipit link]"). */
  linkLog: () => string;
  /** The library's saves and entries, for the transfer checks. */
  putSave: (id: string, data: ArrayBuffer) => Promise<void>;
  getSave: (id: string) => Promise<ArrayBuffer | null>;
  listRoms: () => Promise<RomEntry[]>;
  refreshLibrary: () => Promise<void>;
  /** A ROM made for a test, and its removal. */
  addRom: (name: string, data: ArrayBuffer) => Promise<RomEntry>;
  deleteRom: (id: string) => Promise<void>;
}

/** `300=START,330=,600=A+RIGHT`: the keys held from each frame on (names as in `Key`). */
class ScriptedKeys {
  private readonly entries: [number, number][] = [];

  constructor(script: string) {
    for (const entry of script.split(",")) {
      const [frame, names = ""] = entry.split("=");
      if (!frame?.trim()) continue;
      let keys = 0;
      for (const name of names.split("+")) {
        const key = (Object.keys(Key) as (keyof typeof Key)[]).find((k) => k.toLowerCase() === name.trim().toLowerCase());
        if (key) keys |= Key[key];
      }
      this.entries.push([Number(frame), keys]);
    }
    this.entries.sort((a, b) => a[0] - b[0]);
  }

  /** The keys held at `frame`: the latest entry at or before it. */
  at(frame: number): number {
    let keys = 0;
    for (const [from, value] of this.entries) {
      if (from > frame) break;
      keys = value;
    }
    return keys;
  }
}

export class App {
  private root: HTMLElement;
  private library = new Library();
  private player: HTMLDivElement;
  private screen: Screen;
  private sideScreens = new SideScreens();
  private touch = new TouchControls();
  private controller = new ControllerToggle();
  private controllerSettings = new ControllerSettings();
  private keyboardSettings = new KeyboardSettings();
  private dsSettings = new DsSettings();
  // Sending a game to another device, or receiving one (see `platform/transfer.ts`).
  private transferDialog = new TransferDialog();
  private transferHost: TransferHost | null = null;
  private transferGuest: TransferGuest | null = null;
  private transferEntry: RomEntry | null = null;
  private transferRom: ArrayBuffer | null = null;
  private transferOffer: OfferView | null = null;
  private transferChoice: ReceiveChoice | null = null;
  /** A console-settings save is on its way back as a firmware image. */
  private dsSettingsPending = false;
  private layoutEditor: LayoutEditor;
  /** The touch layout in effect after "auto" is resolved from the orientation. */
  private resolvedLayout: ResolvedTouchLayout = "gbasp";
  private emulator: EmulatorClient;
  private audio = new AudioOutput();
  private input = new Input();
  private pacer = new FramePacer();
  private settings: Settings;
  private current: RomEntry | null = null;
  private paused = false;
  private saveTimer: number | null = null;
  private latestSave: ArrayBuffer | null = null;
  private linkDialog = new LinkDialog();
  private confirmDialog = new ConfirmDialog();
  /** Playing together (see platform/netplay.ts); null when playing alone. */
  private session: LinkSession | null = null;
  /** A connection being made for a session (hosting or joining). */
  private pendingLink: { cancel(): void } | null = null;
  /** Keys the player holds now; on a link they reach the game a few frames later. */
  private heldKeys = 0;
  /** The pen on a DS's bottom screen, while it is down. */
  private pen: { x: number; y: number } | null = null;
  /** The GBA game whose cartridge is in the DS's GBA slot, for writing its save back. */
  private slot2RomId: string | null = null;
  private stalledTicks = 0;
  /** Test hooks (see scripts/): scripted keys on a link, and a copy of the save. */
  private hooks: TestHooks = {
    keyScript: null,
    save: () => this.copyOfSave(),
    linkLog: () => linkLogText(),
    putSave: (id, data) => storage.putSave(id, data),
    getSave: (id) => storage.getSave(id),
    listRoms: () => storage.listRoms(),
    refreshLibrary: () => this.library.refresh(),
    addRom: (name, data) => storage.addRom(new File([data], name)),
    deleteRom: (id) => storage.deleteRom(id),
  };
  private keyScript: ScriptedKeys | null = null;
  private saveWaiters: ((data: ArrayBuffer) => void)[] = [];

  constructor(root: HTMLElement, settings: Settings) {
    this.root = root;
    // Older saved settings may predate some actions: fill the gaps with defaults.
    this.settings = {
      ...settings,
      keyboardMapping: { ...DEFAULT_KEYBOARD, ...settings.keyboardMapping },
      touchLayouts: settings.touchLayouts ?? {},
      connection: { ...DEFAULT_CONNECTION, ...settings.connection },
    };
    this.emulator = new EmulatorClient();
    this.player = document.createElement("div");
    this.player.className = "player hidden";
    const slotButtons = (action: string) =>
      Array.from({ length: STATE_SLOTS }, (_, i) => `<button class="btn" data-action="${action}" data-slot="${i + 1}">${i + 1}</button>`).join("");
    // `.actions` lives in the toolbar, or inside the menu drawer in the landscape
    // touch layout (see applyLayout).
    this.player.innerHTML = `
      <div class="toolbar">
        <div class="actions">
          <button class="btn btn-icon" data-action="back" title="Library" aria-label="Library">${icon("back")}</button>
          <span class="toolbar-spacer"></span>
          <button class="link-pill" data-action="link-stats" aria-pressed="false" aria-label="Session details" title="Round trip / delay — click for the session's details"><span class="link-pill-ping">–</span><span class="link-pill-sep">/</span><span class="link-pill-delay">–</span></button>
          <span class="toolbar-controller"></span>
          <button class="btn btn-icon" data-action="pause" title="Pause" aria-label="Pause">${icon("pause")}</button>
          <button class="btn btn-icon" data-action="fast" title="Fast-forward" aria-label="Fast-forward">${icon("fastForward")}</button>
          <button class="btn btn-icon" data-action="shot" title="Screenshot" aria-label="Screenshot">${icon("camera")}</button>
          <button class="btn btn-icon" data-action="menu" title="More" aria-label="More">${icon("more")}</button>
        </div>
      </div>
      <div class="stage">
        <div class="screen-box"></div>
        <button class="menu-fab" data-action="menu" title="Menu" aria-label="Menu">${icon("menu", 22)}</button>
        <div class="link-wait">Waiting for your partner…</div>
        <div class="link-stats hidden" role="dialog" aria-label="Session details">
          <dl>
            <dt>Players</dt><dd data-stat="players">—</dd>
            <dt>Link</dt><dd data-stat="link">—</dd>
            <dt>Round trip</dt><dd data-stat="ping">—</dd>
            <dt>Delay</dt><dd data-stat="delay">—</dd>
            <dt>Rollbacks</dt><dd data-stat="rollbacks">—</dd>
            <dt>Frame</dt><dd data-stat="frame">—</dd>
            <dt>Frame rate</dt><dd data-stat="fps">—</dd>
            <dt>In-game link</dt><dd data-stat="activity">—</dd>
          </dl>
          <div class="link-stats-actions"><button class="btn" data-action="copy-link-log">Copy session log</button></div>
        </div>
      </div>
      <div class="menu-backdrop hidden" data-action="menu"></div>
      <div class="menu hidden">
        <header class="menu-header">
          <h3 class="menu-title"></h3>
          <span class="toolbar-fps muted small"></span>
        </header>
        <div class="menu-actions"></div>
        <div class="states">
          <h4>Save states</h4>
          <div class="slots"><span class="small">Save</span>${slotButtons("save-state")}</div>
          <div class="slots"><span class="small">Load</span>${slotButtons("load-state")}</div>
          <p class="muted small">Shift+F1–F3 saves, F1–F3 loads.</p>
        </div>
        <h4>Link cable</h4>
        <button class="btn" data-action="link">Play together…</button>
        <button class="btn hidden" data-action="leave-link">Leave the session</button>
        <button class="btn hidden" data-action="copy-link-log" title="What the session did, for bug reports; the browser console (F12) shows it too">Copy session log</button>
        <label class="row link-view hidden"><span>Watch</span><select data-view></select></label>
        <label class="row link-side"><input type="checkbox" data-setting="sideScreens" /><span>Show the other players&#8217; screens beside yours</span></label>
        <h4 class="ds-only">DS</h4>
        <label class="row ds-only"><span>GBA slot</span><select data-slot2></select></label>
        <label class="row ds-only" title="Pipit tells the card's save chip from the game's first write; name it when a game loses its save"><span>Save chip</span><select data-save-type>${SAVE_TYPES.map(([v, l]) => `<option value="${v}">${l}</option>`).join("")}</select></label>
        <button class="btn ds-only" data-action="ds-settings">Console settings…</button>
        <h4>Keyboard</h4>
        <button class="btn" data-action="keyboard">Change key bindings…</button>
        <h4>Battery save</h4>
        <button class="btn" data-action="export">Export save (.sav)</button>
        <button class="btn" data-action="import">Import save (.sav)</button>
        <h4>Settings</h4>
        <label class="row"><span>Volume</span><input type="range" min="0" max="1" step="0.05" data-setting="volume" /></label>
        <label class="row"><span>Colours</span>
          <select data-setting="colorCorrection">
            <option value="gba">LCD — muted, as on the original screen</option>
            <option value="off">Raw — the palette as stored in the game</option>
          </select>
        </label>
        <label class="row color-strength"><span>Strength</span><input type="range" min="0" max="100" step="5" data-setting="colorStrength" /><span class="small strength-label"></span></label>
        <label class="row"><span>Touch layout</span>
          <select data-setting="touchLayout">
            <option value="auto">Auto (GBA in landscape, GBA SP in portrait)</option>
            <option value="gba">GBA — controls beside the screen</option>
            <option value="gbasp">GBA SP — controls below the screen</option>
          </select>
        </label>
        <button class="btn" data-action="edit-layout">Edit touch layout…</button>
        <label class="row"><input type="checkbox" data-setting="integerScale" /><span>Integer pixel scaling</span></label>
        <label class="row"><input type="checkbox" data-setting="alwaysShowTouch" /><span>Always show touch controls</span></label>
        <button class="btn" data-action="fullscreen">Fullscreen</button>
      </div>`;
    this.screen = new Screen();
    this.player.querySelector(".screen-box")!.append(this.screen.element, this.sideScreens.element);
    this.player.querySelector(".toolbar-controller")!.append(this.controller.element);
    this.stage.append(this.touch.element);
    this.layoutEditor = new LayoutEditor(this.stage);
    this.player.append(this.controllerSettings.element, this.keyboardSettings.element, this.dsSettings.element, this.linkDialog.element, this.confirmDialog.element);
    this.root.append(this.library.element, this.player, this.transferDialog.element);
    this.wireTransfer();

    this.library.onPlay = (entry) => this.play(entry);
    this.wireToolbar();
    this.wireSettings();
    this.wireEmulator();
    this.wireController();
    this.wireKeyboard();
    this.wireLayoutEditor();
    this.wireLink();

    this.input.attach(window);
    this.input.onChange = (keys) => {
      this.heldKeys = keys;
      this.emulator.setKeys(keys);
    };
    this.input.onFastForward = (held) => this.setFastForward(held);
    this.input.onPause = () => {
      if (this.current) this.togglePause();
    };
    this.touch.onChange = (keys) => this.input.setTouch(keys);
    this.wireScreenPen();

    // Nothing in the game area opens a context menu: a long press on a phone
    // would otherwise buzz and pop one up over the controls or the screen.
    this.player.querySelector(".stage")!.addEventListener("contextmenu", (e) => e.preventDefault());
    // Audio can only start from a user gesture.
    const unlock = () => this.audio.resume();
    window.addEventListener("pointerdown", unlock, { passive: true });
    window.addEventListener("keydown", unlock);

    document.addEventListener("visibilitychange", () => {
      if (document.hidden && this.current && !this.paused) this.emulator.requestSave();
    });
    window.addEventListener("beforeunload", (e) => {
      this.flushSave();
      // Closing the tab mid-session ends it for everyone: let the browser ask.
      if (this.session && this.player.classList.contains("linked")) e.preventDefault();
    });
    window.addEventListener("resize", () => this.applyLayout());
    this.applySettings();
    (window as unknown as { pipit: TestHooks }).pipit = this.hooks;
  }

  async start() {
    await this.library.refresh();
    // A QR code scanned on this device: receive from the device that showed it.
    const params = new URLSearchParams(location.search);
    const code = params.get("receive");
    if (code) {
      params.delete("receive");
      history.replaceState(null, "", `${location.pathname}${params.size ? `?${params}` : ""}`);
      this.openReceive(code.toUpperCase());
    }
  }

  private get localLink(): boolean {
    return new URLSearchParams(location.search).get("link") === "local";
  }

  private wireTransfer() {
    const dialog = this.transferDialog;
    this.library.onHold = (entry) => void this.openSend(entry);
    this.library.onReceive = () => this.openReceive(null);
    dialog.onClose = () => {
      this.transferHost?.cancel();
      this.transferGuest?.cancel();
      this.transferHost = null;
      this.transferGuest = null;
      this.transferEntry = null;
      this.transferRom = null;
      this.transferOffer = null;
      this.transferChoice = null;
    };
    dialog.onSend = (choice) => void this.startSending(choice);
    dialog.onAccept = () => this.transferHost?.accept();
    dialog.onReject = () => this.transferHost?.reject();
    dialog.onConnect = (code) => void this.startReceiving(code);
    dialog.onReceive = (choice) => {
      this.transferChoice = choice;
      this.transferGuest?.request(choice.rom !== "skip", choice.save);
    };
  }

  private async openSend(entry: RomEntry) {
    const rom = await storage.getRomData(entry.id);
    if (!rom) return;
    this.transferEntry = entry;
    this.transferRom = rom;
    this.transferDialog.openSend(entry, rom.byteLength, await storage.getSaveMeta(entry.id));
  }

  private async startSending(choice: SendChoice) {
    const entry = this.transferEntry;
    const rom = this.transferRom;
    if (!entry || !rom) return;
    const save = choice.save ? await storage.getSave(entry.id) : null;
    const meta = save ? await storage.getSaveMeta(entry.id) : null;
    const items: Items = { entry, rom: choice.rom ? rom : null, save: save ? { data: save, updatedAt: meta?.updatedAt ?? 0 } : null };
    this.transferDialog.setStatus("Setting up…");
    try {
      const host = await TransferHost.start(this.localLink, this.settings.connection, items);
      this.transferHost = host;
      const url = `${location.origin}${location.pathname}?receive=${host.code}${this.localLink ? "&link=local" : ""}`;
      this.transferDialog.showCode(host.code, url);
      host.onStatus = (text) => this.transferDialog.setStatus(text);
      host.onVerify = (words) => this.transferDialog.showWords(words, "host");
      host.onProgress = (item, done, total) => this.transferDialog.setProgress(item, done, total);
      host.onError = (message) => this.transferDialog.fail(message);
      host.onFinished = (sent) => void this.sentDone(entry, sent);
    } catch (error) {
      this.transferDialog.fail(error instanceof Error ? error.message : String(error));
    }
  }

  private async sentDone(entry: RomEntry, sent: { rom: boolean; save: boolean }) {
    const now = Date.now();
    await storage.updateRom(entry.id, { transferredAt: now, ...(sent.save ? { savedAt: now } : {}) });
    this.transferHost = null;
    this.transferDialog.finish(sent.rom || sent.save ? "Sent." : "The other device took nothing.");
    await this.library.refresh();
  }

  private openReceive(code: string | null) {
    this.transferDialog.openReceive(code);
    if (code) void this.startReceiving(code);
  }

  private async startReceiving(code: string) {
    this.transferDialog.setStatus("Connecting…");
    try {
      const guest = await TransferGuest.connect(code, this.localLink, this.settings.connection);
      this.transferGuest = guest;
      guest.onStatus = (text) => this.transferDialog.setStatus(text);
      guest.onVerify = (words) => this.transferDialog.showWords(words, "guest");
      guest.onOffer = (offer) => void this.showOffer(offer);
      guest.onProgress = (item, done, total) => this.transferDialog.setProgress(item, done, total);
      guest.onError = (message) => this.transferDialog.fail(message);
      guest.onReceived = (received) => void this.storeReceived(received);
    } catch (error) {
      this.transferDialog.fail(error instanceof Error ? error.message : String(error));
    }
  }

  /** What the sender offers, against the library here. */
  private async showOffer(offer: Offer) {
    const entries = await storage.listRoms();
    const identical = entries.find((e) => e.id === offer.rom.id) ?? null;
    const candidates = entries.filter(
      (e) => e.id !== offer.rom.id && (e.system ?? "gba") === offer.rom.system && e.gameCode === offer.rom.gameCode && e.title === offer.rom.title,
    );
    const localSave = identical ? await storage.getSaveMeta(identical.id) : null;
    this.transferOffer = {
      offer,
      identical: identical ? { id: identical.id, name: identical.name } : null,
      candidates: candidates.map((c) => ({ id: c.id, name: c.name })),
      localSave: localSave ? { updatedAt: localSave.updatedAt } : null,
    };
    this.transferDialog.showOffer(this.transferOffer);
  }

  /** Everything arrived and checked out: into the library as chosen. */
  private async storeReceived(received: Received) {
    const view = this.transferOffer;
    const choice = this.transferChoice;
    if (!view || !choice) return;
    const { offer } = view;
    let targetId = view.identical?.id ?? null;
    try {
      if (received.rom && choice.rom !== "skip") {
        const extension = { gba: ".gba", gbc: ".gbc", nds: ".nds" }[offer.rom.system];
        const entry = await storage.addRom(new File([received.rom], `${offer.rom.name}${extension}`));
        if (entry.id !== offer.rom.id) {
          await storage.deleteRom(entry.id);
          throw new Error("The ROM arrived damaged");
        }
        if (typeof choice.rom === "object") await storage.replaceRom(choice.rom.update, entry.id);
        targetId = entry.id;
      }
      const gotSave = received.save !== null && choice.save && targetId !== null;
      if (gotSave) await storage.putSave(targetId!, received.save!);
      const now = Date.now();
      if (targetId) await storage.updateRom(targetId, { transferredAt: now, ...(gotSave ? { savedAt: now } : {}) });
      this.transferGuest = null;
      const got = [received.rom && choice.rom !== "skip" ? "the ROM" : null, gotSave ? "the save" : null].filter(Boolean);
      this.transferDialog.finish(got.length ? `Received ${got.join(" and ")}.` : "Nothing was taken.");
      await this.library.refresh();
    } catch (error) {
      this.transferDialog.fail(error instanceof Error ? error.message : String(error));
    }
  }

  /** What goes in a DS game's GBA slot, from its library entry (the Rumble Pak unless chosen otherwise). */
  private async slot2Load(entry: RomEntry): Promise<Slot2Load | undefined> {
    this.slot2RomId = null;
    if (entry.system !== "nds") return undefined;
    const choice = entry.slot2 ?? "rumble";
    if (choice === "empty") return { kind: "empty" };
    if (choice === "rumble") return { kind: "rumble" };
    const rom = await storage.getRomData(choice);
    if (!rom) return { kind: "empty" };
    this.slot2RomId = choice;
    return { kind: "cartridge", rom, save: await storage.getSave(choice) };
  }

  /** Lists the library's GBA games in the slot's select, with this game's choice selected. */
  private async refreshSlot2Options(entry: RomEntry) {
    if (entry.system !== "nds") return;
    const select = this.player.querySelector<HTMLSelectElement>("[data-slot2]")!;
    const games = (await storage.listRoms()).filter((r) => (r.system ?? "gba") === "gba");
    const option = (value: string, label: string) => Object.assign(document.createElement("option"), { value, textContent: label });
    select.replaceChildren(option("rumble", "Rumble Pak"), option("empty", "Nothing"), ...games.map((g) => option(g.id, g.name)));
    const choice = entry.slot2 ?? "rumble";
    select.value = choice === "rumble" || choice === "empty" || games.some((g) => g.id === choice) ? choice : "empty";
    const saveType = this.player.querySelector<HTMLSelectElement>("[data-save-type]")!;
    saveType.value = SAVE_TYPES.some(([v]) => v === entry.saveType) ? entry.saveType! : "auto";
  }

  /** The player named the card's save chip: remembered for this game, and used now (the save moves into it). */
  private async changeSaveType(name: string) {
    const entry = this.current;
    if (!entry || entry.system !== "nds") return;
    entry.saveType = name;
    await storage.updateRom(entry.id, { saveType: name });
    const label = SAVE_TYPES.find(([v]) => v === name)?.[1] ?? name;
    if (name === "auto") {
      this.toast("Pipit will tell the save chip from the game's first write when it next starts");
      return;
    }
    if (this.session) {
      this.toast("The save chip is chosen when the game starts; it stays as it is while playing together");
      return;
    }
    this.emulator.setSaveType(name);
    this.toast(`Save chip: ${label}; the game's save was moved into it`);
  }

  /** Opens the console settings with what the stored firmware image holds (the worker reads it). */
  private async openDsSettings() {
    this.pauseForDialog(true);
    this.emulator.requestFirmwareUser(await storage.getFile("ds-firmware"));
  }

  private async showDsSettings(user: DsUser) {
    const image = await storage.getFile("ds-firmware");
    const note = image ? `Kept in your firmware image (${Math.round(image.byteLength / 1024)} KB).` : "Kept in Pipit's own firmware image.";
    this.dsSettings.open(user, note);
  }

  private async saveDsSettings(user: DsUser) {
    this.dsSettingsPending = true;
    this.emulator.setFirmwareUser(await storage.getFile("ds-firmware"), user);
  }

  /** A firmware dump from the player's own DS takes the place of Pipit's image. */
  private async importFirmware() {
    const data = await pickFile(".bin,.fw");
    if (!data) return;
    if (data.byteLength !== 0x20000 && data.byteLength !== 0x40000 && data.byteLength !== 0x80000) {
      this.toast("That is not a DS firmware image (128, 256 or 512 KB)");
      return;
    }
    await storage.putFile("ds-firmware", data);
    this.toast("Firmware imported; it is used when a DS game starts");
    this.emulator.requestFirmwareUser(data);
  }

  private async resetFirmware() {
    await storage.deleteFile("ds-firmware");
    this.toast("Back to Pipit's own firmware");
    this.emulator.requestFirmwareUser(null);
  }

  /** The player changed what is in the GBA slot: remembered for this game, and inserted now. */
  private async changeSlot2(choice: string) {
    const entry = this.current;
    if (!entry || entry.system !== "nds") return;
    entry.slot2 = choice;
    await storage.updateRom(entry.id, { slot2: choice });
    if (this.session) {
      this.toast("The GBA slot stays empty while playing together");
      return;
    }
    const load = await this.slot2Load(entry);
    if (!load) return;
    this.emulator.setSlot2(load);
    const name = this.player.querySelector<HTMLSelectElement>("[data-slot2]")!.selectedOptions[0]?.textContent ?? "The cartridge";
    this.toast(load.kind === "empty" ? "GBA slot emptied" : load.kind === "rumble" ? "Rumble Pak inserted" : `${name} inserted in the GBA slot`);
  }

  private async play(entry: RomEntry) {
    const rom = await storage.getRomData(entry.id);
    if (!rom) return;
    const save = await storage.getSave(entry.id);
    this.current = entry;
    this.paused = false;
    this.player.querySelector(".menu-title")!.textContent = entry.name;
    this.setMenuOpen(false);
    this.library.element.classList.add("hidden");
    this.player.classList.remove("hidden");
    this.input.setKeyboardActive(true);
    this.applyLayout();
    this.screen.fit();
    this.audio.clear();
    this.player.dataset["system"] = entry.system ?? "gba";
    const slot2 = await this.slot2Load(entry);
    const firmware = entry.system === "nds" ? await storage.getFile("ds-firmware") : null;
    this.emulator.load(rom, save, null, entry.system ?? "gba", undefined, undefined, slot2, firmware, entry.saveType);
    await this.refreshSlot2Options(entry);
    await storage.touchRom(entry.id);
    await this.refreshStateSlots();
  }

  /** Marks the Save buttons of slots that already hold a state. */
  private async refreshStateSlots() {
    const saved = this.current ? await storage.listStates(this.current.id) : [];
    for (const button of this.player.querySelectorAll<HTMLElement>('[data-action="save-state"]')) {
      const entry = saved.find((s) => s.slot === Number(button.dataset["slot"]));
      button.classList.toggle("has-state", entry !== undefined);
      button.title = entry ? `Saved ${new Date(entry.savedAt).toLocaleString()} — click to overwrite` : "Empty slot";
    }
  }

  private backToLibrary() {
    if (this.session) {
      const session = this.session;
      this.session = null;
      session.leave();
    }
    this.clearLinkUi();
    this.emulator.requestSave();
    this.setRunning(false);
    this.flushSave();
    this.setMenuOpen(false);
    this.current = null;
    this.player.classList.add("hidden");
    this.library.element.classList.remove("hidden");
    this.input.setKeyboardActive(false);
    void this.library.refresh();
  }

  /** Starts or stops both the worker and the display loop that feeds it. */
  private setRunning(on: boolean) {
    if (on) {
      this.emulator.run();
      this.pacer.start();
    } else {
      this.pacer.stop();
      this.emulator.pause();
    }
  }

  private wireEmulator() {
    this.pacer.onFrame = () => this.onDisplayFrame();
    this.emulator.on("loaded", (_title, _code, width, height) => {
      this.screen.setSize(width, height);
      this.layoutEditor.setScreenAspect(width / height);
      this.setRunning(true);
    });
    this.emulator.on("hash", (frame, hash) => this.session?.reportHash(frame, hash));
    const fpsLabel = this.player.querySelector<HTMLElement>(".toolbar-fps")!;
    let lastFpsText = "";
    this.emulator.on("frame", (pixels, audio, fps, maxGapMs, sides, activity, rumble) => {
      this.logLinkStatus(fps, maxGapMs, activity);
      this.input.setRumble(rumble === true);
      this.screen.draw(pixels);
      this.emulator.returnFrame(pixels);
      for (const side of sides ?? []) {
        this.sideScreens.draw(side.player, side.pixels);
        this.emulator.returnFrame(side.pixels);
      }
      this.audio.push(new Int16Array(audio));
      const text = `${fps} fps`;
      if (text !== lastFpsText) {
        fpsLabel.textContent = text;
        lastFpsText = text;
      }
      // Worst frame-to-frame gap of the last second, for spotting stutter.
      fpsLabel.dataset["maxGap"] = String(maxGapMs);
      const pacing = this.pacer.locked ? `one frame every ${this.pacer.locked} refresh${this.pacer.locked > 1 ? "es" : ""}` : "frames timed by the clock";
      fpsLabel.title = `longest pause between frames in the last second: ${maxGapMs} ms; display ${Math.round(this.pacer.refreshHz)} Hz, ${pacing}`;
    });
    this.emulator.on("slot2Save", (data) => {
      if (this.slot2RomId) void storage.putSave(this.slot2RomId, data);
    });
    // The DS firmware: kept whenever the console writes to it, or the player
    // changed the console settings.
    this.emulator.on("firmware", (data) => {
      void storage.putFile("ds-firmware", data).then(() => {
        if (!this.dsSettingsPending) return;
        this.dsSettingsPending = false;
        this.toast("Console settings saved; they take effect when a DS game starts");
      });
    });
    this.emulator.on("firmwareUser", (user) => void this.showDsSettings(user));
    this.emulator.on("save", (data) => {
      this.latestSave = data;
      for (const resolve of this.saveWaiters.splice(0)) resolve(data.slice(0));
      if (this.saveTimer !== null) clearTimeout(this.saveTimer);
      this.saveTimer = window.setTimeout(() => this.flushSave(), 500);
    });
    this.emulator.on("state", async (slot, data) => {
      if (!this.current) return;
      await storage.putState(this.current.id, slot, data);
      this.toast(`State ${slot} saved`);
      await this.refreshStateSlots();
    });
    this.emulator.on("error", (message) => this.toast(message));
  }

  // Controller toggle: detection drives the button; the button drives the input.
  /** Whether a controller was present at the last detection, for the connect/disconnect notices. */
  private padConnected = false;
  private wireController() {
    this.input.onGamepads = (state) => {
      this.controller.update(state);
      this.input.setMapping(this.mappingFor(this.input.activeGamepadId()));
      // Only a change is worth a notice; switching input on or off is not one.
      const connected = state.active !== null;
      if (connected !== this.padConnected) {
        this.toast(connected ? "Controller connected" : "Controller disconnected", { brief: true });
      }
      this.padConnected = connected;
    };
    this.controller.onToggle = (enabled) => this.input.setGamepadEnabled(enabled);
    this.controller.onSelect = (index) => this.input.setActiveGamepad(index);
    this.controller.onNothingDetected = () =>
      this.toast("No controller detected — press a button on it to wake it up");
    this.controller.onSettings = () => {
      const id = this.input.activeGamepadId();
      const name = id ? id.replace(/\s*\(.*$/, "") : "No controller connected";
      this.pauseForDialog(true);
      this.controllerSettings.open(name, this.mappingFor(id));
    };
    this.controllerSettings.captureButton = () => this.input.captureButton();
    this.controllerSettings.onChange = (mapping) => {
      const id = this.input.activeGamepadId();
      if (id) {
        this.settings = {
          ...this.settings,
          controllerMappings: { ...this.settings.controllerMappings, [id]: mapping },
        };
        void storage.saveSettings(this.settings);
      }
      this.input.setMapping(mapping);
    };
    this.controllerSettings.onClose = () => this.pauseForDialog(false);
    const initial = this.input.gamepadState();
    this.padConnected = initial.active !== null;
    this.controller.update(initial);
  }

  private mappingFor(id: string | null): ControllerMapping {
    return (id && this.settings.controllerMappings[id]) || DEFAULT_MAPPING;
  }

  private wireKeyboard() {
    this.keyboardSettings.captureKey = () => this.input.captureKey();
    this.keyboardSettings.onChange = (mapping) => {
      this.settings = { ...this.settings, keyboardMapping: mapping };
      void storage.saveSettings(this.settings);
      this.applyKeyboard();
    };
    this.keyboardSettings.onClose = () => this.pauseForDialog(false);
  }

  /** Pushes the key bindings to the input layer and refreshes the texts that cite them. */
  private applyKeyboard() {
    const m = this.settings.keyboardMapping;
    this.input.setKeyboardMapping(m);
    const k = (code: string) => keyLabel(code);
    this.library.setKeys([
      { action: "D-pad", keys: [k(m.Up), k(m.Down), k(m.Left), k(m.Right)] },
      { action: "A", keys: [k(m.A)] },
      { action: "B", keys: [k(m.B)] },
      { action: "Start", keys: [k(m.Start)] },
      { action: "Select", keys: [k(m.Select)] },
      { action: "L / R", keys: [k(m.L), k(m.R)] },
      { action: "Fast-forward", keys: [k(m.FastForward)], note: "hold" },
      { action: "Pause", keys: [k(m.Pause)] },
    ]);
  }

  /** Dialogs pause the game, unless the player had paused it already. */
  private dialogPaused = false;
  private pauseForDialog(open: boolean) {
    if (!this.current) return;
    if (open && !this.paused) {
      this.setRunning(false);
      this.dialogPaused = true;
    } else if (!open && this.dialogPaused) {
      this.setRunning(true);
      this.dialogPaused = false;
    }
  }

  private flushSave() {
    if (this.saveTimer !== null) clearTimeout(this.saveTimer);
    this.saveTimer = null;
    if (this.current && this.latestSave) void storage.putSave(this.current.id, this.latestSave);
  }

  private async loadState(slot: number) {
    if (!this.current) return;
    const entry = await storage.getState(this.current.id, slot);
    if (!entry) {
      this.toast(`State ${slot} is empty`);
      return;
    }
    // The buffer is transferred to the worker, so hand over a copy.
    this.emulator.loadState(entry.data.slice(0));
    this.toast(`State ${slot} loaded`);
  }

  // ---------------------------------------------------------------------------
  // Play together (see platform/netplay.ts)
  // ---------------------------------------------------------------------------

  private onDisplayFrame() {
    if (!this.session) {
      this.emulator.requestFrame();
      return;
    }
    // Scripted keys (a test hook) are given per game frame; otherwise whatever
    // is held now goes out for every frame published.
    const script = this.keyScript;
    const tick = this.session.tick(script ? (frame) => script.at(frame) : () => this.frameKeys());
    if (tick) {
      this.stalledTicks = 0;
      this.emulator.requestFrame(tick.keys, tick.frame, tick.guessed);
      // For diagnostics (the smoke test reads them): how far the session has run, and how.
      this.player.dataset["linkFrame"] = String(this.session.frame);
      this.player.dataset["linkDelay"] = String(this.session.delayFrames);
      this.player.dataset["linkRollbacks"] = String(this.session.rollbacks);
    } else {
      this.stalledTicks++;
    }
    // A hiccup of a few frames is invisible; a longer wait gets a notice.
    this.player.classList.toggle("waiting", this.stalledTicks > 15);
  }

  private wireLink() {
    this.player.querySelector<HTMLSelectElement>("[data-view]")!.addEventListener("change", (e) => {
      const player = Number((e.target as HTMLSelectElement).value);
      this.emulator.setView(player);
      this.sideScreens.setPlayers(this.sessionPlayers, player);
      this.toast(player === (this.session?.local ?? 0) ? "Back to your screen" : `Watching player ${player + 1}`, { brief: true });
    });
    this.player.querySelector<HTMLSelectElement>("[data-slot2]")!.addEventListener("change", (e) => {
      void this.changeSlot2((e.target as HTMLSelectElement).value);
    });
    this.player.querySelector<HTMLSelectElement>("[data-save-type]")!.addEventListener("change", (e) => {
      void this.changeSaveType((e.target as HTMLSelectElement).value);
    });
    this.dsSettings.onClose = () => this.pauseForDialog(false);
    this.dsSettings.onSave = (user) => void this.saveDsSettings(user);
    this.dsSettings.onImport = () => void this.importFirmware();
    this.dsSettings.onReset = () => void this.resetFirmware();
    this.linkDialog.onHost = () => void this.startSession("host", generateCode());
    this.linkDialog.onJoin = (code) => void this.startSession("guest", code);
    this.linkDialog.onCopyLog = () => void this.copyLinkLog();
    this.linkDialog.onStart = () => {
      if (!this.session) return;
      this.session.wireless = this.linkDialog.wireless;
      this.session.start();
    };
    this.linkDialog.onSettings = (connection) => {
      this.settings = { ...this.settings, connection };
      void storage.saveSettings(this.settings);
    };
    this.linkDialog.onCancel = () => {
      // Cancelling before the game started drops the session.
      this.pendingLink?.cancel();
      this.pendingLink = null;
      if (this.session && !this.player.classList.contains("linked")) {
        const session = this.session;
        this.session = null;
        session.leave();
      }
      this.pauseForDialog(false);
    };
  }

  private openLinkDialog() {
    if (!this.current || this.session) return;
    this.pauseForDialog(true);
    // Only the Advance has a choice of cable or adapters: DSs are wireless.
    this.linkDialog.open(this.settings.connection, this.current.system === "gba");
  }

  /** The keys held now; on a DS, with the pen packed in (see `PEN_INCLUDED`). */
  private frameKeys(): number {
    if (this.current?.system !== "nds") return this.heldKeys;
    const pen = this.pen;
    return this.heldKeys | PEN_INCLUDED | (pen ? PEN_DOWN | (pen.x << 12) | (pen.y << 20) : 0);
  }

  /** Moves the pen (or lifts it, with a negative coordinate): now when playing alone, with the next frame's keys on a link. */
  private setPen(x: number, y: number) {
    this.pen = x < 0 || y < 0 ? null : { x, y };
    this.emulator.setTouch(x, y);
  }

  private async startSession(role: Role, code: string) {
    if (!this.current || this.session || this.pendingLink) return;
    const entry = this.current;
    this.linkDialog.setBusy(true);
    this.linkDialog.setStatus(role === "host" ? "Setting up…" : "Joining…");
    this.linkDialog.showCode(role === "host" ? code : null);
    this.flushSave();
    const [rom, save] = await Promise.all([storage.getRomData(entry.id), storage.getSave(entry.id)]);
    if (!rom) return;
    const romHash = await hashRom(rom);
    const local = new URLSearchParams(location.search).get("link") === "local";
    linkLog(`${role} with code ${code} (${local ? "tabs of this browser" : "online"}), ${(entry.system ?? "gba").toUpperCase()} ROM ${romHash.slice(0, 8)}`);
    let session: LinkSession;
    if (role === "host") {
      const listener = await this.settle(listen(code, local, this.settings.connection), entry);
      if (!listener) return;
      session = LinkSession.host(listener, romHash, save, entry.system === "gbc" ? 2 : undefined);
    } else {
      const transport = await this.settle(connect(code, local, this.settings.connection), entry);
      if (!transport) return;
      session = LinkSession.join(transport, romHash, save);
    }
    this.session = session;
    session.onStatus = (text) => this.linkDialog.setStatus(text);
    session.onLobby = (players) => this.linkDialog.setPlayers(players);
    session.onPing = (ms) => this.showPing(ms);
    session.onStart = (start) => this.startLinkedGame(rom, start);
    session.onEnd = (reason) => this.endSession(session, reason);
    session.onRollback = (toFrame, inputs, snapshots) => this.emulator.rollback(toFrame, inputs, snapshots);
    session.onConfirm = (frame) => this.emulator.confirm(frame);
    if (role === "host") this.linkDialog.setPlayers(1);
  }

  /** Waits for a connection step; null when it failed (the dialog says why) or was overtaken. */
  private async settle<T extends { close(): void }>(pending: Pending<T>, entry: RomEntry): Promise<T | null> {
    this.pendingLink = pending;
    let ready: T;
    try {
      ready = await pending.ready;
    } catch (error) {
      if (this.pendingLink === pending) this.linkFailed(error instanceof Error ? error.message : String(error));
      return null;
    }
    if (this.pendingLink !== pending || this.current !== entry || this.session) {
      ready.close();
      return null;
    }
    this.pendingLink = null;
    return ready;
  }

  private sessionPlayers = 0;
  private sessionWireless = false;
  private lastPing = 0;
  /** Frames shown since the last status line in the session log. */
  private statusFrames = 0;
  private lastActivity = 0;

  /** Once a second while linked: where the session stands, in the session log. */
  private logLinkStatus(fps: number, maxGapMs: number, activity: number | undefined) {
    if (!this.session || !this.player.classList.contains("linked")) return;
    if (++this.statusFrames < 60) return;
    this.statusFrames = 0;
    const perSecond = activity === undefined ? "?" : String(activity - this.lastActivity);
    this.lastActivity = activity ?? 0;
    this.lastRate = perSecond;
    this.lastFps = fps;
    this.lastGap = maxGapMs;
    this.refreshLinkStats();
    linkLog(
      `frame ${this.session.frame} · delay ${this.session.delayFrames} · ping ${Math.round(this.lastPing)} ms · rollbacks ${this.session.rollbacks} · ${fps} fps, worst gap ${maxGapMs} ms · in-game link activity ${perSecond}/s`,
    );
  }

  /** Puts the session log on the clipboard (or downloads it), for bug reports. */
  private async copyLinkLog() {
    const text = linkLogText() || "(nothing yet: no session in this page)";
    try {
      await navigator.clipboard.writeText(text);
      this.toast("Session log copied");
    } catch {
      const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = "pipit-session.log";
      a.click();
      URL.revokeObjectURL(url);
    }
  }

  private sessionLocal = 0;
  private lastFps = 0;
  private lastGap = 0;
  private lastRate = "?";

  private linkName(): string {
    if (this.current?.system === "nds") return "Local wireless";
    return this.sessionWireless ? "Wireless Adapters" : "Link cable";
  }

  /** The pill (round trip / delay) and the details panel, from the latest numbers. */
  private refreshLinkStats() {
    const session = this.session;
    if (!session) return;
    const ping = Math.round(this.lastPing);
    const delay = session.delayFrames;
    this.player.querySelector(".link-pill-ping")!.textContent = String(ping);
    this.player.querySelector(".link-pill-delay")!.textContent = String(delay);
    this.player.querySelector<HTMLElement>(".link-pill")!.title =
      `${ping} ms round trip, keys land ${delay} frames later — click for the session's details`;
    const set = (stat: string, text: string) => {
      this.player.querySelector(`[data-stat="${stat}"]`)!.textContent = text;
    };
    set("players", this.sessionPlayers ? `${this.sessionPlayers} players, you are player ${this.sessionLocal + 1}` : "—");
    set("link", this.linkName());
    set("ping", `${ping} ms`);
    set("delay", `${delay} frames (${Math.round((delay * 1000) / 60)} ms)`);
    set("rollbacks", String(session.rollbacks));
    set("frame", String(session.frame));
    set("fps", this.lastFps ? `${this.lastFps} fps, worst gap ${this.lastGap} ms` : "—");
    set("activity", `${this.lastRate}/s ${this.sessionWireless ? "adapter wake-ups" : "cable transfers"}`);
  }

  /** Shows or hides the details panel in the corner of the screen. */
  private toggleLinkStats(open?: boolean) {
    const panel = this.player.querySelector(".link-stats")!;
    const show = open ?? panel.classList.contains("hidden");
    panel.classList.toggle("hidden", !show);
    this.player.querySelector(".link-pill")!.setAttribute("aria-pressed", String(show));
    if (show) this.refreshLinkStats();
  }

  private showPing(ms: number) {
    this.lastPing = ms;
    if (!this.session) return;
    this.refreshLinkStats();
    if (!this.player.classList.contains("linked")) this.linkDialog.setStatus(`Connected, ${Math.round(ms)} ms round trip`);
  }

  /** Both saves are in hand: restart the game as two linked consoles. */
  private startLinkedGame(rom: ArrayBuffer, start: SessionStart) {
    this.keyScript = this.hooks.keyScript ? new ScriptedKeys(this.hooks.keyScript) : null;
    this.linkDialog.close();
    this.dialogPaused = false;
    this.paused = false;
    this.player.querySelector('[data-action="pause"]')!.classList.remove("active");
    this.player.classList.add("linked");
    this.player.querySelector('[data-action="link"]')!.classList.add("hidden");
    this.player.querySelector('[data-action="leave-link"]')!.classList.remove("hidden");
    this.player.querySelector('.menu [data-action="copy-link-log"]')!.classList.remove("hidden");
    this.audio.clear();
    this.stalledTicks = 0;
    // The worker reports `loaded`, which starts the display loop. The GBA
    // slot stays empty: the other players would need the same cartridge.
    this.slot2RomId = null;
    this.emulator.load(rom, null, null, this.current?.system ?? "gba", start.link, start.epoch);
    this.sessionPlayers = start.link.players;
    this.sessionWireless = start.link.wireless === true;
    this.sessionLocal = start.link.local;
    this.refreshLinkStats();
    // Watching: any console of the link can be shown; keys still go to your own.
    const view = this.player.querySelector<HTMLSelectElement>("[data-view]")!;
    view.replaceChildren(
      ...Array.from({ length: start.link.players }, (_, p) => {
        const option = document.createElement("option");
        option.value = String(p);
        option.textContent = p === start.link.local ? "Your screen" : `Player ${p + 1}`;
        return option;
      }),
    );
    view.value = String(start.link.local);
    view.closest(".link-view")!.classList.remove("hidden");
    this.sideScreens.setPlayers(start.link.players, start.link.local);
    this.applySideScreens();
    this.toast(`Linked: ${start.link.players} players, you are player ${start.link.local + 1}; keys land ${start.delay} frames later`);
  }

  private endSession(session: LinkSession, reason: string) {
    if (this.session !== session) return;
    this.session = null;
    const linked = this.player.classList.contains("linked");
    if (linked) {
      // The two games cannot go on alone from here; the save is kept.
      this.clearLinkUi();
      this.backToLibrary();
      this.toast(`Session over: ${reason}`);
    } else {
      this.linkDialog.setBusy(false);
      this.linkDialog.showCode(null);
      this.linkDialog.setPlayers(0);
      this.linkDialog.setStatus(reason);
    }
  }

  /** The save as the game last wrote it (asks the worker for a fresh copy). */
  private copyOfSave(): Promise<ArrayBuffer> {
    return new Promise((resolve) => {
      this.saveWaiters.push(resolve);
      this.emulator.requestSave();
    });
  }

  /** In a running session, leaving ends it for everyone: ask first. */
  private async leaveIfAgreed(leave: () => void) {
    if (this.session && this.player.classList.contains("linked")) {
      const hosting = this.session.role === "host";
      const agreed = await this.confirmDialog.ask(
        "Leave the session?",
        hosting
          ? "You are hosting: leaving ends the game for every player."
          : "Leaving ends the game for every player, not just you.",
        { confirm: "Leave", cancel: "Stay" },
      );
      if (!agreed) return;
    }
    leave();
  }

  private linkFailed(message: string) {
    this.pendingLink = null;
    this.linkDialog.setBusy(false);
    this.linkDialog.showCode(null);
    this.linkDialog.setStatus(message);
  }

  private clearLinkUi() {
    this.pendingLink?.cancel();
    this.pendingLink = null;
    this.sessionPlayers = 0;
    this.linkDialog.close();
    this.player.classList.remove("linked", "waiting");
    this.player.querySelector('[data-action="link"]')!.classList.remove("hidden");
    this.player.querySelector('[data-action="leave-link"]')!.classList.add("hidden");
    this.player.querySelector('.menu [data-action="copy-link-log"]')!.classList.add("hidden");
    this.toggleLinkStats(false);
    this.player.querySelector(".link-view")!.classList.add("hidden");
    this.sideScreens.clear();
    this.emulator.setSideScreens(false);
    this.screen.fit();
  }

  /** Shows a notice card; a `brief` one (controller events) stays about a second. */
  private toast(message: string, options: { brief?: boolean } = {}) {
    // One notice at a time: a new one replaces whatever is still showing.
    for (const old of document.querySelectorAll(".toast")) old.remove();
    const el = document.createElement("div");
    el.className = options.brief ? "toast brief" : "toast";
    el.textContent = message;
    document.body.append(el);
    // The card fades in, holds, then slides off the screen; it goes once the slide ends.
    el.addEventListener("animationend", (e) => {
      if (e.animationName === "toast-out") el.remove();
    });
  }

  private setFastForward(on: boolean) {
    // Fast-forwarding alone would leave a link partner behind.
    if (this.session) return;
    this.emulator.setFastForward(on);
    this.player.querySelector('[data-action="fast"]')!.classList.toggle("active", on);
  }

  private togglePause() {
    this.paused = !this.paused;
    this.setRunning(!this.paused);
    this.player.querySelector('[data-action="pause"]')!.classList.toggle("active", this.paused);
  }

  /**
   * On a DS game the bottom half of the picture is the touch screen: a
   * pointer pressed on it is the pen, in that screen's pixel coordinates.
   */
  private wireScreenPen() {
    const canvas = this.screen.element;
    let penPointer: number | null = null;
    const pen = (e: PointerEvent) => {
      const rect = canvas.getBoundingClientRect();
      const x = Math.floor(((e.clientX - rect.left) / rect.width) * canvas.width);
      const y = Math.floor(((e.clientY - rect.top) / rect.height) * canvas.height) - canvas.height / 2;
      if (x < 0 || x >= canvas.width || y < 0 || y >= canvas.height / 2) this.setPen(-1, -1);
      else this.setPen(x, y);
    };
    const lift = (e: PointerEvent) => {
      if (e.pointerId !== penPointer) return;
      penPointer = null;
      this.setPen(-1, -1);
    };
    canvas.addEventListener("pointerdown", (e) => {
      if (this.current?.system !== "nds" || penPointer !== null) return;
      penPointer = e.pointerId;
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        // Capture is a nicety: the pen still follows the pointer while it is over the screen.
      }
      pen(e);
      e.preventDefault();
    });
    canvas.addEventListener("pointermove", (e) => {
      if (e.pointerId === penPointer) pen(e);
    });
    canvas.addEventListener("pointerup", lift);
    canvas.addEventListener("pointercancel", lift);
  }

  /** The player minus the toolbar: the screen and the touch controls. */
  private get stage(): HTMLElement {
    return this.player.querySelector<HTMLElement>(".stage")!;
  }

  private wireLayoutEditor() {
    this.layoutEditor.onApplied = () => this.screen.fit();
    this.layoutEditor.onDone = (layout) => {
      this.settings = {
        ...this.settings,
        touchLayouts: { ...this.settings.touchLayouts, [this.resolvedLayout]: layout },
      };
      void storage.saveSettings(this.settings);
      this.endLayoutEdit();
      this.toast("Touch layout saved");
    };
    this.layoutEditor.onCancel = () => this.endLayoutEdit();
    this.layoutEditor.onReset = () => {
      const touchLayouts = { ...this.settings.touchLayouts };
      delete touchLayouts[this.resolvedLayout];
      this.settings = { ...this.settings, touchLayouts };
      void storage.saveSettings(this.settings);
      this.endLayoutEdit();
      this.toast("Default layout restored");
    };
  }

  private openLayoutEditor() {
    if (!this.player.classList.contains("touch-on")) {
      this.toast("Touch controls are hidden — turn on “Always show touch controls” first");
      return;
    }
    this.setMenuOpen(false);
    this.pauseForDialog(true);
    // Start from the saved layout, or from wherever the stock layout put things.
    const initial = this.settings.touchLayouts[this.resolvedLayout] ?? this.layoutEditor.measure();
    this.stage.classList.add("custom", "editing");
    this.layoutEditor.open(initial);
  }

  private endLayoutEdit() {
    this.stage.classList.remove("editing");
    this.pauseForDialog(false);
    this.applyLayout();
  }

  private setMenuOpen(open: boolean) {
    this.player.querySelector(".menu")!.classList.toggle("hidden", !open);
    this.player.querySelector(".menu-backdrop")!.classList.toggle("hidden", !open);
  }

  private isMenuOpen(): boolean {
    return !this.player.querySelector(".menu")!.classList.contains("hidden");
  }

  private wireToolbar() {
    this.player.addEventListener("click", async (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("[data-action]");
      if (!button || button.closest(".modal-backdrop")) return;
      const slot = Number(button.dataset["slot"] ?? 0);
      switch (button.dataset["action"]) {
        case "back":
          void this.leaveIfAgreed(() => this.backToLibrary());
          break;
        case "pause":
          this.togglePause();
          break;
        case "fast":
          this.setFastForward(!button.classList.contains("active"));
          break;
        case "shot":
          await takeScreenshot(this.screen, this.current?.name ?? "pipit");
          break;
        case "menu":
          this.setMenuOpen(!this.isMenuOpen());
          break;
        case "keyboard":
          this.setMenuOpen(false);
          this.pauseForDialog(true);
          this.keyboardSettings.open(this.settings.keyboardMapping);
          break;
        case "link":
          this.setMenuOpen(false);
          this.openLinkDialog();
          break;
        case "copy-link-log":
          void this.copyLinkLog();
          break;
        case "link-stats":
          this.toggleLinkStats();
          break;
        case "leave-link":
          this.setMenuOpen(false);
          void this.leaveIfAgreed(() => this.session?.leave());
          break;
        case "edit-layout":
          this.openLayoutEditor();
          break;
        case "save-state":
          this.emulator.saveState(slot);
          break;
        case "load-state":
          await this.loadState(slot);
          break;
        case "ds-settings":
          await this.openDsSettings();
          break;
        case "export":
          this.emulator.requestSave();
          setTimeout(() => {
            if (this.latestSave && this.current) exportSave(this.latestSave, this.current.name);
          }, 100);
          break;
        case "import": {
          const data = await pickSaveFile();
          if (data && this.current) {
            await storage.putSave(this.current.id, data);
            await this.play(this.current);
          }
          break;
        }
        case "fullscreen":
          if (document.fullscreenElement) await document.exitFullscreen();
          else await this.player.requestFullscreen().catch(() => {});
          break;
      }
    });
    window.addEventListener("keydown", (e) => {
      if (!this.current) return;
      if (e.code === "Escape") this.setMenuOpen(false);
      const fkey = /^F([1-9])$/.exec(e.code);
      if (fkey) {
        const slot = Number(fkey[1]);
        if (slot <= STATE_SLOTS) {
          e.preventDefault();
          if (e.shiftKey) this.emulator.saveState(slot);
          else void this.loadState(slot);
        }
      }
    });
  }

  private wireSettings() {
    for (const el of this.player.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      const handler = () => {
        const value =
          el instanceof HTMLSelectElement ? el.value
          : el.type === "checkbox" ? el.checked
          : Number(el.value);
        this.settings = { ...this.settings, [key]: value };
        this.applySettings();
        void storage.saveSettings(this.settings);
      };
      el.addEventListener(el instanceof HTMLSelectElement ? "change" : "input", handler);
    }
  }

  private applySettings() {
    for (const el of this.player.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      const value = this.settings[key];
      if (el instanceof HTMLSelectElement) el.value = String(value);
      else if (el.type === "checkbox") el.checked = Boolean(value);
      else el.value = String(value);
    }
    const lcd = this.settings.colorCorrection === "gba";
    this.emulator.setColorCorrection(lcd ? 1 : 0, this.settings.colorStrength / 100);
    this.player.querySelector(".strength-label")!.textContent = `${this.settings.colorStrength}%`;
    this.player.querySelector(".color-strength")!.classList.toggle("hidden", !lcd);
    this.audio.setVolume(this.settings.volume);
    this.screen.setIntegerScale(this.settings.integerScale);
    this.applyKeyboard();
    this.applyLayout();
    this.applySideScreens();
  }

  /** Partners' screens beside the player's own: when playing together and the option is on. */
  private applySideScreens() {
    const on = this.settings.sideScreens && this.player.classList.contains("linked");
    this.sideScreens.show(on);
    this.emulator.setSideScreens(on);
    this.screen.fit();
  }

  /** Resolves the touch layout for the current orientation and shows the controls. */
  private applyLayout() {
    const landscape = matchMedia("(orientation: landscape)").matches;
    const coarse = matchMedia("(pointer: coarse)").matches;
    const chosen: TouchLayout = this.settings.touchLayout;
    const layout = chosen === "auto" ? (landscape ? "gba" : "gbasp") : chosen;
    const touchOn = coarse || this.settings.alwaysShowTouch;
    this.player.dataset["layout"] = layout;
    this.player.classList.toggle("touch-on", touchOn);

    // Rotating the device while editing ends the edit: the layouts are per orientation.
    if (this.layoutEditor.isOpen && layout !== this.resolvedLayout) {
      this.layoutEditor.close();
      this.stage.classList.remove("editing");
      this.pauseForDialog(false);
    }
    this.resolvedLayout = layout;

    // Landscape touch layout: no toolbar; the actions live in a drawer opened
    // from the menu disc at the top-right. Everywhere else they stay in the toolbar.
    const drawer = touchOn && layout === "gba";
    this.player.classList.toggle("drawer-mode", drawer);
    document.body.dataset["chrome"] = drawer ? "drawer" : "toolbar";
    const actions = this.player.querySelector(".actions")!;
    const slot = this.player.querySelector(drawer ? ".menu-actions" : ".toolbar")!;
    if (actions.parentElement !== slot) {
      slot.append(actions);
      this.setMenuOpen(false);
    }

    if (this.layoutEditor.isOpen) {
      // Mid-edit (e.g. a resize): keep the working layout on screen.
      this.layoutEditor.reapply();
      return;
    }
    const custom = touchOn ? this.settings.touchLayouts[layout] : undefined;
    const stage = this.stage;
    if (custom) {
      stage.classList.add("custom");
      this.layoutEditor.apply(custom);
    } else {
      stage.classList.remove("custom");
      this.layoutEditor.clear();
      this.screen.fit();
      // Measure after the layout has applied.
      requestAnimationFrame(() => {
        this.touch.fit(layout, stage);
        this.screen.fit();
      });
    }
  }
}
