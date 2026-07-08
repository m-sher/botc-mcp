# Gameplay Loop

Blood on the Clocktower alternates **Night** and **Day** until a win condition
triggers. The game **always starts with the First Night**.

```
Setup → First Night → Day 1 → Night 2 → Day 2 → Night 3 → …
         └────────────── repeat until win ──────────────┘
```

## Phase summary

| Phase | Eyes | Main activities |
| --- | --- | --- |
| **Night** | Closed | Storyteller wakes characters in night-sheet order; demon kills; info abilities resolve |
| **Day** | Open | Discussion, nominations, voting, at most one execution |

---

## Night phase

1. Storyteller announces night; all players close their eyes.
2. Storyteller walks the **night sheet** top-to-bottom (First Night side, then Other Nights on later nights).
3. For each relevant character:
   - Wake with the agreed signal (typically two taps).
   - Player uses ability (point, receive fingers/yes-no/tokens).
   - Sleep (Storyteller covers own eyes).
4. If a player died earlier that night **before** their wake slot, skip them.
5. After the last ability: brief pause (5–10 seconds).
6. Dawn: “Open your eyes.” Announce **which players died** (not who killed them or how).
   - If nobody died, say so without extra detail.

### First Night specials (7+ players)

Before most good abilities (exact order is on the night sheet):

1. Wake **all Minions** together → show **This is the Demon** → sleep.
2. Wake the **Demon** → show **These are your Minions** → show **These characters are not in play** (three good tokens as safe bluffs) → sleep.

At **5–6 players**, evil often does **not** learn each other / bluffs the same way
(Teensyville / small-game variants — check script guidance).

### Other nights

- Flip the night sheet to **Other Nights**.
- Demon typically kills; many day-reactive abilities (Undertaker, etc.) fire.
- Poisoner / Spy / Empath / Fortune Teller / Monk / Butler / etc. act as listed.

Full Trouble Brewing order: [night-order.md](night-order.md).

---

## Day phase

1. **Discussion** — free talk (group or private). Recommended ~5–10 minutes; Storyteller may extend.
2. **Nominations open** when Storyteller calls for them.
3. For each nomination: defense → clockwise vote → record tally.
4. At most **one execution** per day (highest valid vote total; ties mean no execution).
5. If a win condition is met after execution or other day death, end the game.
6. Otherwise → next Night.

Details: [voting-and-nominations.md](voting-and-nominations.md).

---

## Timing notes

- There is **no fixed real-time limit** on discussion; the Storyteller paces.
- Night is silent for players; only the Storyteller communicates via signals.
- Ability order on the night sheet is a **guide** for waking and reminders;
  some abilities trigger immediately on death regardless of sheet position.
  Ability text on the character token outranks sheet order when they conflict.
  See [abilities-rules.md](abilities-rules.md).

---

## End of game

Check after relevant deaths (execution, night kills, ability-triggered deaths):

- Demon(s) dead → good wins (unless a character ability delays this).
- Exactly **two** living players (non-Traveller) remain with a living Demon → evil wins.
- Simultaneous both → **good** wins.
- Special characters (Saint execution, Mayor 3-alive no-execution, Klutz, etc.) can end the game early.

See [win-conditions.md](win-conditions.md).
