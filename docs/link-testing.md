# Testing the link cable

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
written back afterwards, so a completed trade shows up in them.

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
