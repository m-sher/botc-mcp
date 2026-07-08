# Gameplay Loop

Phases alternate **Night** and **Day** until a win condition fires.
The game **always starts with the First Night**.

```
Setup → First Night → Day 1 → Night 2 → Day 2 → Night 3 → …
         └────────────── repeat until win ──────────────┘
```

## Phase summary

| Phase | Main activities |
| --- | --- |
| **Night** | Characters act in fixed order; private info and choices; Imp kills on nights after the first |
| **Day** | Discussion, nominations, voting, at most one execution |

---

## Night phase

1. Night begins. Players do not observe other players’ night actions.
2. Storyteller resolves the **night order** top-to-bottom
   ([night-order.md](night-order.md)): First Night list, then Other Nights on later nights.
3. For each character that acts:
   - Contact that player privately.
   - They choose targets / receive information as their ability requires.
   - Proceed to the next character.
4. If a player died earlier **that night** before their slot, skip them.
5. Dawn: announce **which players died** (not who killed them or how).
   - If nobody died, say so with no extra detail.

### First Night — evil briefing

**7+ players:**

1. Minions learn who the Demon is (and who the other Minions are).
2. Demon learns who the Minions are and receives **three good characters not in play**
   (safe bluffs).
3. Imp does **not** kill on the first night.

**5–6 players:** Demon does **not** learn Minions or receive not-in-play bluffs; Minions
do **not** learn who the Demon is. (They still know they are evil and their own role.)

### Later nights

- Imp chooses a kill.
- Poisoner, Spy, Empath, Fortune Teller, Monk, Butler, Undertaker, Ravenkeeper, etc.
  resolve per [night-order.md](night-order.md).

---

## Day phase

1. **Discussion** — free public or private talk. Storyteller paces when to open nominations.
2. **Nominations** — living players nominate; votes; at most one execution.
3. Check win conditions after any death.
4. If the game continues → next Night.

Details: [voting-and-nominations.md](voting-and-nominations.md).

---

## Resolution notes

- Night order is the wake/reminder sequence. Death-triggered abilities (e.g. Ravenkeeper)
  resolve when the death happens. Ability text beats sheet order on conflict.
  See [abilities-rules.md](abilities-rules.md).

---

## End of game

After relevant deaths, check [win-conditions.md](win-conditions.md):

- Demon dead (and Scarlet Woman did not convert) → **good** wins.
- Exactly **two** living players remain with a living Demon → **evil** wins.
- Both at once → **good** wins.
- Saint executed / Mayor three-alive no-exec / etc. per that file.
