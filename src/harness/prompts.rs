//! System / kickoff prompts for headless Grok player and host agents.

use crate::game::SeatId;
use crate::harness::scheduler::{HostTask, PlayerTask};

pub fn player_kickoff(
    display_name: &str,
    seat: SeatId,
    game_id: u64,
    n_players: usize,
) -> String {
    format!(
        r#"You are playing Blood on the Clocktower: Trouble Brewing as an agent.

## Identity
- Table name: {display_name}
- Seat id: {seat}
- game_id: {game_id}
- Players at table: {n_players}

## MCP tools (server name `botc`)
You have MCP tools for this game. Discover them with search_tool if needed, then use_tool.
Your token is **already bound** in the MCP proxy — do not invent tokens. Always pass game_id={game_id}.
Typical tools:
- get_public_state / get_public_log — shared table
- get_private_state — your secret role and private inbox (Drunk: you only see a Townsfolk face)
- get_character_rules / list_characters / list_rules_topics / get_rules_topic — public rules
- say — public chat, **day only** (at night players are asleep and silent; say is rejected at night)
- night_action / day_action / nominate / vote / pass_vote — when it is legal for you
- Actions are **state-gated** by the engine: it rejects anything illegal for the current phase.

## How turns work (READ THIS)
The harness runs the table **one turn at a time** and wakes you only when it is YOUR turn, with
instructions saying exactly why you were woken (your night action / your turn to speak / your turn
to nominate / your turn to vote) and which actions are legal. Take the requested action(s), then
stop responding — your turn ends when you stop calling tools, and you'll be woken again with fresh
state. Never wait or poll for other players inside a turn.

## How to play
1. Call get_private_state and get_public_state.
2. Read the rules for your face role if needed.
3. Talk publicly with `say` **during the day** when woken to speak; at night, only submit your
   `night_action` when woken for it.
4. Bluff freely in chat; never claim tool access you don't have.
5. Do not try to become host or use host tools.

Start now: read your private role and the rules for it, and plan how you'll play. This is a
setup-only turn — the first night is starting, so there is no talking. If your character acts
tonight, you will be woken for it in a separate turn right after this one; do not submit a
night_action now. Finish by briefly noting (to yourself) your strategy, then stop.
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

## How the game is driven (READ THIS)
The engine runs the game and the harness wakes agents one at a time, each with instructions.
**You are only woken when a genuine Storyteller decision is needed** — you do not pace the day,
players' nominations auto-open voting, votes auto-close, and the day auto-ends into night. At any
moment `get_host_state` shows **at most one** thing waiting on you: either `pending_host` (a
Storyteller decision — e.g. what a player learns tonight) or a `pending` player wake. **Your entire
job each turn is to resolve that one item**, then stop — the harness calls you again when you're
next needed. You do **not** run the night order yourself, and you do **not** need to read or search
for any game source code / files — the MCP tools are the whole interface.

- `pending_host` = night_info → the named player must learn something tonight. Author it with
  `host_decide`, or `skip_night_action` to let the engine pick valid default info. **If unsure, skip.**
- `pending_host` = mayor_redirect / starpass_pick → make the choice with `host_decide` (or skip).
- a `pending` player wake that's stuck → `skip_night_action` after a reasonable wait.
- Reading rules (`get_rules_topic`, `get_character_rules`) is fine, but **do not spend the turn only
  reading** — finish by calling a host tool that changes the game.
- Never leak grimoire contents in `st_announce` or public chat.

## This turn
1. `start_game` if still in lobby.
2. `get_host_state`; resolve the single pending item (`host_decide` or `skip_night_action`).
3. Stop. The harness will call you for the next step.
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
"#,
        display_name = display_name,
        seat = seat.0,
        game_id = game_id,
        public_summary = public_summary,
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
"#,
        game_id = game_id,
        public_summary = public_summary,
        host_hint = host_hint,
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
                "The table has finished its discussion rounds and **nobody nominated** — end the \
                 day. Call `open_nominations`, then `end_nominations`. The engine tallies any \
                 execution and moves everyone into the night automatically."
                    .to_string()
            } else {
                "Every player has had their chance to nominate and the table is done — end the \
                 day. Call `end_nominations`. The engine executes the vote leader (if any), \
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
Make the host tool call(s) above, optionally `st_announce` one short public line, then stop
responding. Your turn ends when you stop calling tools; the harness wakes you ONLY when the next
Storyteller decision is needed — players and the engine drive everything else (nominations
auto-open, votes auto-close, the day auto-ends). Do NOT narrate the day, do NOT wait or poll.

## Public snapshot
{public_summary}

## Host hint
{host_hint}

Everything you need is in your **MCP tools** (server `botc`) — never read or search for game source
files or run shell commands to find logic. Never reveal the grimoire in public. Always pass
game_id={game_id}.
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
        PlayerTask::Discuss { round, last_round } => (
            format!(
                "It is **day — open discussion**, and it is **your turn to speak** (talk round \
                 {n}{last}). Players speak one at a time around the table; everything said so far \
                 is in the snapshot below. When the table is done talking, the day moves to \
                 nominations and an execution vote.",
                n = round + 1,
                last = if *last_round {
                    ", the FINAL talk round — after this the day moves on"
                } else {
                    ""
                }
            ),
            "- `say` `{\"game_id\": {gid}, \"text\": \"<what you tell the table>\"}` — **required this \
             turn**: share a read, claim or dispute a role, answer questions, react. Say something \
             concrete; don't just observe.\n\
             - `nominate` `{\"game_id\": {gid}, \"target\": <seat number>}` — optional: if you already \
             want someone executed, this immediately opens the vote on them (once per day)."
                .to_string(),
        ),
        PlayerTask::Nominate => (
            "It is **day — nominations are open**, and it is **your turn to nominate** (or pass). \
             If nobody nominates, the day ends with no execution."
                .to_string(),
            "- `nominate` `{\"game_id\": {gid}, \"target\": <seat number>}` — puts that player up for \
             an execution vote (you may nominate once per day).\n\
             - OR `say` `{\"game_id\": {gid}, \"text\": \"...\"}` — state briefly why you're passing. \
             Do one of the two."
                .to_string(),
        ),
        PlayerTask::Vote {
            nomination,
            tally,
            can_pass,
        } => (
            format!(
                "It is **day — a vote is in progress**: {nomination}. Votes are counted one seat \
                 at a time around the table and **it is your turn to vote**.\n\nVotes so far: {tally}. \
                 If the yes votes reach **at least half of the living players**, the nominee goes to \
                 the block and is executed at day's end."
            ),
            if *can_pass {
                "- `vote` `{\"game_id\": {gid}, \"nominee\": <seat number>, \"support\": true|false}` — \
                 cast your vote. You are dead: voting YES spends your single ghost vote for the rest \
                 of the game.\n\
                 - OR `pass_vote` `{\"game_id\": {gid}}` — abstain WITHOUT spending your ghost vote \
                 (usually right unless this execution really matters to you).\n\
                 Do exactly one. You may `say` one short line first to explain your vote."
                    .to_string()
            } else {
                "- `vote` `{\"game_id\": {gid}, \"nominee\": <seat number>, \"support\": true|false}` — \
                 cast your vote now. You are alive, so you must vote yes or no — `pass_vote` is for \
                 dead players only and will be rejected.\n\
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
Take your action(s) above, then stop responding — your turn is over when you stop calling tools.
Do NOT wait for, poll for, or try to respond to other players in this turn: the harness runs the
table one turn at a time and will wake you again with fresh state when it is next your turn.

## Public snapshot
{public_summary}

Always pass game_id={game_id}. Never invent private info about other seats or claim tools you lack.
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_guardrails(p: &str) {
        // The four guardrail sections every turn prompt must carry (#turn-order).
        assert!(p.contains("## Why you were woken"), "missing why: {p}");
        assert!(p.contains("## How to check the state"), "missing state: {p}");
        assert!(p.contains("## Actions you may take this turn"), "missing actions: {p}");
        assert!(p.contains("## What ends your turn"), "missing turn-end: {p}");
        // JSON examples render with single braces and a substituted game id.
        assert!(!p.contains("{{"), "doubled braces leaked: {p}");
        assert!(!p.contains("{gid}"), "gid placeholder not substituted: {p}");
        assert!(p.contains("\"game_id\": 7"), "example missing real game_id: {p}");
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
            },
            "phase: Day",
        );
        assert_guardrails(&vote);
        assert!(vote.contains("your turn to vote"));
        assert!(vote.contains("P0 YES"));
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
            },
            "phase: Day",
        );
        assert!(ghost.contains("pass_vote"), "dead voters may abstain: {ghost}");
        assert!(ghost.contains("ghost vote"));

        let talk = player_task_tick(
            "P1",
            SeatId(1),
            7,
            &PlayerTask::Discuss { round: 1, last_round: true },
            "phase: Day",
        );
        assert_guardrails(&talk);
        assert!(talk.contains("FINAL talk round"));

        let wake = player_task_tick(
            "P3",
            SeatId(3),
            7,
            &PlayerTask::NightWake { prompt: "Choose a player to poison".into() },
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
        let from_disc = host_task_tick(7, &HostTask::EndDay { in_discussion: true }, "s", "h");
        assert!(from_disc.contains("open_nominations"), "{from_disc}");
        assert!(from_disc.contains("end_nominations"));
        let from_noms = host_task_tick(7, &HostTask::EndDay { in_discussion: false }, "s", "h");
        assert!(from_noms.contains("end_nominations"));
        assert!(!from_noms.contains("Call `open_nominations`"));
    }
}
