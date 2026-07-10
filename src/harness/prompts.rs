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
- say — public chat only (no whispers)
- night_action / day_action / nominate / vote / pass_vote — when it is legal for you

## How to play
1. Call get_private_state and get_public_state.
2. Read the rules for your face role if needed.
3. Talk publicly with say when useful.
4. When you are awaiting a night/day action, submit it.
5. Bluff freely in chat; never claim tool access you don't have.
6. Do not try to become host or use host tools.
7. Keep playing until the game ends (winner in public state) or you are told to stop.

Start now: inspect your private state and introduce yourself in public chat briefly.
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

## How the engine drives the night (READ THIS)
The engine runs the night order for you. At any moment `get_host_state` shows **at most one**
thing waiting on you: either `pending_host` (a Storyteller decision — e.g. what a player learns
tonight) or a `pending` player wake. **Your entire job each turn is to resolve that one item**, then
you're done — the engine advances to the next step and the harness calls you again. You do **not**
run the night order yourself, and you do **not** need to read or search for any game source code /
files — the MCP tools are the whole interface.

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
             it is holding up the night. As Storyteller, resolve it now: call `skip_night_action` to \
             apply the engine default for that wake and advance the cursor. Do not wait further.",
            seat.0
        ),
        HostTask::PaceDiscussion => {
            "It is **day (discussion)**. Let players talk. When discussion has run enough (or it is \
             stalling), call `open_nominations` to move to nominations. Make only legal `st_announce` \
             statements; never reveal roles."
                .to_string()
        }
        HostTask::ManageNominations => {
            "It is **day (nominations)** with no open vote. Ensure nominations are open \
             (`open_nominations` if needed); if nominations are exhausted or the day should end, \
             `end_nominations`. Move the day forward."
                .to_string()
        }
        HostTask::CloseVoting => {
            "A nomination's **vote is open**. Once the eligible players have voted (or the window \
             should close), call `close_vote` to tally it, then continue the day \
             (`end_nominations` when done)."
                .to_string()
        }
    };
    format!(
        r#"You are the Storyteller for game_id={game_id}. It is your turn to act.

## Do this now
{action}

## Public snapshot
{public_summary}

## Host hint
{host_hint}

Everything you need is in your **MCP tools** (server `botc`) — you do **not** need to read or search
for any game source code or files on disk; do not run shell commands to look for game logic. This
turn resolves exactly one thing and the harness will call you again for the next step. Reading rules
(`get_rules_topic`, `get_character_rules`) is fine, but **finish this turn by calling a host tool**
that changes the game (`host_decide`, `skip_night_action`, `open_nominations`, `st_announce`, …).
Always pass game_id={game_id}.
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
    let action = match task {
        PlayerTask::NightWake { prompt } => format!(
            "**You are being woken for your night action.** The Storyteller asks:\n\n> {prompt}\n\n\
             Call `get_private_state` to see your role and the exact choice, then submit your \
             `night_action` now with a legal target. If your ability is passive/info-only, acknowledge \
             as the prompt requires."
        ),
        PlayerTask::Discuss => {
            "It is **day (discussion)** and it is your turn to speak. Call `get_public_state` \
             (and `get_private_state`) for context, then use `say` to share a read, a claim, or a \
             question. If you want someone executed, you may `nominate` them. Say something concrete \
             this turn — don't just inspect."
                .to_string()
        }
        PlayerTask::Nominate => {
            "It is **day (nominations)** and it is your turn. If you want a player executed, \
             `nominate` them now (you may nominate at most once per day); otherwise `say` your \
             reasoning for holding off. Act this turn."
                .to_string()
        }
        PlayerTask::Vote { nomination } => format!(
            "**A nomination is open** — {nomination}. It is your turn to vote. Decide and cast it now: \
             `vote` (yes/no) if you have a vote available, or `pass_vote` to abstain. Dead players \
             have only one ghost vote for the whole game — spend it deliberately."
        ),
    };
    format!(
        r#"Continue as {display_name} (seat {seat}) in Trouble Brewing, game_id={game_id}. It is your turn.

## Do this now
{action}

## Public snapshot (re-fetch with tools for detail)
{public_summary}

Always pass game_id={game_id}. Never invent private info about other seats or claim tools you lack.
"#,
    )
}
