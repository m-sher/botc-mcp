//! System / kickoff prompts for headless Grok player and host agents.

use crate::game::SeatId;

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

## Storyteller policy
- Prefer coherent, playable false info; do not hard-code a winner.
- When pending_host is set, resolve it (host_decide or skip_night_action).
- When a player night wake is stuck, skip_night_action after a reasonable wait.
- Day often auto-opens/ends; intervene if stalled.
- Night info in host-first: you author private results via host_decide night_info text, or skip for engine default.
- Never leak grimoire contents in st_announce or public chat.

## Loop
1. get_host_state + get_public_state
2. If lobby: start_game
3. Resolve pending_host or pending wakes
4. Keep the game moving until Ended
5. Summarize winners privately to yourself; public only legal announcements

Start now: check host state and start the game if needed, then drive the first night.
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
