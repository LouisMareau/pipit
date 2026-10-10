// Playing together. Both players' consoles run on both machines, joined by the
// core's link cable, and only button states cross the network: emulation is
// deterministic, so the two machines compute the same two games and each shows
// its own console. Keys are applied a few frames late (the delay) to hide
// latency: frame F runs once both players' keys for F are known, and reaching
// frame F publishes the local keys for frame F + delay.
//
// The host is player 0, the guest player 1. Joining sends the guest's save and
// ROM digest; the host answers with its own save, the delay and the cartridge
// clock, and both load the two consoles the same way.

import type { DataConnection } from "peerjs";
import { Peer } from "peerjs";
import type { LinkLoad } from "../types";

export type LinkMessage =
  | { type: "hello"; romHash: string; save: ArrayBuffer | null }
  | { type: "welcome"; save: ArrayBuffer | null; delay: number; epoch: number }
  | { type: "refuse"; reason: string }
  | { type: "input"; frame: number; keys: number }
  | { type: "hash"; frame: number; hash: number }
  | { type: "bye" };

export interface Transport {
  send(message: LinkMessage): void;
  onMessage: (message: LinkMessage) => void;
  onClose: () => void;
  close(): void;
}

/** Two tabs of this browser that share a code: for trying it out on one machine. */
export class LocalTransport implements Transport {
  private channel: BroadcastChannel;
  onMessage: (message: LinkMessage) => void = () => {};
  onClose: () => void = () => {};

  constructor(code: string) {
    this.channel = new BroadcastChannel(`pipit-link:${code}`);
    this.channel.onmessage = (event) => this.onMessage(event.data as LinkMessage);
  }

  send(message: LinkMessage) {
    this.channel.postMessage(message);
  }

  close() {
    this.channel.close();
  }
}

export type Role = "host" | "guest";

export interface SessionStart {
  link: LinkLoad;
  /** The cartridge clock both players start from, as a Unix time. */
  epoch: number;
}

const CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** A six-letter code without look-alike characters. */
export function generateCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  return Array.from(bytes, (b) => CODE_ALPHABET[b % CODE_ALPHABET.length]).join("");
}

/** SHA-256 of the ROM, so both players can check they run the same game. */
export async function hashRom(rom: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", rom);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

export class LinkSession {
  /** The console this player controls: the host's is 0, the guest's 1. */
  readonly local: number;
  /** Frames run so far, which is also the next frame to run. */
  frame = 0;
  private delay: number;
  private started = false;
  private closed = false;
  private localKeys: number[] = [];
  private remoteKeys = new Map<number, number>();
  private localHashes = new Map<number, number>();
  private remoteHashes = new Map<number, number>();
  private helloTimer: ReturnType<typeof setInterval> | null = null;
  onStart: (start: SessionStart) => void = () => {};
  onEnd: (reason: string) => void = () => {};
  onStatus: (text: string) => void = () => {};

  constructor(
    private transport: Transport,
    readonly role: Role,
    private romHash: string,
    private save: ArrayBuffer | null,
    delay = 3,
  ) {
    this.local = role === "host" ? 0 : 1;
    this.delay = delay;
    transport.onMessage = (message) => this.receive(message);
    transport.onClose = () => this.end("The connection was lost");
    if (role === "guest") {
      // The host may not be listening yet: knock until it answers.
      const hello = () => transport.send({ type: "hello", romHash, save });
      hello();
      this.helloTimer = setInterval(hello, 1000);
    }
  }

  private stopHello() {
    if (this.helloTimer !== null) clearInterval(this.helloTimer);
    this.helloTimer = null;
  }

  private receive(message: LinkMessage) {
    if (this.closed) return;
    switch (message.type) {
      case "hello": {
        if (this.role !== "host" || this.started) return;
        if (message.romHash !== this.romHash) {
          this.transport.send({ type: "refuse", reason: "a different ROM" });
          this.onStatus("Someone tried to join with a different ROM");
          return;
        }
        const epoch = Math.floor(Date.now() / 1000);
        this.transport.send({ type: "welcome", save: this.save, delay: this.delay, epoch });
        this.start([this.save, message.save], epoch);
        break;
      }
      case "welcome":
        if (this.role !== "guest" || this.started) return;
        this.delay = message.delay;
        this.start([message.save, this.save], message.epoch);
        break;
      case "refuse":
        this.end(`The host has ${message.reason}`);
        break;
      case "input":
        this.remoteKeys.set(message.frame, message.keys);
        break;
      case "hash":
        this.remoteHashes.set(message.frame, message.hash);
        this.checkHash(message.frame);
        break;
      case "bye":
        this.end("Your partner left");
        break;
    }
  }

  private start(saves: (ArrayBuffer | null)[], epoch: number) {
    this.stopHello();
    this.started = true;
    // Nobody can have pressed anything before the first `delay` frames.
    for (let f = 0; f < this.delay; f++) {
      this.localKeys[f] = 0;
      this.remoteKeys.set(f, 0);
    }
    this.onStatus("Connected");
    this.onStart({ link: { players: 2, local: this.local, saves }, epoch });
  }

  /**
   * Called once per display frame with the keys the player holds. Returns every
   * player's keys for the next frame, or null while the partner's are still on
   * their way. Publishes the local keys for `delay` frames ahead either way, so
   * two players waiting on each other never deadlock.
   */
  tick(heldKeys: number): number[] | null {
    if (!this.started || this.closed) return null;
    const f = this.frame;
    const ahead = f + this.delay;
    if (this.localKeys[ahead] === undefined) {
      this.localKeys[ahead] = heldKeys;
      this.transport.send({ type: "input", frame: ahead, keys: heldKeys });
    }
    const remote = this.remoteKeys.get(f);
    if (remote === undefined) return null;
    const mine = this.localKeys[f] ?? 0;
    this.remoteKeys.delete(f);
    delete this.localKeys[f];
    this.frame = f + 1;
    return this.local === 0 ? [mine, remote] : [remote, mine];
  }

  /** How many frames after being pressed the keys reach the game. */
  get delayFrames(): number {
    return this.delay;
  }

  /** Whether the next frame is held up by the partner's keys. */
  get waiting(): boolean {
    return this.started && !this.closed && !this.remoteKeys.has(this.frame);
  }

  /** The worker's digest after `frame` frames, to compare with the partner's. */
  reportHash(frame: number, hash: number) {
    if (this.closed) return;
    this.localHashes.set(frame, hash);
    this.transport.send({ type: "hash", frame, hash });
    this.checkHash(frame);
  }

  private checkHash(frame: number) {
    const mine = this.localHashes.get(frame);
    const theirs = this.remoteHashes.get(frame);
    if (mine === undefined || theirs === undefined) return;
    this.localHashes.delete(frame);
    this.remoteHashes.delete(frame);
    if (mine !== theirs) this.end("The two games drifted apart");
  }

  leave() {
    if (this.closed) return;
    this.transport.send({ type: "bye" });
    this.end("You left");
  }

  private end(reason: string) {
    if (this.closed) return;
    this.closed = true;
    this.stopHello();
    this.transport.close();
    this.onEnd(reason);
  }
}

/**
 * Over the internet: a WebRTC data channel between the two browsers, with a
 * public PeerJS server making the introduction (the code is the host's id).
 */
export class PeerTransport implements Transport {
  onMessage: (message: LinkMessage) => void = () => {};
  onClose: () => void = () => {};

  private constructor(
    private peer: Peer,
    private conn: DataConnection,
  ) {
    conn.on("data", (data) => this.onMessage(data as LinkMessage));
    conn.on("close", () => this.onClose());
    conn.on("error", () => this.onClose());
  }

  private static id(code: string): string {
    return `pipit-gba-${code}`;
  }

  /** Listens under `code`; the partner's connection completes it. */
  static host(code: string, onListening: () => void): Pending<Transport> {
    const peer = new Peer(PeerTransport.id(code));
    const transport = new Promise<Transport>((resolve, reject) => {
      peer.on("open", onListening);
      peer.on("error", (error) => reject(describe(error)));
      peer.on("connection", (conn) => conn.on("open", () => resolve(new PeerTransport(peer, conn))));
    });
    return { transport, cancel: () => peer.destroy() };
  }

  static join(code: string): Pending<Transport> {
    const peer = new Peer();
    const transport = new Promise<Transport>((resolve, reject) => {
      peer.on("error", (error) => reject(describe(error)));
      peer.on("open", () => {
        const conn = peer.connect(PeerTransport.id(code), { reliable: true });
        conn.on("open", () => resolve(new PeerTransport(peer, conn)));
      });
    });
    return { transport, cancel: () => peer.destroy() };
  }

  send(message: LinkMessage) {
    this.conn.send(message);
  }

  close() {
    this.conn.close();
    this.peer.destroy();
  }
}

/** A connection being made; `cancel` gives up on it. */
export interface Pending<T> {
  transport: Promise<T>;
  cancel: () => void;
}

function describe(error: Error & { type?: string }): Error {
  switch (error.type) {
    case "peer-unavailable":
      return new Error("Nobody is hosting a game with that code");
    case "unavailable-id":
      return new Error("That code is taken; try hosting again");
    case "network":
    case "server-error":
    case "socket-error":
    case "socket-closed":
      return new Error("Could not reach the introduction server; check the connection");
    case "browser-incompatible":
      return new Error("This browser cannot connect to other players");
    default:
      return new Error(error.message || "Could not connect");
  }
}

/**
 * Starts connecting as `role` under `code`: over the internet, or between two
 * tabs of this browser when `local` (the `?link=local` switch, used by the tests).
 */
export function connect(role: Role, code: string, local: boolean, onListening: () => void): Pending<Transport> {
  if (local) {
    onListening();
    return { transport: Promise.resolve(new LocalTransport(code)), cancel: () => {} };
  }
  return role === "host" ? PeerTransport.host(code, onListening) : PeerTransport.join(code);
}
