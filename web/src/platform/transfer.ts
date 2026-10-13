// Sending a game to another device: a ROM and its save go straight from one
// browser to the other over the connection "Play together" uses, with an
// eight-character code (or a QR code of it) to find each other.
//
// The introduction server only brokers the connection and never sees the
// bytes, and WebRTC encrypts the link itself. On top of that the two devices
// agree a key of their own (an ECDH exchange bound to the code) and show the
// same three words derived from it: a server that tried to sit in the middle
// would make the words differ. Nothing leaves the sender until its user
// accepts the receiver, and nothing lands in the receiver's library until its
// user chooses what to take.

import type { ConnectionSettings, RomEntry, System } from "../types";
import type { LinkMessage, Listener, Transport } from "./netplay";
import { connect, hashRom, listen } from "./netplay";

/** Codes are typed rarely (the QR code carries them), so they are longer than game codes. */
const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export function generateTransferCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (b) => ALPHABET[b % 32]).join("");
}

/** Transfers share the connection code space with game sessions: a prefix keeps them apart. */
const codeFor = (code: string) => `xfer-${code}`;

export type Item = "rom" | "save";

export interface RomOffer {
  id: string;
  name: string;
  system: System;
  title: string;
  gameCode: string;
  size: number;
}

export interface SaveOffer {
  size: number;
  updatedAt: number;
}

/** The game on offer (always named, so the receiver can place a save), with or without its bytes. */
export interface Offer {
  rom: RomOffer;
  romData: boolean;
  save: SaveOffer | null;
}

/** What the sender has to give: the game, its bytes if they are to go, its save if it is. */
export interface Items {
  entry: RomEntry;
  rom: ArrayBuffer | null;
  save: { data: ArrayBuffer; updatedAt: number } | null;
}

export interface Received {
  rom: ArrayBuffer | null;
  save: ArrayBuffer | null;
}

/** What goes over the wire; the hello is the only message with content in the clear. */
type Wire =
  | { type: "hello"; v: number; pub: ArrayBuffer }
  | { type: "enc"; data: ArrayBuffer }
  | { type: "chunk"; item: Item; index: number; data: ArrayBuffer }
  | { type: "ack"; item: Item; index: number }
  | { type: "cancel"; reason: string };

/** The messages inside `enc`. */
type Control =
  | { kind: "offer"; offer: Offer }
  | { kind: "request"; rom: boolean; save: boolean }
  | { kind: "begin"; item: Item; size: number; chunks: number; sha256: string }
  | { kind: "end"; item: Item }
  | { kind: "done" }
  | { kind: "finished" };

const VERSION = 1;
export const CHUNK = 48 * 1024;
/** Chunks in flight before the sender waits for an acknowledgement. */
const WINDOW = 24;
const ACK_EVERY = 4;
/** A sender waits this long for a receiver before giving up. */
const HOST_TIMEOUT_MS = 10 * 60 * 1000;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** The words both devices show, picked by bytes of the agreed key. */
const WORDS =
  "acorn amber anchor apple arrow aspen badge bagel bamboo banjo basil beach beacon berry birch bison blade blossom bridge brook bubble butter cabin cactus camel candle canoe canyon carrot castle cedar cello cherry cider cliff cloud clover cobalt cocoa comet copper coral cotton crane crystal daisy dawn delta denim desert dolphin dune eagle ember engine falcon feather fern fiddle flame flint forest fossil frost garden garnet ginger glacier glove goose grape gravel guitar harbor harp hazel heron honey horizon iceberg indigo iris island ivory jade jasmine jelly jigsaw juniper kayak kettle kiwi koala lagoon lantern laser lemon lilac lime linen lion lizard lobster lotus magnet mango maple marble meadow melon meteor mint mirror moose moss mountain muffin nectar needle noodle nutmeg oasis ocean olive onion orbit orchid otter oyster paddle panda papaya parrot pastel peach pearl pebble pepper piano pickle pillow pine planet plum pocket poppy prism pumpkin puzzle quartz quill rabbit radar rain raven ribbon river robin rocket rose ruby saddle saffron salmon sandal sapphire scarf seal shadow shell silver sketch sled slate smoke snail socket sparrow spruce squid stable stone storm sugar sunset swan tango teapot thistle thunder tiger timber toffee topaz tulip tundra turtle umbrella valley velvet violet walnut walrus wasabi willow window winter wolf yarn yogurt zebra zephyr zinc zipper".split(
    " ",
  );

// ---------------------------------------------------------------------------
// The key the two devices agree, and the envelope every message travels in
// ---------------------------------------------------------------------------

interface Secrets {
  key: CryptoKey;
  words: string;
}

async function makeKeys(): Promise<{ priv: CryptoKey; pub: ArrayBuffer }> {
  const pair = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
  return { priv: pair.privateKey, pub: await crypto.subtle.exportKey("raw", pair.publicKey) };
}

/** The key and the words from our private key, their public key and the code. */
async function agree(priv: CryptoKey, theirPub: ArrayBuffer, code: string): Promise<Secrets> {
  const theirs = await crypto.subtle.importKey("raw", theirPub, { name: "ECDH", namedCurve: "P-256" }, false, []);
  const shared = await crypto.subtle.deriveBits({ name: "ECDH", public: theirs }, priv, 256);
  const base = await crypto.subtle.importKey("raw", shared, "HKDF", false, ["deriveKey", "deriveBits"]);
  const salt = encoder.encode(`pipit-transfer-${VERSION}:${code}`);
  const key = await crypto.subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt, info: encoder.encode("data") },
    base,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  const bytes = new Uint8Array(
    await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt, info: encoder.encode("words") }, base, 32),
  );
  const words = Array.from(bytes.subarray(0, 3), (b) => WORDS[b % WORDS.length]).join(" ");
  return { key, words };
}

/**
 * Seals and opens messages with the agreed key. Every nonce is our direction
 * byte and a counter, so no nonce repeats and a message replayed or taken out
 * of order is refused.
 */
class Cipher {
  private sent = 0;
  private received = 0;

  constructor(
    private readonly key: CryptoKey,
    private readonly dir: number,
  ) {}

  async seal(plain: ArrayBuffer, aad: string): Promise<ArrayBuffer> {
    const iv = new Uint8Array(12);
    iv[0] = this.dir;
    new DataView(iv.buffer).setUint32(8, ++this.sent);
    const data = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: encoder.encode(aad) }, this.key, plain);
    const out = new Uint8Array(12 + data.byteLength);
    out.set(iv);
    out.set(new Uint8Array(data), 12);
    return out.buffer;
  }

  async open(sealed: ArrayBuffer, aad: string): Promise<ArrayBuffer> {
    const bytes = new Uint8Array(sealed);
    if (bytes.length < 12 + 16) throw new Error("A message arrived cut short");
    const iv = bytes.slice(0, 12);
    const counter = new DataView(iv.buffer).getUint32(8);
    if (iv[0] === this.dir || counter !== this.received + 1) throw new Error("A message arrived out of order");
    this.received = counter;
    return crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: encoder.encode(aad) }, this.key, bytes.slice(12));
  }
}

/**
 * Wire messages on a link transport, handled one at a time in order (the
 * handlers are asynchronous) and kept until a handler is set.
 */
class Pipe {
  private queue = Promise.resolve();
  private pending: Wire[] = [];
  private handler: ((message: Wire) => Promise<void>) | null = null;
  onClose: () => void = () => {};
  onError: (error: unknown) => void = () => {};

  constructor(private readonly transport: Transport) {
    transport.onMessage = (message) => this.push(message as unknown as Wire);
    transport.onClose = () => this.onClose();
  }

  set onMessage(handler: (message: Wire) => Promise<void>) {
    this.handler = handler;
    const kept = this.pending;
    this.pending = [];
    for (const message of kept) this.push(message);
  }

  private push(message: Wire) {
    const handler = this.handler;
    if (!handler) {
      this.pending.push(message);
      return;
    }
    this.queue = this.queue.then(() => handler(message)).catch((error) => this.onError(error));
  }

  send(message: Wire) {
    this.transport.send(message as unknown as LinkMessage);
  }

  close() {
    this.transport.close();
  }
}

const sizeText = (bytes: number) => (bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MB` : `${Math.ceil(bytes / 1024)} KB`);
export { sizeText };

// ---------------------------------------------------------------------------
// The sender
// ---------------------------------------------------------------------------

export class TransferHost {
  onStatus: (text: string) => void = () => {};
  /** The words to compare with the other device; `accept` or `reject` follows. */
  onVerify: (words: string) => void = () => {};
  onProgress: (item: Item, done: number, total: number) => void = () => {};
  onFinished: (sent: { rom: boolean; save: boolean }) => void = () => {};
  onError: (message: string) => void = () => {};

  private listener: Listener | null = null;
  private pipe: Pipe | null = null;
  private cipher: Cipher | null = null;
  private decided: ((ok: boolean) => void) | null = null;
  private readonly acked = new Map<Item, number>();
  private wake: (() => void) | null = null;
  private readonly sent = { rom: false, save: false };
  private timer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;

  private constructor(
    readonly code: string,
    private readonly items: Items,
  ) {}

  /** Listens under a fresh code; the host reports through its callbacks. */
  static async start(local: boolean, settings: ConnectionSettings, items: Items): Promise<TransferHost> {
    const host = new TransferHost(generateTransferCode(), items);
    host.listener = await listen(codeFor(host.code), local, settings).ready;
    host.listener.onGuest = (transport) => void host.greet(transport);
    host.timer = setTimeout(() => host.fail("Nobody connected; the code has expired"), HOST_TIMEOUT_MS);
    return host;
  }

  accept() {
    this.decided?.(true);
  }

  reject() {
    this.decided?.(false);
  }

  cancel() {
    if (this.closed) return;
    this.pipe?.send({ type: "cancel", reason: "cancelled" });
    this.shutdown();
  }

  private shutdown() {
    this.closed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.pipe?.close();
    this.listener?.close();
    this.wake?.();
  }

  private fail(message: string) {
    if (this.closed) return;
    this.shutdown();
    this.onError(message);
  }

  /** The first device to arrive is the receiver; any other is turned away. */
  private async greet(transport: Transport) {
    if (this.pipe || this.closed) {
      transport.send({ type: "cancel", reason: "busy" } as unknown as LinkMessage);
      transport.close();
      return;
    }
    const pipe = new Pipe(transport);
    this.pipe = pipe;
    pipe.onClose = () => this.fail("The other device went away");
    pipe.onError = (error) => this.fail(error instanceof Error ? error.message : String(error));
    this.onStatus("A device connected; agreeing a key…");
    const keys = await makeKeys();
    pipe.send({ type: "hello", v: VERSION, pub: keys.pub });
    pipe.onMessage = async (message) => {
      if (message.type === "cancel") return this.fail("The other device cancelled");
      if (message.type !== "hello") return;
      if (message.v !== VERSION) return this.fail("The other device runs another version of Pipit");
      const secrets = await agree(keys.priv, message.pub, this.code);
      this.cipher = new Cipher(secrets.key, 0);
      pipe.onMessage = (m) => this.handle(m);
      this.onVerify(secrets.words);
      const ok = await new Promise<boolean>((resolve) => (this.decided = resolve));
      this.decided = null;
      if (this.closed) return;
      if (!ok) {
        pipe.send({ type: "cancel", reason: "refused" });
        this.fail("You turned the other device away");
        return;
      }
      await this.control({ kind: "offer", offer: this.offer() });
      this.onStatus("Waiting for the other device to choose…");
    };
  }

  private offer(): Offer {
    const entry = this.items.entry;
    const rom = {
      id: entry.id,
      name: entry.name,
      system: entry.system ?? "gba",
      title: entry.title,
      gameCode: entry.gameCode,
      size: this.items.rom?.byteLength ?? entry.size,
    };
    const save = this.items.save ? { size: this.items.save.data.byteLength, updatedAt: this.items.save.updatedAt } : null;
    return { rom, romData: this.items.rom !== null, save };
  }

  private async control(message: Control) {
    const plain = encoder.encode(JSON.stringify(message));
    this.pipe?.send({ type: "enc", data: await this.cipher!.seal(plain.buffer as ArrayBuffer, "m") });
  }

  private async handle(message: Wire) {
    switch (message.type) {
      case "cancel":
        this.fail("The other device cancelled");
        break;
      case "ack":
        this.acked.set(message.item, message.index);
        this.wake?.();
        break;
      case "enc": {
        const control = JSON.parse(decoder.decode(await this.cipher!.open(message.data, "m"))) as Control;
        if (control.kind === "request") await this.sendItems(control.rom, control.save);
        else if (control.kind === "finished") {
          this.shutdown();
          this.onFinished(this.sent);
        }
        break;
      }
      default:
        break;
    }
  }

  private async sendItems(rom: boolean, save: boolean) {
    if (rom && this.items.rom) {
      await this.stream("rom", this.items.rom);
      this.sent.rom = true;
    }
    if (save && this.items.save) {
      await this.stream("save", this.items.save.data);
      this.sent.save = true;
    }
    if (this.closed) return;
    await this.control({ kind: "done" });
    this.onStatus("Sent; waiting for the other device to finish…");
  }

  /** Sends `data` in chunks, never more than a window ahead of what was acknowledged. */
  private async stream(item: Item, data: ArrayBuffer) {
    const chunks = Math.ceil(data.byteLength / CHUNK);
    await this.control({ kind: "begin", item, size: data.byteLength, chunks, sha256: await hashRom(data) });
    this.acked.set(item, -1);
    for (let index = 0; index < chunks && !this.closed; index++) {
      while (index - (this.acked.get(item) ?? -1) > WINDOW && !this.closed) {
        await new Promise<void>((resolve) => (this.wake = resolve));
        this.wake = null;
      }
      if (this.closed) return;
      const piece = data.slice(index * CHUNK, Math.min(data.byteLength, (index + 1) * CHUNK));
      this.pipe?.send({ type: "chunk", item, index, data: await this.cipher!.seal(piece, `${item}:${index}`) });
      this.onProgress(item, index + 1, chunks);
    }
    if (!this.closed) await this.control({ kind: "end", item });
  }
}

// ---------------------------------------------------------------------------
// The receiver
// ---------------------------------------------------------------------------

interface Part {
  size: number;
  total: number;
  sha256: string;
  chunks: Uint8Array[];
  got: number;
}

export class TransferGuest {
  onStatus: (text: string) => void = () => {};
  /** The words to compare with the sender, who decides whether to go on. */
  onVerify: (words: string) => void = () => {};
  /** What the sender offers; `request` says what to take. */
  onOffer: (offer: Offer) => void = () => {};
  onProgress: (item: Item, done: number, total: number) => void = () => {};
  onReceived: (received: Received) => void = () => {};
  onError: (message: string) => void = () => {};

  private pipe: Pipe | null = null;
  private cipher: Cipher | null = null;
  private readonly parts = new Map<Item, Part>();
  private readonly received: Received = { rom: null, save: null };
  private closed = false;

  private constructor(private readonly code: string) {}

  /** Connects to the sender under `code`; the guest reports through its callbacks. */
  static async connect(code: string, local: boolean, settings: ConnectionSettings): Promise<TransferGuest> {
    const guest = new TransferGuest(code);
    const transport = await connect(codeFor(code), local, settings).ready;
    const pipe = new Pipe(transport);
    guest.pipe = pipe;
    pipe.onClose = () => guest.fail("The sender went away");
    pipe.onError = (error) => guest.fail(error instanceof Error ? error.message : String(error));
    const keys = await makeKeys();
    pipe.send({ type: "hello", v: VERSION, pub: keys.pub });
    pipe.onMessage = async (message) => {
      if (message.type === "cancel") return guest.fail(guest.reason(message.reason));
      if (message.type !== "hello") return;
      if (message.v !== VERSION) return guest.fail("The sender runs another version of Pipit");
      const secrets = await agree(keys.priv, message.pub, code);
      guest.cipher = new Cipher(secrets.key, 1);
      pipe.onMessage = (m) => guest.handle(m);
      guest.onVerify(secrets.words);
      guest.onStatus("Check the words with the sender, who decides whether to go on");
    };
    guest.onStatus("Connected; agreeing a key…");
    return guest;
  }

  /** Asks for the offered items the user chose. */
  request(rom: boolean, save: boolean) {
    void this.control({ kind: "request", rom, save });
    this.onStatus("Receiving…");
  }

  cancel() {
    if (this.closed) return;
    this.pipe?.send({ type: "cancel", reason: "cancelled" });
    this.shutdown();
  }

  private reason(code: string): string {
    switch (code) {
      case "busy":
        return "The sender is already sending to another device";
      case "refused":
        return "The sender turned this device away";
      default:
        return "The sender cancelled";
    }
  }

  private shutdown() {
    this.closed = true;
    this.pipe?.close();
  }

  private fail(message: string) {
    if (this.closed) return;
    this.shutdown();
    this.onError(message);
  }

  private async control(message: Control) {
    const plain = encoder.encode(JSON.stringify(message));
    this.pipe?.send({ type: "enc", data: await this.cipher!.seal(plain.buffer as ArrayBuffer, "m") });
  }

  private async handle(message: Wire) {
    switch (message.type) {
      case "cancel":
        this.fail(this.reason(message.reason));
        break;
      case "enc": {
        const control = JSON.parse(decoder.decode(await this.cipher!.open(message.data, "m"))) as Control;
        await this.handleControl(control);
        break;
      }
      case "chunk": {
        const part = this.parts.get(message.item);
        if (!part || message.index >= part.total) return this.fail("The sender sent more than it announced");
        const plain = await this.cipher!.open(message.data, `${message.item}:${message.index}`);
        part.chunks[message.index] = new Uint8Array(plain);
        part.got++;
        this.onProgress(message.item, part.got, part.total);
        if ((message.index + 1) % ACK_EVERY === 0 || message.index + 1 === part.total) {
          this.pipe?.send({ type: "ack", item: message.item, index: message.index });
        }
        break;
      }
      default:
        break;
    }
  }

  private async handleControl(control: Control) {
    switch (control.kind) {
      case "offer":
        this.onOffer(control.offer);
        break;
      case "begin":
        if (control.size > 512 * 1048576 || control.chunks !== Math.ceil(control.size / CHUNK)) {
          return this.fail("The sender announced something too large");
        }
        this.parts.set(control.item, { size: control.size, total: control.chunks, sha256: control.sha256, chunks: [], got: 0 });
        break;
      case "end": {
        const part = this.parts.get(control.item);
        if (!part || part.got !== part.total) return this.fail("Some of the data never arrived");
        const data = new Uint8Array(part.size);
        let at = 0;
        for (const chunk of part.chunks) {
          data.set(chunk, at);
          at += chunk.length;
        }
        if (at !== part.size || (await hashRom(data.buffer)) !== part.sha256) return this.fail("The data arrived damaged");
        this.received[control.item] = data.buffer;
        this.parts.delete(control.item);
        break;
      }
      case "done":
        await this.control({ kind: "finished" });
        this.shutdown();
        this.onReceived(this.received);
        break;
      default:
        break;
    }
  }
}
