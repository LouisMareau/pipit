# Testing the link cable and the Wireless Adapter

`pipit link` runs two to four consoles joined by a link cable, in lockstep, in one
process. The first console is the parent (the one that starts every transfer).
Each console gets its own save file and key script:

```text
pipit link game.gba --players 2 --save a.sav --save b.sav --frames 9000 \
    --keys "$(cat keys-a.txt)" --keys "$(cat keys-b.txt)" \
    --trace-sio --screenshot build/link.png
```

`--trace-sio` prints every transfer (frame, then the 16-bit word from each
console); `--screenshot` writes `link-1.png`, `link-2.png`, …; the save files are
written back afterwards, so a completed trade shows up in them. `--peek
ADDR[:LEN]` prints memory of every console at the end (Emerald keeps the player
object at `02037350`, its map coordinates plus seven at `+10`).

`--check-rollback N` does what playing together does when keys arrive late:
every N frames it snapshots the link, runs on, restores the snapshot and runs
the same frames again, then compares state digests, and reports any stretch
that ran differently. A run of the Colosseum scenario with N = 8 (the most
frames the app ever guesses) reports no mismatches; it is the first thing to
rerun if a link ever misbehaves only when playing online.

## A trade in Pokémon Emerald, headless

This is the test the link was built against. It needs a save file standing at the
Cable Club counter (the right-hand attendant on a Pokémon Center's upper floor,
which runs the cable club when no wireless adapter is present), with the Pokédex
and at least two Pokémon. Both consoles can use the same save file; name one of
the Pokémon so the trade is visible afterwards.

The key scripts below are the ones that worked with such a save (text speed and
the player's spot change the frame numbers, so expect to nudge them with a few
`--screenshot` runs):

| Frames | Console 1 (parent) | Console 2 | What happens |
|-------:|-------------------|-----------|--------------|
| 60–600 | A every 30 | same | title screen, Continue |
| 760 | UP | same | face the counter |
| 820–3650 | A every 45 | same | attendant, Trade Center, save prompt, "Please wait", link-up, "A Button: Confirm", enter the room |
| 3760 | UP 12 | same | out of the doorway |
| 3800 | LEFT 10 | RIGHT 10 | one tile sideways |
| 3840 | UP 28 | same | two tiles up, onto the chair: the trade menu opens once both sit |
| 4450 | — | RIGHT | console 2 picks its second Pokémon |
| 4500 | A | A | select |
| 4600 | DOWN | same | Summary → Trade |
| 4650 | A | same | Trade |
| 4800, 5000 | A | same | "Is this trade okay?" → Yes |
| …9000 | | | the trade, then the game saves itself |

Serial transfers start at the link-up (one per frame during the handshake, then
eight per frame once connected). After 9000 frames the written-back saves show the
swapped parties.

A quick way to read a party out of a save file: the newest slot is the one with
the higher save counter at offset `0xFFC` of its sectors; in its section 1 the
party count is at `0x234` and each Pokémon's nickname at `0x238 + 100·i + 8`
(Gen 3 text: `0xBB` = A).

## Through the web app

Playing together runs both consoles on both machines and only exchanges button
states (see [architecture.md](architecture.md)), so the same trade can be driven
through the web app itself, in two browser windows:

```text
node scripts/link-trade-web.mjs game.gba counter.sav keys-a.txt keys-b.txt http://localhost:4173
```

The script adds the ROM, seeds the save, hosts in one window and joins from the
other, feeds the key scripts through the app's test hook (`window.pipit.keyScript`,
keys per game frame as above), waits the three minutes the session takes at the
display rate, and reads both saves back (`window.pipit.save()`) to check the
parties swapped. The smoke test covers the session mechanics without a ROM that
uses the cable: three windows join one host, the host starts, all stay linked
with matching state digests while keys are pressed, and when one guest leaves
everyone is told. It uses `?link=local` (tabs of one browser, no introduction
server); running it against the plain URL exercises the PeerJS path instead.
With `&lag=0:120` the local channel delays every message by 120 ms after a
connection's first seven seconds, which makes the host raise the delay and
forces guessed keys and rollbacks; the test asserts both happened. The trade
script accepts the same URL, so the Emerald trade can be run with lag too.

## The session log

Everything playing together does is written to the browser console (F12,
Console) under `[pipit link]`: hosting and joining, who joined the lobby, the
start (players, delay, round trips, cable or adapters), every delay change,
keys that came late and the rollbacks they caused, stalls while waiting for a
player's keys, the state-digest checks, the end and its reason. Once a second
while linked a status line gives the frame, the delay, the ping, the rollback
count, the frame rate with the worst gap, and the in-game link activity (cable
transfers, or the adapters' "data ready" wake-ups, per second): a game whose
link has broken shows that number dropping to zero. "Copy session log" (in the
⋯ menu while linked, and in the Play-together dialog afterwards) puts the whole
log on the clipboard for a bug report; `window.pipit.linkLog()` returns it to
scripts. Nothing in the log identifies a player beyond the player number.

## Where the cable club is

In Emerald, FireRed and LeafGreen only the **right-hand** counter upstairs in a
Pokémon Center runs the cable club (Trade Center, Colosseum, Record Corner); the
other two attendants ask for the Wireless Adapter and, with a cable, say it is
not connected. Ruby and Sapphire have a single cable counter. The Pokédex and,
for trading, at least two Pokémon are needed; no badge is.

## The Wireless Adapter

`pipit link --wireless` plugs a Wireless Adapter into every console instead of
a cable; in the app the host picks "Wireless Adapter" under Link before
starting. Any console can then host a wireless group and the others find it, so
the **middle** counter (Union Room) and the **left** one (Wireless Club) work.
`--trace-rfu` prints every adapter command with its answer, one line each
(console, command byte, parameter words, answer words), plus the commands the
adapter sends back (`<- 28` "data ready", `<- 29` "disconnected") and the
logins and resets:

```text
pipit link game.gba --players 2 --wireless --trace-rfu \
    --save a.sav --save b.sav --frames 4800 --keys "$(cat union-a.txt)" --keys "$(cat union-b.txt)"
```

Two consoles that start from the same save with the same key script are perfect
clones: in the Union Room they switch between hosting and searching at the same
random moments and never meet. Give the second console's script an offset of a
few frames (every frame number plus eleven, say) and the partner's trainer
appears in the room a couple of seconds after entering. From the counter save
above, the attendant for the Union Room is reached with Down (one tile, past the
counter's end), Left for four tiles, Up (one tile), Up again to face it; then A
every 45 frames through "enter?", the save prompt and the door.

A trade over the adapters, through the Wireless Club, is the test the adapter
was built against. With an adapter the **right-hand** counter is the Direct
Corner; from the counter save, both consoles press A every 45 frames from
frame 820 (the attendant, Trade, the save prompt, the explanations). At frame
2750 the "Join group / Become leader" menu is open: the leader presses Down
there, the joiner nothing. From 3655 A every 45 frames answers the join request
and the OKs; the leader presses Start at 3300 and 3600 to begin. Both are in
the Trade Center by frame 4200 and the trade goes as over the cable, 550 frames
later: Up 12 at 4300, Left/Right 10 at 4340, Up 28 at 4380 onto the chairs, the
menu at 5100 (joiner Right at 5150, A 5200, Down 5300, A 5350, A 5500, A
5700), done by frame 10500 with console 1 holding DUPE, DUPE and console 2
MUDKIP, MUDKIP. `--trace-rfu` shows the joiner connecting (`1F`, `20`, `21`)
and then every frame crossing: the child's `25` with its frame, the host's `24`
and `26`, and the `<- 28` wake-ups.

A trade inside the Union Room goes through its trading board. One player
registers a Pokémon with the attendant (top-left counter: Up to the top row,
Left until the attendant blocks, Left again to face her, A; Register; the
first Pokémon; then the wanted type from a list whose order is Normal, Fire,
Water, Electric, Grass, Ice, Flying, Psychic, Fighting, Poison, … so Water is
two Downs; A; A to dismiss; then one step Down, away from the counter, because
any further A would offer to cancel the registration). The other player reads
the board, the poster left of the attendant: Up to the top row, Down one tile,
Left to x = 2, Up one tile, A; the first offer; Yes; the first Pokémon; then
"Please wait" while the adapters connect. The registrar sees "Somebody has
contacted you", an offer for the registered Pokémon, answers Yes, and both
say "The trade will be started"; the trade and the save follow by frame
13600 of the scripts in `build/link/union-trade-a.txt` and `-b.txt`. To tell
the Pokémon apart, give the second console a copy of the save as another
trainer: `node scripts/emerald-save.mjs as-trainer counter.sav bob.sav Bob`
(names only; changing trainer ids would turn the Pokémon into bad eggs, since
the id is part of their encryption key), and read the parties afterwards with
`node scripts/emerald-save.mjs party <file.sav>`: QUINCY ends with BOB's
Mudkip and BOB with QUINCY's.

How the adapter is modelled is in [architecture.md](architecture.md): the
protocol words in `crates/gba/src/rfu.rs`, what travels between adapters in
`crates/gba/src/wireless.rs`. Data frames cross in rounds; a round runs as soon
as a host and all its children wait for one, and every quarter frame otherwise.

## Game Boy games

The same `pipit link` works for `.gb` and `.gbc` files, two consoles at a time
(a Game Boy cable joins two). Either console may clock a transfer, as the Pokémon
games take turns doing, so the two are kept within one instruction of each other
rather than synchronised at the host's transfers. Playing together in the app
caps the lobby at two players for these games.
