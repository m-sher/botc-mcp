# Overview

## The game

Players sit in a fixed circle order. One seat is the **Storyteller** (moderator /
server): not a character, not on a team. Every other seat is a **player** with a
secret character.

A **Demon** (the Imp) hides among the group. Good must execute the Demon. Evil must
reduce the living players to **two**.

| Team | Types | Goal |
| --- | --- | --- |
| **Good** | Townsfolk, Outsiders | Kill the Demon |
| **Evil** | Minions, Demon (Imp) | Two living players left |

- Good outnumbers evil. Evil knows each other from the first night (see setup / night
  rules for player-count edge cases) and may lie freely.
- Every player has exactly one character and ability.
- The Storyteller holds the **Grimoire**: private full game state.

## Four player rules

1. **Say whatever you want at any time** (public or private).
2. **No peeking** — do not learn other players’ true characters or Grimoire contents
   except via abilities.
3. **Ask the Storyteller** rules or ability questions (privately if needed).
4. **Play nice** — deception is in-game; abuse is not.

## Design pillars (for the sim)

- **Death is not the end** — dead players talk, share their team’s win/loss, and have
  **one ghost vote** for the rest of the game.
- **Info can be wrong** — drunk or poisoned players may receive false ability results
  and are not told.
- **Abilities are the engine** — each role is unique; resolve them in night order and
  on day triggers.

## Character pool

This simulation uses a fixed pool (Trouble Brewing). Full list:
[characters.md](characters.md).

## State the Storyteller tracks

| Concept | Purpose |
| --- | --- |
| **Grimoire** | True characters, reminders, dead, poison, protection, etc. |
| **Living / dead** | Public after announcements |
| **Night order** | Who acts when at night |
| **Character sheet** | Public list of characters that *might* be in the bag |
| **Reminders** | Poison, Monk protect, Butler master, red herring, Drunk, etc. |
