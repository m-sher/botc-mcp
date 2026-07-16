# Nominations, Voting, and Execution

Day phase after discussion. Storyteller opens and closes nominations.

## Constraints

| Rule | Detail |
| --- | --- |
| Nominations per living player | **Once** per day |
| Times a given player may be nominated | **Once** per day |
| Executions per day | **At most one** |
| Execution required? | **No** |
| Dead players nominate? | **No** |
| Dead players vote? | **Yes**, one **ghost vote** total for the rest of the game |

## Nomination

1. Storyteller opens nominations, **or** the first living player nominates during Discussion (engine auto-opens).
2. A living player: “I nominate **X**.”
3. Nominee may defend.
4. Vote runs immediately after.

### Virgin

The **first** time the Virgin is nominated:

- If the nominator is a **Townsfolk**, the nominator is **executed immediately** (no vote).
  That is the day’s execution.
- If the nominator is Outsider / Minion / Demon, nothing special; vote normally.
- Ability is spent after the first nomination of the Virgin, regardless of outcome.

## Voting

1. Votes are counted in **clockwise** order starting from the nominee; nominee is last.
2. **Engine deviation from official BotC (#73):** the nominator's **yes** is cast automatically at nomination time (official play requires them to raise their hand like anyone else, and they may vote **no**). This engine records the yes immediately and will not let them flip to no later.
3. **Exception — Butler:** a living Butler whose ability is active still cannot cast yes until their master has voted yes. If they nominate before that, the auto-yes is skipped and they take a normal Vote turn under the Butler rule.
4. Other living players may vote on **every** nomination that day (one vote each per nomination).
5. Dead players may spend their **single** ghost vote on one nomination (any day remaining).
6. Dead players may **pass** (abstain) without spending the ghost vote; once the ghost is spent they cannot vote again (yes or no).
7. Storyteller tallies raised hands / declared votes.

### Engine auto-close

The vote window auto-closes when **all living** players have voted **and** every dead player who still has a ghost vote available has either voted or called `pass_vote`. The host may `close_vote` earlier; missing votes count as no.

Host `close_vote` (and player auto-close) may also **end the day** as a side effect when the closed nomination leaves no further legal nomination (see below).

### Engine auto-end day

After a vote closes (or a Virgin execution ends the day’s execution), if **no further legal nomination** exists — every living seat has already nominated, or every other living seat has already been nominated, or an execution already happened today — the engine runs execution resolution / win checks and enters the next night automatically. The host may still force-end earlier with `end_nominations`.

## Execution threshold

A nomination is a contender only if:

1. Votes ≥ **half the number of living players** (e.g. 6 living → need 3), **and**
2. Votes are **strictly greater** than any other nomination’s total **today**.

**Ties** for highest valid total → **no execution**.

Later nominations may overtake the current leader with a strictly higher valid total.

Storyteller last-calls nominations, then executes the leader if any.

## Execution resolution

1. Kill the executed player (unless an ability prevents death on execution).
2. Mark them dead; they lose their ability (unless text says otherwise); they gain one ghost vote if not already dead.
3. **Do not** reveal character or alignment.
4. Check win conditions.
5. If the game continues → Night.
