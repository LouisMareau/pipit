// Playing together. Every player's console runs on every machine, joined by the
// core's link cable, and only button states cross the network: emulation is
// deterministic, so all machines compute the same games and each shows its own
// console. Keys are applied a few frames late (the delay) to hide latency: frame
// F runs once every player's keys for F are known, and reaching frame F
// publishes the local keys for frame F + delay.
//
// The host is player 0 and the hub: guests connect to it, it relays their keys
// to one another, pings everyone, and picks the delay from the slowest path
// when it starts the game. Joining sends the guest's save and ROM digest; the
// start sends every save, the delay and the cartridge clock to each guest.

import type { DataConnection, PeerOptions } from "peerjs";
import { Peer } from "peerjs";
import type { ConnectionSettings, LinkLoad } from "../types";

export const MAX_PLAYERS = 4;
const FRAME_MS = 1000 / 59.7275;
const PING_EVERY_MS = 500;
const RTT_SAMPLES = 8;

export type LinkMessage =
  | { type: "hello"; romHash: string; save: ArrayBuffer | null }
  | { type: "refuse"; reason: string }
  | { type: "lobby"; players: number }
  | { type: "welcome"; local: number; players: number; saves: (ArrayBuffer | null)[]; delay: number; epoch: number }
  | { type: "input"; player: number; frame: number; keys: number }
  | { type: "hash"; frame: number; hash: number }
  | { type: "ping"; t: number }
  | { type: "pong"; t: number }
  | { type: "delay"; value: number }
  | { type: "end"; reason: string };

/** What `LinkSession.tick` hands to the worker: the keys to run frame `frame` with. */
export interface Tick {
  frame: number;
  keys: number[];
  /** Some keys were guessed from the last seen ones; keep the state before this frame. */
  guessed: boolean;
}

/** How far ahead of the last settled frame guessing may go before waiting instead. */
const MAX_GUESSED_FRAMES = 8;

export interface Transport {
  send(message: LinkMessage): void;
  onMessage: (message: LinkMessage) => void;
  onClose: () => void;
  close(): void;
}

/** The host's side of the code: hands over a transport for each guest that arrives. */
export interface Listener {
  onGuest: (transport: Transport) => void;
  close(): void;
}

/** Something being set up; `cancel` gives up on it. */
export interface Pending<T> {
  ready: Promise<T>;
  cancel: () => void;
}

export type Role = "host" | "guest";

export interface SessionStart {
  link: LinkLoad;
  /** The cartridge clock everyone starts from, as a Unix time. */
  epoch: number;
  delay: number;
}

const CODE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** A six-letter code without look-alike characters. */
export function generateCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  return Array.from(bytes, (b) => CODE_ALPHABET[b % CODE_ALPHABET.length]).join("");
}

/** SHA-256 of the ROM, so everyone can check they run the same game. */
export async function hashRom(rom: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", rom);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Input delay for the slowest one-way path, in frames: at least two, at most ten. */
export function delayForLatency(oneWayMs: number): number {
  return Math.min(10, Math.max(2, Math.ceil(oneWayMs / FRAME_MS) + 1));
}

const median = (values: number[]): number | null => {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[sorted.length >> 1]!;
};

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

/** Over the internet: a WebRTC data channel, introduced by a PeerJS server. */
class PeerTransport implements Transport {
  onMessage: (message: LinkMessage) => void = () => {};
  onClose: () => void = () => {};

  /** `owner` is the Peer to destroy on close (a guest's); a host keeps its Peer for others. */
  constructor(
    private conn: DataConnection,
    private owner: Peer | null,
  ) {
    conn.on("data", (data) => this.onMessage(data as LinkMessage));
    conn.on("close", () => this.onClose());
    conn.on("error", () => this.onClose());
  }

  send(message: LinkMessage) {
    this.conn.send(message);
  }

  close() {
    this.conn.close();
    this.owner?.destroy();
  }
}

const peerId = (code: string) => `pipit-gba-${code}`;

/** PeerJS options from the connection settings: the introduction server and ICE servers. */
function peerOptions(settings: ConnectionSettings): PeerOptions {
  const iceServers: RTCIceServer[] = [{ urls: "stun:stun.l.google.com:19302" }];
  if (settings.relayUrl) {
    iceServers.push({ urls: settings.relayUrl, username: settings.relayUsername, credential: settings.relayCredential });
  }
  const options: PeerOptions = { config: { iceServers } };
  if (settings.server) {
    const url = new URL(settings.server);
    const secure = url.protocol === "https:" || url.protocol === "wss:";
    Object.assign(options, {
      host: url.hostname,
      port: Number(url.port) || (secure ? 443 : 80),
      path: url.pathname || "/",
      secure,
    });
  }
  return options;
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
      return new Error("Could not reach the introduction server; check the connection settings");
    case "browser-incompatible":
      return new Error("This browser cannot connect to other players");
    default:
      return new Error(error.message || "Could not connect");
  }
}

function listenPeer(code: string, settings: ConnectionSettings): Pending<Listener> {
  const peer = new Peer(peerId(code), peerOptions(settings));
  const listener: Listener = { onGuest: () => {}, close: () => peer.destroy() };
  const ready = new Promise<Listener>((resolve, reject) => {
    peer.on("open", () => resolve(listener));
    peer.on("error", (error) => reject(describe(error)));
    peer.on("connection", (conn) => conn.on("open", () => listener.onGuest(new PeerTransport(conn, null))));
  });
  return { ready, cancel: () => peer.destroy() };
}

function connectPeer(code: string, settings: ConnectionSettings): Pending<Transport> {
  const peer = new Peer(peerOptions(settings));
  const ready = new Promise<Transport>((resolve, reject) => {
    peer.on("error", (error) => reject(describe(error)));
    peer.on("open", () => {
      const conn = peer.connect(peerId(code), { reliable: true });
      conn.on("open", () => resolve(new PeerTransport(conn, peer)));
    });
  });
  return { ready, cancel: () => peer.destroy() };
}

/**
 * Two or more tabs of this browser sharing a code (the `?link=local` switch,
 * used by the tests). Everyone is on one broadcast channel, so envelopes carry
 * addresses; the host is always "host".
 */
interface Envelope {
  from: string;
  to: string;
  body: LinkMessage | { type: "connect" } | { type: "accept" };
}

class LocalTransport implements Transport {
  onMessage: (message: LinkMessage) => void = () => {};
  onClose: () => void = () => {};
  private readonly listener = (event: MessageEvent<Envelope>) => {
    const { from, to, body } = event.data;
    if (to !== this.me || from !== this.peer || body.type === "connect" || body.type === "accept") return;
    this.onMessage(body);
  };

  constructor(
    private channel: BroadcastChannel,
    private me: string,
    private peer: string,
  ) {
    channel.addEventListener("message", this.listener);
  }

  private readonly createdAt = performance.now();

  send(message: LinkMessage) {
    const envelope = { from: this.me, to: this.peer, body: message } satisfies Envelope;
    const lag = localLag(this.createdAt);
    if (lag > 0) setTimeout(() => this.channel.postMessage(envelope), lag * (0.75 + Math.random() * 0.5));
    else this.channel.postMessage(envelope);
  }

  close() {
    this.channel.removeEventListener("message", this.listener);
  }
}

/**
 * Test hook: `?lag=A:B` delays every local message by A ms for a connection's
 * first seven seconds, then by B ms (with some jitter), to exercise guessing,
 * rollback and the delay adjustment without a network.
 */
function localLag(createdAt: number): number {
  const spec = new URLSearchParams(location.search).get("lag");
  if (!spec) return 0;
  const [first, later = first] = spec.split(":").map(Number);
  return performance.now() - createdAt < 7000 ? first! : later!;
}

const localChannel = (code: string) => new BroadcastChannel(`pipit-link:${code}`);

function listenLocal(code: string): Pending<Listener> {
  const channel = localChannel(code);
  const known = new Set<string>();
  const listener: Listener = { onGuest: () => {}, close: () => channel.close() };
  channel.addEventListener("message", (event: MessageEvent<Envelope>) => {
    const { from, to, body } = event.data;
    if (to !== "host" || body.type !== "connect" || known.has(from)) return;
    known.add(from);
    channel.postMessage({ from: "host", to: from, body: { type: "accept" } } satisfies Envelope);
    listener.onGuest(new LocalTransport(channel, "host", from));
  });
  return { ready: Promise.resolve(listener), cancel: () => channel.close() };
}

function connectLocal(code: string): Pending<Transport> {
  const channel = localChannel(code);
  const me = generateCode();
  let timer: ReturnType<typeof setInterval> | null = null;
  const ready = new Promise<Transport>((resolve) => {
    channel.addEventListener("message", (event: MessageEvent<Envelope>) => {
      const { from, to, body } = event.data;
      if (to === me && from === "host" && body.type === "accept") {
        if (timer !== null) clearInterval(timer);
        resolve(new LocalTransport(channel, me, "host"));
      }
    });
    const knock = () => channel.postMessage({ from: me, to: "host", body: { type: "connect" } } satisfies Envelope);
    knock();
    timer = setInterval(knock, 500);
  });
  return {
    ready,
    cancel: () => {
      if (timer !== null) clearInterval(timer);
      channel.close();
    },
  };
}

/** Hosts under `code`: over the internet, or between tabs of this browser when `local`. */
export function listen(code: string, local: boolean, settings: ConnectionSettings): Pending<Listener> {
  return local ? listenLocal(code) : listenPeer(code, settings);
}

/** Joins the host under `code`. */
export function connect(code: string, local: boolean, settings: ConnectionSettings): Pending<Transport> {
  return local ? connectLocal(code) : connectPeer(code, settings);
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

interface Guest {
  transport: Transport;
  player: number;
  save: ArrayBuffer | null;
  /** Said hello with the right ROM. */
  ready: boolean;
  rtts: number[];
}

export class LinkSession {
  /** The console this player controls: the host's is 0. */
  local = 0;
  players = 1;
  /** Frames run so far, which is also the next frame to run. */
  frame = 0;
  private delay = 2;
  private started = false;
  private closed = false;
  /** Host only: everyone who connected, in player order. */
  private guests: Guest[] = [];
  /** Guest only. */
  private host: Transport | null = null;
  /** Every player's keys per frame, as far as they are known. */
  private inputs = new Map<number, (number | undefined)[]>();
  /** The keys each frame actually ran with (guesses included), until settled. */
  private applied = new Map<number, number[]>();
  /** Frames run with guessed keys that have not been confirmed yet. */
  private guessedFrames = new Set<number>();
  /** Every frame up to here ran with keys everyone has confirmed. */
  private settled = -1;
  /** The last keys seen from each player: the guess for frames not heard about yet. */
  private lastKnown: number[] = [];
  private lastPublished = -1;
  private calmSeconds = 0;
  /** Times a wrong guess had to be undone. */
  rollbacks = 0;
  /** A wrong guess: the worker goes back to `toFrame` and re-runs with `inputs`. */
  onRollback: (toFrame: number, inputs: number[][], snapshots: boolean[]) => void = () => {};
  /** Every frame up to this one is final; kept states up to it can go. */
  onConfirm: (frame: number) => void = () => {};
  private ownHashes = new Map<number, number>();
  private guestHashes = new Map<number, Map<number, number>>();
  private rtts: number[] = [];
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private helloTimer: ReturnType<typeof setInterval> | null = null;
  /** How many players are in (host and ready guests). */
  onLobby: (players: number) => void = () => {};
  onStart: (start: SessionStart) => void = () => {};
  onEnd: (reason: string) => void = () => {};
  onStatus: (text: string) => void = () => {};
  /** The latest round trip (a guest's to the host; the host's slowest guest). */
  onPing: (ms: number) => void = () => {};

  private constructor(
    readonly role: Role,
    private romHash: string,
    private save: ArrayBuffer | null,
  ) {}

  /** Hosts: guests arriving through `listener` join the lobby until `start()`. */
  static host(listener: Listener, romHash: string, save: ArrayBuffer | null): LinkSession {
    const session = new LinkSession("host", romHash, save);
    listener.onGuest = (transport) => session.addGuest(transport);
    session.pingTimer = setInterval(() => session.pingAll(), PING_EVERY_MS);
    session.onStatus("Waiting for players…");
    return session;
  }

  /** Joins the host behind `transport`, knocking until it answers. */
  static join(transport: Transport, romHash: string, save: ArrayBuffer | null): LinkSession {
    const session = new LinkSession("guest", romHash, save);
    session.host = transport;
    transport.onMessage = (message) => session.fromHost(message);
    transport.onClose = () => session.end("The connection to the host was lost");
    const hello = () => transport.send({ type: "hello", romHash, save });
    hello();
    session.helloTimer = setInterval(hello, 1000);
    session.pingTimer = setInterval(() => transport.send({ type: "ping", t: performance.now() }), PING_EVERY_MS);
    session.onStatus("Joining…");
    return session;
  }

  // -- host side --------------------------------------------------------------

  private addGuest(transport: Transport) {
    if (this.closed) return;
    if (this.started || this.guests.length >= MAX_PLAYERS - 1) {
      transport.send({ type: "refuse", reason: this.started ? "already started" : "no room left" });
      transport.close();
      return;
    }
    const guest: Guest = { transport, player: this.guests.length + 1, save: null, ready: false, rtts: [] };
    this.guests.push(guest);
    transport.onMessage = (message) => this.fromGuest(guest, message);
    transport.onClose = () => {
      if (this.started) this.endAll("A player left", guest);
      else this.dropGuest(guest);
    };
  }

  /**
   * Host: ends the game for everyone still here (`gone` has already left).
   * Closed first: a send to a channel that is already gone reports an error,
   * which would otherwise end the session again from inside this one.
   */
  private endAll(reason: string, gone?: Guest) {
    if (this.closed) return;
    this.closed = true;
    for (const guest of this.guests) if (guest !== gone) safeSend(guest.transport, { type: "end", reason });
    this.finish(reason);
  }

  private dropGuest(guest: Guest) {
    this.guests = this.guests.filter((g) => g !== guest);
    this.guests.forEach((g, i) => (g.player = i + 1));
    this.announceLobby();
  }

  private fromGuest(guest: Guest, message: LinkMessage) {
    if (this.closed) return;
    switch (message.type) {
      case "hello":
        if (guest.ready || this.started) return;
        if (message.romHash !== this.romHash) {
          guest.transport.send({ type: "refuse", reason: "a different ROM" });
          guest.transport.close();
          this.dropGuest(guest);
          this.onStatus("Someone tried to join with a different ROM");
          return;
        }
        guest.save = message.save;
        guest.ready = true;
        this.announceLobby();
        break;
      case "ping":
        guest.transport.send({ type: "pong", t: message.t });
        break;
      case "pong":
        guest.rtts.push(performance.now() - message.t);
        if (guest.rtts.length > RTT_SAMPLES) guest.rtts.shift();
        this.onPing(Math.max(...this.guests.map((g) => median(g.rtts) ?? 0)));
        break;
      case "input":
        for (const other of this.guests) if (other !== guest && other.ready) other.transport.send(message);
        this.receiveInput(message.frame, message.player, message.keys);
        break;
      case "hash":
        if (!this.guestHashes.has(message.frame)) this.guestHashes.set(message.frame, new Map());
        this.guestHashes.get(message.frame)!.set(guest.player, message.hash);
        this.checkHashes(message.frame);
        break;
      case "end":
        if (this.started) this.endAll("A player left", guest);
        else this.dropGuest(guest);
        break;
      default:
        break;
    }
  }

  private announceLobby() {
    const players = 1 + this.guests.filter((g) => g.ready).length;
    this.onLobby(players);
    for (const guest of this.guests) if (guest.ready) guest.transport.send({ type: "lobby", players });
  }

  private pings = 0;

  private pingAll() {
    for (const guest of this.guests) guest.transport.send({ type: "ping", t: performance.now() });
    if (++this.pings % 2 === 0) this.adjustDelay();
  }

  /** The delay that covers the slowest path, guests' keys being relayed through the host. */
  private chooseDelay(): number {
    const trips = this.guests.filter((g) => g.ready).map((g) => median(g.rtts) ?? 100);
    let worst = Math.max(0, ...trips.map((t) => t / 2));
    for (const a of trips) for (const b of trips) if (a !== b) worst = Math.max(worst, (a + b) / 2);
    return delayForLatency(worst);
  }

  /** Host: starts the game with everyone who is in. */
  start() {
    if (this.role !== "host" || this.started || this.closed) return;
    const ready = this.guests.filter((g) => g.ready);
    if (ready.length === 0) return;
    for (const guest of this.guests) {
      if (!guest.ready) {
        guest.transport.send({ type: "refuse", reason: "the game started" });
        guest.transport.close();
      }
    }
    this.guests = ready;
    this.guests.forEach((g, i) => (g.player = i + 1));
    this.players = 1 + ready.length;
    this.delay = this.chooseDelay();
    const epoch = Math.floor(Date.now() / 1000);
    const saves = [this.save, ...ready.map((g) => g.save)];
    for (const guest of ready) {
      guest.transport.send({ type: "welcome", local: guest.player, players: this.players, saves, delay: this.delay, epoch });
    }
    this.begin({ link: { players: this.players, local: 0, saves }, epoch, delay: this.delay });
  }

  // -- guest side -------------------------------------------------------------

  private fromHost(message: LinkMessage) {
    if (this.closed) return;
    switch (message.type) {
      case "lobby":
        this.onLobby(message.players);
        this.onStatus(`${message.players} players in; waiting for the host to start`);
        break;
      case "welcome":
        if (this.started) return;
        this.local = message.local;
        this.players = message.players;
        this.delay = message.delay;
        this.begin({ link: { players: message.players, local: message.local, saves: message.saves }, epoch: message.epoch, delay: message.delay });
        break;
      case "refuse":
        this.end(`The host said no: ${message.reason}`);
        break;
      case "input":
        this.receiveInput(message.frame, message.player, message.keys);
        break;
      case "delay":
        this.delay = message.value;
        break;
      case "ping":
        this.host?.send({ type: "pong", t: message.t });
        break;
      case "pong":
        this.rtts.push(performance.now() - message.t);
        if (this.rtts.length > RTT_SAMPLES) this.rtts.shift();
        this.onPing(median(this.rtts) ?? 0);
        break;
      case "end":
        this.end(message.reason);
        break;
      default:
        break;
    }
  }

  // -- both -------------------------------------------------------------------

  private begin(start: SessionStart) {
    if (this.helloTimer !== null) clearInterval(this.helloTimer);
    this.helloTimer = null;
    this.started = true;
    // Nobody can have pressed anything before the first `delay` frames.
    for (let f = 0; f < this.delay; f++) this.inputs.set(f, new Array<number>(this.players).fill(0));
    this.lastPublished = this.delay - 1;
    this.settled = this.delay - 1;
    this.lastKnown = new Array<number>(this.players).fill(0);
    this.onStatus("Connected");
    this.onStart(start);
  }

  private storeKeys(frame: number, player: number, keys: number) {
    let row = this.inputs.get(frame);
    if (!row) {
      row = new Array<number | undefined>(this.players).fill(undefined);
      this.inputs.set(frame, row);
    }
    row[player] = keys;
  }

  /** A player's keys for a frame arrived: keep them, and undo any wrong guess. */
  private receiveInput(frame: number, player: number, keys: number) {
    this.storeKeys(frame, player, keys);
    this.lastKnown[player] = keys;
    const used = this.applied.get(frame);
    if (frame < this.frame && used && used[player] !== keys) this.rollbackTo(frame);
    this.settle();
  }

  private sendAll(message: LinkMessage) {
    if (this.role === "host") for (const guest of this.guests) safeSend(guest.transport, message);
    else if (this.host) safeSend(this.host, message);
  }

  /** The keys a frame will run with: what is known, and the last seen keys for the rest. */
  private resolve(frame: number): { keys: number[]; guessed: boolean } {
    const row = this.inputs.get(frame) ?? [];
    let guessed = false;
    const keys = Array.from({ length: this.players }, (_, p) => {
      const known = row[p];
      if (known !== undefined) return known;
      guessed = true;
      return this.lastKnown[p] ?? 0;
    });
    return { keys, guessed };
  }

  /**
   * Called once per display frame. `keysFor` gives the local keys for a frame
   * about to be published (the frame `delay` ahead, or several when the delay
   * just grew). Returns the next frame to run, with any late keys guessed from
   * the last seen ones, or null when the guesses would reach too far ahead.
   * Publishing happens either way, so players waiting on each other never deadlock.
   */
  tick(keysFor: (frame: number) => number): Tick | null {
    if (!this.started || this.closed) return null;
    const f = this.frame;
    for (let ahead = this.lastPublished + 1; ahead <= f + this.delay; ahead++) {
      const keys = keysFor(ahead);
      this.storeKeys(ahead, this.local, keys);
      this.sendAll({ type: "input", player: this.local, frame: ahead, keys });
      this.lastPublished = ahead;
    }
    const { keys, guessed } = this.resolve(f);
    if (guessed && f - this.settled > MAX_GUESSED_FRAMES) return null;
    this.applied.set(f, keys);
    if (guessed) this.guessedFrames.add(f);
    this.frame = f + 1;
    this.settle();
    return { frame: f, keys, guessed };
  }

  /** Re-runs from `frame` with the keys now known; guesses again for what is still missing. */
  private rollbackTo(frame: number) {
    const rows: number[][] = [];
    const snapshots: boolean[] = [];
    for (let g = frame; g < this.frame; g++) {
      const { keys, guessed } = this.resolve(g);
      this.applied.set(g, keys);
      if (guessed) this.guessedFrames.add(g);
      else this.guessedFrames.delete(g);
      rows.push(keys);
      snapshots.push(guessed);
    }
    this.rollbacks++;
    this.onRollback(frame, rows, snapshots);
  }

  /** Advances past every frame whose keys are all known and were run as known. */
  private settle() {
    let moved = false;
    while (this.settled + 1 < this.frame) {
      const next = this.settled + 1;
      const row = this.inputs.get(next);
      if (!row || row.some((k) => k === undefined)) break;
      this.settled = next;
      this.guessedFrames.delete(next);
      this.inputs.delete(next);
      this.applied.delete(next);
      moved = true;
    }
    if (moved) this.onConfirm(this.settled);
  }

  /** Host: raises the delay as soon as the pings call for it, lowers it only after a calm while. */
  private adjustDelay() {
    if (!this.started || this.guests.length === 0) return;
    const needed = this.chooseDelay();
    if (needed > this.delay) this.setDelay(needed);
    else if (needed < this.delay && ++this.calmSeconds >= 5) this.setDelay(this.delay - 1);
    else return;
    this.calmSeconds = 0;
  }

  private setDelay(value: number) {
    this.delay = value;
    this.sendAll({ type: "delay", value });
  }

  /** How many frames after being pressed the keys reach the game. */
  get delayFrames(): number {
    return this.delay;
  }

  /** Whether the next frame is held up: someone's keys are missing beyond what can be guessed. */
  get waiting(): boolean {
    if (!this.started || this.closed) return false;
    const { guessed } = this.resolve(this.frame);
    return guessed && this.frame - this.settled > MAX_GUESSED_FRAMES;
  }

  /** The worker's digest after `frame` frames: the host compares everyone's. */
  reportHash(frame: number, hash: number) {
    if (this.closed) return;
    if (this.role === "host") {
      this.ownHashes.set(frame, hash);
      this.checkHashes(frame);
    } else {
      this.host?.send({ type: "hash", frame, hash });
    }
  }

  private checkHashes(frame: number) {
    // A digest is skipped while keys are guesses, so partners' entries may never pair up.
    for (const old of this.ownHashes.keys()) if (old < frame - 600) this.ownHashes.delete(old);
    for (const old of this.guestHashes.keys()) if (old < frame - 600) this.guestHashes.delete(old);
    const mine = this.ownHashes.get(frame);
    const theirs = this.guestHashes.get(frame);
    if (mine === undefined || !theirs || theirs.size < this.guests.length) return;
    this.ownHashes.delete(frame);
    this.guestHashes.delete(frame);
    for (const hash of theirs.values()) {
      if (hash !== mine) {
        this.endAll("The games drifted apart");
        return;
      }
    }
  }

  leave() {
    if (this.closed) return;
    this.closed = true;
    this.sendAll({ type: "end", reason: this.role === "host" ? "The host left" : "A player left" });
    this.finish("You left");
  }

  private end(reason: string) {
    if (this.closed) return;
    this.closed = true;
    this.finish(reason);
  }

  private finish(reason: string) {
    if (this.pingTimer !== null) clearInterval(this.pingTimer);
    if (this.helloTimer !== null) clearInterval(this.helloTimer);
    for (const guest of this.guests) safeClose(guest.transport);
    if (this.host) safeClose(this.host);
    this.onEnd(reason);
  }
}

/** A channel may already be gone; that is not a reason to fail here. */
function safeSend(transport: Transport, message: LinkMessage) {
  try {
    transport.send(message);
  } catch {
    // Closed under us: nothing to deliver to.
  }
}

function safeClose(transport: Transport) {
  try {
    transport.close();
  } catch {
    // Already closed.
  }
}
