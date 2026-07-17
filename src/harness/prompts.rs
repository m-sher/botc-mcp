//! System / kickoff prompts for headless player and host agents (grok or claude).

use crate::game::SeatId;
use crate::harness::scheduler::{HostTask, PlayerTask};

/// One-line tool-discipline reminder repeated on every per-tick prompt — the kickoff
/// sets the rule, but agents (claude especially) fall back to narrating their move on
/// resume ticks, so re-state it. Identical for every backend.
const ACT_VIA_TOOLS: &str = "REMINDER: only tool calls do anything — any prose you write is \
discarded and reaches no one. To act you MUST call the matching tool (say / vote / nominate / \
night_action / host_decide / skip_night_action); do not merely describe or announce your move.";

pub fn player_kickoff(display_name: &str, seat: SeatId, game_id: u64, n_players: usize) -> String {
    format!(
        r#"You are playing Blood on the Clocktower: Trouble Brewing as an agent.

## Identity
- Table name: {display_name}
- Seat id: {seat}
- game_id: {game_id}
- Players at table: {n_players}

## MCP tools (server name `botc`) — DISCOVER, then CALL
Your whole game runs through MCP tools on the `botc` server. Your client may **not** list them
upfront — they can be **deferred behind a tool-search step**. If you don't already see a tool you
need, **search for it first, then call it**: use your tool-discovery tool (e.g. `ToolSearch`, or
grok's `search_tool`/`use_tool`) to load the `botc` tools by name, then invoke them (some clients
name them `mcp__botc__<tool>`). Discovering a tool is not the same as calling it — after you find
it, actually call it. Never write the call out as prose/JSON in your reply; that does nothing.
Your token is **already bound** in the MCP proxy — do not invent tokens. Always pass game_id={game_id}.
Typical tools (search for these):
- get_public_state / get_public_log — shared table
- get_private_state — your secret role and private inbox (Drunk: you only see a Townsfolk face)
- get_character_rules / list_characters / list_rules_topics / get_rules_topic — public rules
- say — public chat, **day only** (at night players are asleep and silent; say is rejected at night)
- night_action / day_action / nominate / vote / pass_vote — when it is legal for you
- Actions are **state-gated** by the engine: it rejects anything illegal for the current phase.

## How turns work (READ THIS) — continuous session
You stay in **this process for the whole game**. Do **not** end by chatting without tools and
waiting. The only correct idle is the MCP tool **`await_turn`**.

Loop for the entire game:
1. Call `await_turn` with `game_id={game_id}`.
2. If `status` is **`wake`**: read `prompt` (why you were woken + legal actions). Do those
   actions (`night_action` / `say` / `nominate` / `vote` / …), then call `await_turn` again.
3. If `status` is **`idle`**: immediately call `await_turn` again (server poll budget ended;
   this is normal, not a failure).
4. If `await_turn` **errors or times out**: call `await_turn` again. Wakes are durable — you
   cannot miss a turn by timing out; the same `wake_id` may redeliver until you act.
5. If `status` is **`game_over`**: stop.

Never busy-poll with get_public_state instead of `await_turn`. Never invent other seats' private info.

**Everything you do happens through tool calls — nothing else does.** Any prose you write as an
ordinary reply is **discarded**: no other player and no game engine ever sees it, and it changes
nothing. To speak you MUST call `say` (your words go in its `text` argument); to vote, nominate, or
act at night, call the matching tool. **Never narrate, describe, or summarize what you "would do"
or "have done" — actually call the tool.** A turn where you write a reply but call no tool
accomplishes **nothing** and stalls the whole table waiting on you.

## How to play
1. Call get_private_state and get_public_state once at setup.
2. Read the rules for your face role if needed.
3. Then enter the `await_turn` loop. Talk with `say` only when a wake says so; at night only
   `night_action` when woken for it.
4. Never claim tool access you don't have. Do not try to become host or use host tools.

## Table talk (important)
Nothing forces you to name your character. This is an information game — **what** you reveal and
**when** matters as much as what you know.
- **Anyone may lie** in public chat — good and evil. Treat private Storyteller messages as your
  own facts; treat *other seats' claims* as unproven.
- Lies work best where **no one can check**. Example: “I was never woken last night” proves
  nothing — night wakes are private, so that claim cannot be verified and may be false. Same for
  invented reads, fake bluffs, or a role that only you would know you “have.”
- Announcing a role that sounds hard to kill (Soldier, Mayor, etc.) can read as self-protection —
  evil players want to survive, so a convenient “don't kill me” claim often draws suspicion.
- Announcing a powerful good role can put a night-kill target on your back.
- Holding information can look evasive; dumping everything early can hand evil a map of who to
  kill or who to frame. Soft claims, partial reads, and silence are all legal tools.
- Before you `say` anything, ask what others will *do* with it: who they nominate, who the demon
  kills, what story they build. Good players may also **mislead** about their identity when that
  protects the town — bluffing is not reserved for evil.

Start now: read your private role and the rules for it, plan how you'll play (no night_action yet),
then call **`await_turn`** and stay in that loop until `game_over`.
"#,
        display_name = display_name,
        seat = seat.0,
        game_id = game_id,
        n_players = n_players,
    )
}

pub fn host_kickoff(game_id: u64, n_players: usize, st_choice_mode: &str) -> String {
    format!(
        r#"You are the Storyteller (host) for a Blood on the Clocktower: Trouble Brewing game run for model agents.

## Identity
- You are the HOST, not a player seat.
- game_id: {game_id}
- Players: {n_players}
- st_choice_mode: {st_choice_mode}

## MCP tools (server name `botc`)
Your host token is bound in the MCP proxy. Always pass game_id={game_id}.
Key tools:
- get_host_state — full grimoire (true roles, poison, pending wakes, pending_host)
- get_public_state / get_public_log
- start_game — if still lobby (prefer random bag unless scripted)
- skip_night_action — default pending player wake OR pending host decision
- host_decide — mayor_redirect / starpass_pick / night_info (when pending_host)
- host_queue_lie — optional false info text for disabled roles
- open_nominations / close_vote / end_nominations — day pacing when needed
- st_announce — public ST announcements

## How the game is driven (READ THIS) — continuous session
You stay in **this process for the whole game**. Idle **only** via MCP **`await_turn`**.

**You are only woken when a genuine Storyteller decision (or stall fallback) is needed** — you do
not pace the day; nominations/votes/day-end are engine-driven. When `await_turn` returns
`status=wake`, resolve that one item, then call `await_turn` again.

**Everything you do happens through tool calls — nothing else does.** Prose you write as an ordinary
reply is **discarded**: no one sees it and it changes nothing. Never narrate or summarize what you
"would do" — actually call the host tool. A turn with no tool call accomplishes **nothing** and
stalls the game.

- `pending_host` = night_info → the named player must learn something tonight. Author it with
  `host_decide`, or `skip_night_action` to let the engine pick valid default info. **If unsure, skip.**
- `pending_host` = mayor_redirect / starpass_pick → make the choice with `host_decide` (or skip).
- a `pending` player wake that's stuck → `skip_night_action` after a reasonable wait.
- Reading rules (`get_rules_topic`, `get_character_rules`) is fine, but **do not spend the turn only
  reading** — finish by calling a host tool that changes the game.
- Never leak grimoire contents in `st_announce` or public chat.

Loop:
1. `await_turn` → on `idle`/timeout/error, call `await_turn` again; on `game_over`, stop.
2. On `wake`: follow `prompt` (usually `get_host_state` then `host_decide` / `skip_night_action` /
   `open_nominations` / `end_nominations` / `close_vote` as specified).
3. Never leak the grimoire in `st_announce`. Do not search the filesystem for game source.

If unsure on night_info, `skip_night_action` is fine.

## This session start
1. `start_game` if still in lobby (the TUI may already have started).
2. Enter the `await_turn` loop and stay there until `game_over`.
"#,
        game_id = game_id,
        n_players = n_players,
        st_choice_mode = st_choice_mode,
    )
}

pub fn player_tick(display_name: &str, seat: SeatId, game_id: u64, public_summary: &str) -> String {
    format!(
        r#"Continue playing Trouble Brewing as {display_name} (seat {seat}), game_id={game_id}.

Public snapshot for context (also re-fetch with tools):
{public_summary}

Call get_private_state and get_public_state. If you can act (awaiting night/day action, vote open, discussion), do so. Otherwise say something useful or wait. Do not invent private info about other seats.

{tools_reminder}
"#,
        display_name = display_name,
        seat = seat.0,
        game_id = game_id,
        public_summary = public_summary,
        tools_reminder = ACT_VIA_TOOLS,
    )
}

pub fn host_tick(game_id: u64, public_summary: &str, host_hint: &str) -> String {
    format!(
        r#"Continue as Storyteller for game_id={game_id}.

Public snapshot:
{public_summary}

Host hint:
{host_hint}

Call get_host_state. Resolve pending_host / stuck wakes / stalled day. Advance the game.

{tools_reminder}
"#,
        game_id = game_id,
        public_summary = public_summary,
        host_hint = host_hint,
        tools_reminder = ACT_VIA_TOOLS,
    )
}

/// After a headless process exits mid-game, resume with this prompt so the agent
/// re-enters the `await_turn` loop (durable wakes redeliver).
pub fn reconnect_await_loop(display_name: &str, game_id: u64) -> String {
    format!(
        r#"You are still {display_name} in Trouble Brewing game_id={game_id}. Your previous
process ended; this is a resume of the same session.

Immediately call **`await_turn`** with game_id={game_id} and stay in the await_turn loop:
wake → act → await_turn; idle/timeout/error → await_turn again; game_over → stop.
Do not re-do orientation; do not invent private info.
"#,
        display_name = display_name,
        game_id = game_id,
    )
}

/// Targeted host prompt for a single scheduled turn: says exactly what the engine
/// is waiting on, so the host acts instead of re-inspecting and idling.
pub fn host_task_tick(
    game_id: u64,
    task: &HostTask,
    public_summary: &str,
    host_hint: &str,
) -> String {
    let action = match task {
        HostTask::StartGame => {
            "The game is in **lobby**. Call `start_game` (prefer a random bag unless scripted), \
             then begin the first night."
                .to_string()
        }
        HostTask::ResolveDecision { detail, .. } => format!(
            "The night is paused waiting on **one** Storyteller decision from you:\n\n\
             {detail}\n\n\
             Call `get_host_state` if you want the exact details, then make that **one** decision \
             (`host_decide` or `skip_night_action`). If unsure, `skip_night_action` always works and \
             immediately advances the night. Never reveal the grimoire in public."
        ),
        HostTask::AdvanceNight => {
            "It is night with no pending player wake and no pending decision. Advance the night: \
             call `get_host_state`, then `skip_night_action` to move to the next wake. Keep advancing \
             until a player must act or the day opens."
                .to_string()
        }
        HostTask::SkipStuckWake { seat } => format!(
            "Player **seat {}** was woken but has not submitted a night action for several turns — \
             it is holding up the night. First call `get_host_state` and confirm the pending wake is \
             still seat {}; if it is, call `skip_night_action` to apply the engine default and advance. \
             (If a different item is pending by then, resolve THAT one instead — `skip_night_action` \
             applies to whatever is currently pending.)",
            seat.0, seat.0
        ),
        HostTask::CloseVoting => {
            "An open **vote has stalled** (a voter is not acting). Call `close_vote` to tally it \
             now — anyone who has not voted counts as a 'no'. The engine then continues the day \
             automatically."
                .to_string()
        }
        HostTask::EndDay { in_discussion } => {
            if *in_discussion {
                // Single action only: opening Nominations lets concurrent player
                // sessions take nominate turns. Bundling end_nominations in the same
                // wake collapses that window before models can act.
                "The table has finished its discussion rounds and **nobody nominated** during \
                 talk. Open the nomination stage so each living seat gets a turn: call \
                 **`open_nominations` only**, then call `await_turn` again. Do **not** call \
                 `end_nominations` in this wake — the harness will re-wake you to end the day \
                 after everyone has had a nomination chance (or after a stall)."
                    .to_string()
            } else {
                "Every player has had their chance to nominate and the table is done — end the \
                 day. Call **`end_nominations` only**. The engine executes the vote leader (if any), \
                 announces it, and moves everyone into the night automatically."
                    .to_string()
            }
        }
    };
    format!(
        r#"You are the Storyteller for game_id={game_id}. You were woken for ONE reason.

## Why you were woken
{action}

## How to check the state (optional, quick)
`get_host_state` shows the full grimoire, the pending wake, and any pending decision. The snapshot
below is the public view. Reading rules (`get_rules_topic`, `get_character_rules`) is allowed.

## What ends your turn
Make the host tool call(s) above, optionally `st_announce` one short public line, then call
**`await_turn` again**. Do NOT free-text conclude and sit idle; do NOT narrate the day.

## Public snapshot
{public_summary}

## Host hint
{host_hint}

Everything you need is in your **MCP tools** (server `botc`) — never read or search for game source
files or run shell commands to find logic. Never reveal the grimoire in public. Always pass
game_id={game_id}.

{ACT_VIA_TOOLS}
"#,
    )
}

/// Targeted player prompt for a single scheduled turn.
pub fn player_task_tick(
    display_name: &str,
    seat: SeatId,
    game_id: u64,
    task: &PlayerTask,
    public_summary: &str,
) -> String {
    let seat = seat.0;
    // Each turn spells out: why woken, the legal actions (with exact arg shapes),
    // and what ends the turn — so the model never has to guess the protocol.
    let (why, actions) = match task {
        PlayerTask::NightWake { prompt } => (
            format!(
                "It is night and **you are being woken for your night action**. The Storyteller \
                 asks:\n\n> {prompt}\n\nEveryone else is asleep — there is no talking at night."
            ),
            "- `night_action` with your choice, e.g. `{\"game_id\": {gid}, \"payload\": {\"target\": <seat number>}}` \
             (two-target abilities: `{\"a\": <seat>, \"b\": <seat>}`; follow the wake prompt above). \
             This is the ONLY action available to you right now — submit it and you're done."
                .to_string(),
        ),
        PlayerTask::Discuss {
            round,
            last_round,
            directed_reply,
        } => (
            if *directed_reply {
                "It is **day — open discussion**, and **someone just publicly addressed you** \
                 (`say` with `to` your seat). Everyone can still read the message; you are woken \
                 immediately so you can answer. This is an **extra** turn and does **not** replace \
                 your fair share of talk rounds.\n\n\
                 Check `get_public_state` → `directed_say` (cap + per-seat sent/received counts) \
                 before you direct a reply — do not target a seat that is already at the receive cap, \
                 and do not send if you are at the send cap."
                    .to_string()
            } else {
                format!(
                    "It is **day — open discussion**, and it is **your turn to speak** (talk round \
                     {n}{last}). Players speak one at a time around the table; everything said so far \
                     is in the snapshot below. When the table is done talking, the day moves to \
                     nominations and an execution vote.\n\n\
                     You are **not** required to claim a role. Choose what (if anything) to reveal with \
                     timing in mind: a “safe” claim can look like someone dodging death; a strong town \
                     claim can paint a target; holding out can look shady if the table needs your read. \
                     Weigh what others will do with your words — nominations, night kills, who they trust. \
                     Soft claims, partial info, redirects, and even a deliberate misclaim are all on the \
                     table when they serve your win condition.\n\n\
                     **Everyone can lie**, especially about things the table cannot audit. A seat saying \
                     they were never woken at night is free to invent that — night contact is private, so \
                     no one else can confirm or deny it. Do not treat unverifiable claims as hard fact; \
                     you may also use such claims yourself when it helps your team.\n\n\
                     Optional: publicly address one seat with `say.to` to wake them immediately (still \
                     fully public). Check `directed_say` on public state for the cap and counts first.",
                    n = round + 1,
                    last = if *last_round {
                        ", the FINAL talk round — after this the day moves on"
                    } else {
                        ""
                    }
                )
            },
            "- `say` `{\"game_id\": {gid}, \"text\": \"<what you tell the table>\"}` — **required this \
             turn**: advance the social game (press a contradiction, float a theory, answer a question, \
             drop a careful read, or deliberately stay vague). Be concrete about *someone or something* \
             — not a content-free filler line — but do **not** treat a full role claim as mandatory.\n\
             - Optional `to`: `{\"game_id\": {gid}, \"text\": \"...\", \"to\": <seat>}` — same public \
             message, but **immediately wakes that seat** (not a whisper). Max 6 directed sends and 6 \
             directed receives per player per discussion day; refuse targets already at the receive cap \
             (see `get_public_state.directed_say`).\n\
             - `nominate` `{\"game_id\": {gid}, \"target\": <seat number>}` — optional: if you already \
             want someone executed, this opens the vote (once per day). Engine house rule: nominating \
             usually records your yes automatically (Butler may still need a later vote turn)."
                .to_string(),
        ),
        PlayerTask::Nominate => (
            "It is **day — nominations are open**, and it is **your turn to nominate** (or pass). \
             If nobody nominates, the day ends with no execution."
                .to_string(),
            "- `nominate` `{\"game_id\": {gid}, \"target\": <seat number>}` — puts that player up for \
             an execution vote (you may nominate once per day). Engine house rule: this usually \
             records your **yes** automatically so you skip the vote turn; the Butler ability can \
             delay that until the master has voted yes (then you will be offered a Vote turn).\n\
             - OR `say` `{\"game_id\": {gid}, \"text\": \"...\"}` — state briefly why you're passing. \
             Do one of the two."
                .to_string(),
        ),
        PlayerTask::Vote {
            nomination,
            tally,
            can_pass,
            nominator_yes,
        } => (
            format!(
                "It is **day — a vote is in progress**: {nomination}. {nominator_note}Votes are \
                 counted one seat at a time around the table and **it is your turn to vote**.\n\n\
                 Votes so far: {tally}. \
                 If the yes votes reach **at least half of the living players**, the nominee goes to \
                 the block and is executed at day's end.",
                nominator_note = if *nominator_yes {
                    "The nominator's yes is already in the tally. "
                } else {
                    "The nominator has not (yet) recorded a yes — count only what is in the tally. "
                }
            ),
            if *can_pass {
                "- `vote` `{\"game_id\": {gid}, \"nominee\": <seat number>, \"support\": true|false}` — \
                 cast your **one** vote on this nomination. You are dead: voting YES spends your \
                 single ghost vote for the rest of the game.\n\
                 - OR `pass_vote` `{\"game_id\": {gid}}` — abstain WITHOUT spending your ghost vote \
                 (usually right unless this execution really matters to you).\n\
                 Do exactly one — a second vote/pass on the **same** nomination is rejected. You may \
                 `say` one short line first to explain your vote."
                    .to_string()
            } else {
                "- `vote` `{\"game_id\": {gid}, \"nominee\": <seat number>, \"support\": true|false}` — \
                 cast your **one** vote now (yes or no). A second vote on this nomination is rejected. \
                 You are alive, so you must vote — `pass_vote` is for dead players only.\n\
                 You may `say` one short line first to explain your vote."
                    .to_string()
            },
        ),
    };
    let actions = actions.replace("{gid}", &game_id.to_string());
    format!(
        r#"Continue as {display_name} (seat {seat}) in Trouble Brewing, game_id={game_id}.

## Why you were woken
{why}

## How to check the state (optional, quick)
- `get_private_state` — your secret role and private Storyteller messages.
- `get_public_state` / `get_public_log` — the table, who's alive, full chat history.
The snapshot below usually has everything you need already.

## Actions you may take this turn
{actions}

## What ends your turn
Take your action(s) above, then call **`await_turn` again** — do not free-text conclude and sit idle.
Do NOT poll for other players: `await_turn` blocks until your next wake (or soft-idles; re-call it).

## Public snapshot
{public_summary}

Always pass game_id={game_id}. Never invent private info about other seats or claim tools you lack.

{ACT_VIA_TOOLS}
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_guardrails(p: &str) {
        // The four guardrail sections every turn prompt must carry (#turn-order).
        assert!(p.contains("## Why you were woken"), "missing why: {p}");
        assert!(
            p.contains("## How to check the state"),
            "missing state: {p}"
        );
        assert!(
            p.contains("## Actions you may take this turn"),
            "missing actions: {p}"
        );
        assert!(
            p.contains("## What ends your turn"),
            "missing turn-end: {p}"
        );
        // JSON examples render with single braces and a substituted game id.
        assert!(!p.contains("{{"), "doubled braces leaked: {p}");
        assert!(!p.contains("{gid}"), "gid placeholder not substituted: {p}");
        assert!(
            p.contains("\"game_id\": 7"),
            "example missing real game_id: {p}"
        );
    }

    #[test]
    fn player_turn_prompts_carry_guardrails() {
        let vote = player_task_tick(
            "P2",
            SeatId(2),
            7,
            &PlayerTask::Vote {
                nomination: "P0 nominated P1 for execution".into(),
                tally: "P0 YES — 1 of 6 eligible have acted".into(),
                can_pass: false,
                nominator_yes: true,
            },
            "phase: Day",
        );
        assert_guardrails(&vote);
        assert!(vote.contains("your turn to vote"));
        assert!(vote.contains("P0 YES"));
        assert!(
            vote.contains("nominator's yes is already"),
            "should state nominator yes when flag true: {vote}"
        );
        // Living voters must NOT be offered pass_vote (engine rejects it).
        assert!(!vote.contains("OR `pass_vote`"), "{vote}");
        assert!(vote.contains("at least half of the living players"));
        let ghost = player_task_tick(
            "P5",
            SeatId(5),
            7,
            &PlayerTask::Vote {
                nomination: "P0 nominated P1 for execution".into(),
                tally: "no votes yet — 5 eligible voters".into(),
                can_pass: true,
                nominator_yes: false,
            },
            "phase: Day",
        );
        assert!(
            ghost.contains("has not (yet) recorded a yes") || ghost.contains("not (yet)"),
            "should not invent nominator yes: {ghost}"
        );
        assert!(
            ghost.contains("pass_vote"),
            "dead voters may abstain: {ghost}"
        );
        assert!(ghost.contains("ghost vote"));

        let talk = player_task_tick(
            "P1",
            SeatId(1),
            7,
            &PlayerTask::Discuss {
                round: 1,
                last_round: true,
                directed_reply: false,
            },
            "phase: Day",
        );
        assert_guardrails(&talk);
        assert!(talk.contains("FINAL talk round"));
        // Discussion should not pressure a mandatory role claim.
        assert!(
            talk.to_lowercase().contains("not") && talk.to_lowercase().contains("required"),
            "discuss should discourage mandatory claims: {talk}"
        );
        assert!(
            !talk.contains("claim or dispute a role"),
            "old claim-push wording should be gone: {talk}"
        );

        let wake = player_task_tick(
            "P3",
            SeatId(3),
            7,
            &PlayerTask::NightWake {
                prompt: "Choose a player to poison".into(),
            },
            "phase: Night",
        );
        assert_guardrails(&wake);
        assert!(wake.contains("Choose a player to poison"));

        let nom = player_task_tick("P0", SeatId(0), 7, &PlayerTask::Nominate, "phase: Day");
        assert_guardrails(&nom);
        assert!(nom.contains("your turn to nominate"));
    }

    #[test]
    fn host_end_day_prompt_matches_stage() {
        let from_disc = host_task_tick(
            7,
            &HostTask::EndDay {
                in_discussion: true,
            },
            "s",
            "h",
        );
        assert!(from_disc.contains("open_nominations"), "{from_disc}");
        assert!(
            from_disc.contains("open_nominations` only")
                || from_disc.to_lowercase().contains("do **not** call"),
            "must forbid bundling end_nominations: {from_disc}"
        );
        assert!(
            !from_disc.contains("then `end_nominations`")
                && !from_disc.contains("then end_nominations"),
            "must not instruct open+end in one wake: {from_disc}"
        );
        let from_noms = host_task_tick(
            7,
            &HostTask::EndDay {
                in_discussion: false,
            },
            "s",
            "h",
        );
        assert!(from_noms.contains("end_nominations"));
        assert!(!from_noms.contains("open_nominations"), "{from_noms}");
    }

    #[test]
    fn player_kickoff_covers_info_hygiene() {
        let p = player_kickoff("P0", SeatId(0), 1, 5);
        assert!(p.contains("## Table talk"), "{p}");
        assert!(p.to_lowercase().contains("nothing forces"), "{p}");
        assert!(
            p.to_lowercase().contains("mislead") || p.to_lowercase().contains("bluff"),
            "{p}"
        );
        assert!(p.to_lowercase().contains("anyone may lie"), "{p}");
        assert!(
            p.to_lowercase().contains("never woken") || p.to_lowercase().contains("never"),
            "kickoff should mention unverifiable night-wake claims: {p}"
        );
    }

    #[test]
    fn discuss_prompt_warns_unverifiable_lies() {
        let talk = player_task_tick(
            "P1",
            SeatId(1),
            7,
            &PlayerTask::Discuss {
                round: 0,
                last_round: false,
                directed_reply: false,
            },
            "phase: Day",
        );
        assert!(talk.to_lowercase().contains("everyone can lie"), "{talk}");
        assert!(talk.to_lowercase().contains("never woken"), "{talk}");
    }
}
