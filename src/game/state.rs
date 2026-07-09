//! `Game` aggregate: seats, phase, comms handles, win state.

use std::collections::VecDeque;

use crate::auth::{Actor, Token, TokenBook};
use crate::comms::{PrivateInboxes, PrivateMessage, PublicEvent, PublicLog};
use crate::error::GameError;
use crate::game::ids::{GameId, SeatId};
use crate::game::night::{build_first_night_queue, PendingWake};
use crate::game::phase::{NightStep, Phase, Winner};
use crate::game::seat::Seat;
use crate::game::setup::{build_bag, setup_markers, validate_fixed_assignments, StartOpts};
use crate::game::st_policy::{
    HostDecision, MayorRedirectChoice, PendingHostDecision, RegistrationMode,
};
use crate::rng::SeededRng;
use crate::roles::{Character, CharacterType, Team};

/// Trouble Brewing table size (inclusive).
pub const MIN_PLAYERS: usize = 5;
pub const MAX_PLAYERS: usize = 15;

impl From<Winner> for Team {
    fn from(w: Winner) -> Self {
        match w {
            Winner::Good => Team::Good,
            Winner::Evil => Team::Evil,
        }
    }
}

/// Public projection of a seat (no characters).
#[derive(Debug, Clone)]
pub struct PublicSeatView {
    pub id: SeatId,
    pub name: String,
    pub alive: bool,
    pub ghost_vote_available: bool,
}

#[derive(Debug)]
pub struct Game {
    pub id: GameId,
    pub seed: u64,
    /// Per-game CSPRNG salt mixed into every RNG substream. Host-only; never player views.
    pub secret_salt: u64,
    pub rng: SeededRng,
    pub phase: Phase,
    pub seats: Vec<Seat>,
    pub tokens: TokenBook,
    pub public_log: PublicLog,
    pub private_inboxes: PrivateInboxes,
    pub winner: Option<Winner>,
    /// Fortune Teller red herring seat, if FT in play.
    pub red_herring: Option<SeatId>,
    /// Three not-in-play good characters shown to Imp (7+).
    pub demon_bluffs: Vec<Character>,
    /// Concrete night steps for the current night (FirstNight or Night).
    pub night_queue: Vec<NightStep>,
    /// Index into `night_queue` (mirrors phase cursor while in night).
    pub night_cursor: usize,
    /// Active player wake, if the night machine is waiting on a choice.
    pub pending_night: Option<PendingWake>,
    /// Host-only night decision (Mayor bounce / starpass); pauses night_tick.
    pub pending_host: Option<PendingHostDecision>,
    /// Spy/Recluse registration policy for this game.
    pub registration_mode: RegistrationMode,
    /// FIFO free-text lies for disabled info roles (host-authored; else seeded-random).
    pub host_lie_queue: VecDeque<String>,
    /// Seats that died during the current night (demon kill, etc.).
    pub deaths_tonight: Vec<SeatId>,
    /// Seat executed during the current day, if any (Undertaker eligibility).
    pub executed_today: Option<SeatId>,
    /// Living seats that have already nominated today.
    pub day_nominators: Vec<SeatId>,
    /// Seats that have already been nominated today.
    pub day_nominees: Vec<SeatId>,
    /// Open nomination with in-progress vote window, if any.
    pub current_nomination: Option<crate::game::day::OpenNomination>,
    /// Closed nominations today (yes tallies for leader comparison).
    pub closed_nominations: Vec<crate::game::day::ClosedNomination>,
}

/// Result of opening a lobby: host token + player tokens in seat order.
pub struct CreateGameResult {
    pub game: Game,
    pub host_token: Token,
    pub player_tokens: Vec<Token>,
}

/// Alias kept for older sketch call sites; prefer [`CreateGameResult`].
pub type Lobby = CreateGameResult;

/// Per-seat assignment at start. Drunk **must** include a Townsfolk face.
#[derive(Debug, Clone)]
pub struct RoleAssignment {
    pub seat: SeatId,
    pub true_character: Character,
    /// Required when `true_character` is Drunk; ignored otherwise (should be None).
    pub believed_character: Option<Character>,
}

impl RoleAssignment {
    pub fn normal(seat: SeatId, true_character: Character) -> Self {
        Self {
            seat,
            true_character,
            believed_character: None,
        }
    }

    pub fn drunk(seat: SeatId, townsfolk_face: Character) -> Result<Self, GameError> {
        if townsfolk_face.character_type() != CharacterType::Townsfolk {
            return Err(GameError::IllegalAction(
                "Drunk face must be a Townsfolk character",
            ));
        }
        if townsfolk_face == Character::Drunk {
            return Err(GameError::IllegalAction("Drunk face cannot be Drunk"));
        }
        Ok(Self {
            seat,
            true_character: Character::Drunk,
            believed_character: Some(townsfolk_face),
        })
    }
}

impl Game {
    /// Open a lobby with seats and issued tokens. `id` is a placeholder until [`crate::store::GameStore::insert`].
    ///
    /// Generates a fresh CSPRNG [`Self::secret_salt`] so substreams cannot be reconstructed from
    /// `seed` and public labels alone.
    pub fn create(player_names: Vec<String>, seed: u64) -> Result<CreateGameResult, GameError> {
        Self::create_with_salt(player_names, seed, rand::random())
    }

    /// Like [`Self::create`] but with an explicit secret salt (deterministic tests / replay).
    pub fn create_with_salt(
        player_names: Vec<String>,
        seed: u64,
        secret_salt: u64,
    ) -> Result<CreateGameResult, GameError> {
        let n = player_names.len();
        if n < MIN_PLAYERS || n > MAX_PLAYERS {
            return Err(GameError::BadRequest(
                "player count must be between 5 and 15 inclusive",
            ));
        }

        let mut tokens = TokenBook::default();
        let host_token = tokens.issue_host();
        let mut player_tokens = Vec::with_capacity(n);
        let seats: Vec<Seat> = player_names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let seat = SeatId(i as u8);
                player_tokens.push(tokens.issue_player(seat));
                Seat::new(seat, name)
            })
            .collect();

        Ok(CreateGameResult {
            game: Self {
                id: GameId(0),
                seed,
                secret_salt,
                rng: SeededRng::from_seed_and_salt(seed, secret_salt),
                phase: Phase::Lobby,
                seats,
                tokens,
                public_log: PublicLog::default(),
                private_inboxes: PrivateInboxes::default(),
                winner: None,
                red_herring: None,
                demon_bluffs: Vec::new(),
                night_queue: Vec::new(),
                night_cursor: 0,
                pending_night: None,
                pending_host: None,
                registration_mode: RegistrationMode::Random,
                host_lie_queue: VecDeque::new(),
                deaths_tonight: Vec::new(),
                executed_today: None,
                day_nominators: Vec::new(),
                day_nominees: Vec::new(),
                current_nomination: None,
                closed_nominations: Vec::new(),
            },
            host_token,
            player_tokens,
        })
    }

    pub fn public_seats(&self) -> Vec<PublicSeatView> {
        self.seats
            .iter()
            .map(|s| PublicSeatView {
                id: s.id,
                name: s.display_name.clone(),
                alive: s.alive,
                ghost_vote_available: s.ghost_vote_available,
            })
            .collect()
    }

    /// Public-only speech. No recipient field by design.
    pub fn say(&mut self, seat: SeatId, text: String) -> Result<(), GameError> {
        let name = self
            .seats
            .iter()
            .find(|s| s.id == seat)
            .map(|s| s.display_name.clone())
            .ok_or(GameError::NoSuchSeat)?;
        self.public_log.push(PublicEvent::Chat { seat, name, text });
        Ok(())
    }

    pub fn st_announce(&mut self, text: impl Into<String>) {
        self.public_log.push(PublicEvent::StorytellerAnnounce {
            text: text.into(),
        });
    }

    /// Deliver Storyteller private info to one seat only.
    pub fn st_whisper(&mut self, seat: SeatId, msg: PrivateMessage) {
        self.private_inboxes.push(seat, msg);
    }

    /// Player-facing character for private role tools. Never returns Drunk when a face is set.
    pub fn player_facing_character(&self, seat: SeatId) -> Option<Character> {
        self.seats
            .iter()
            .find(|s| s.id == seat)
            .and_then(|s| s.visible_character())
    }

    /// Host-only start: build bag (or apply fixed assignments), brief seats, enter First Night.
    pub fn start_game(&mut self, host: &Token, opts: StartOpts) -> Result<(), GameError> {
        match self.tokens.resolve(host) {
            Some(Actor::Host) => {}
            _ => return Err(GameError::Unauthorized),
        }
        if !matches!(self.phase, Phase::Lobby) {
            return Err(GameError::WrongPhase);
        }

        let n = self.seats.len() as u8;
        let (mut assignments, mut red_herring, mut demon_bluffs) =
            if let Some(fixed) = opts.assignments {
                validate_fixed_assignments(self.seats.len(), &fixed)?;
                let bag_set: Vec<Character> = fixed.iter().map(|a| a.true_character).collect();
                let (red_herring, demon_bluffs) = setup_markers(&self.rng, n, &fixed, &bag_set);
                (fixed, red_herring, demon_bluffs)
            } else {
                let bag = build_bag(&self.rng, n)?;
                (bag.assignments, bag.red_herring, bag.demon_bluffs)
            };

        let bag_set: Vec<Character> = assignments.iter().map(|a| a.true_character).collect();

        if let Some(faces) = opts.drunk_faces {
            apply_drunk_face_overrides(&mut assignments, &bag_set, &faces)?;
            // Auto-generated bluffs may collide with overridden faces — re-filter.
            demon_bluffs = refilter_demon_bluffs_for_faces(
                &self.rng,
                &bag_set,
                &assignments,
                demon_bluffs,
            );
        }

        if let Some(rh) = opts.red_herring {
            red_herring = Some(validate_red_herring_override(&assignments, &bag_set, rh)?);
        }

        if let Some(bluffs) = opts.demon_bluffs {
            demon_bluffs = validate_demon_bluffs_override(&bag_set, &assignments, bluffs)?;
        }

        self.registration_mode = opts.registration_mode;
        self.red_herring = red_herring;
        self.demon_bluffs = demon_bluffs;
        self.apply_assignments_and_brief(assignments)
    }

    /// Assign characters and push private `YouAre` using **player-facing** identity only.
    ///
    /// Does not require host token (used by tests and as the final step of [`Self::start_game`]).
    pub fn start_game_assign(
        &mut self,
        assignments: Vec<RoleAssignment>,
    ) -> Result<(), GameError> {
        for a in &assignments {
            if a.true_character == Character::Drunk {
                let face = a.believed_character.ok_or(GameError::IllegalAction(
                    "Drunk assignment requires a Townsfolk believed_character face",
                ))?;
                if face.character_type() != CharacterType::Townsfolk {
                    return Err(GameError::IllegalAction(
                        "Drunk face must be a Townsfolk character",
                    ));
                }
            } else if a.believed_character.is_some() {
                return Err(GameError::IllegalAction(
                    "believed_character only valid for Drunk",
                ));
            }
        }
        self.apply_assignments_and_brief(assignments)
    }

    fn apply_assignments_and_brief(
        &mut self,
        assignments: Vec<RoleAssignment>,
    ) -> Result<(), GameError> {
        for a in &assignments {
            let seat = self
                .seats
                .iter_mut()
                .find(|s| s.id == a.seat)
                .ok_or(GameError::NoSuchSeat)?;
            seat.true_character = Some(a.true_character);
            seat.is_drunk_outsider = a.true_character == Character::Drunk;
            seat.believed_character = a.believed_character;
        }

        // Snapshot faces for private messages (immutable borrow after mut loop).
        let briefings: Vec<(SeatId, Character, Team)> = self
            .seats
            .iter()
            .filter_map(|seat| {
                let true_c = seat.true_character?;
                let facing = seat.visible_character()?;
                Some((seat.id, facing, true_c.team()))
            })
            .collect();

        for (seat_id, facing, team) in briefings {
            // facing is never Drunk if setup enforced a face.
            debug_assert!(
                facing != Character::Drunk,
                "player-facing identity must not be Drunk"
            );
            self.private_inboxes.push(
                seat_id,
                PrivateMessage::YouAre {
                    character_label: facing.display_name().to_string(),
                    team,
                    rules_path: facing.rules_doc_path().to_string(),
                    note: None,
                },
            );
        }

        self.deaths_tonight.clear();
        self.executed_today = None;
        self.day_nominators.clear();
        self.day_nominees.clear();
        self.current_nomination = None;
        self.closed_nominations.clear();
        self.pending_night = None;
        self.pending_host = None;
        self.night_queue = build_first_night_queue(self);
        self.night_cursor = 0;
        self.phase = Phase::FirstNight { cursor: 0 };
        self.st_announce("Night falls. Eyes closed. The first night begins.");
        // Auto ST steps (setup, evil briefings, info stubs) until a choice or dawn.
        self.night_tick();
        Ok(())
    }

    /// Sketch helper: plain (seat, character) pairs. Drunk is rejected — use [`RoleAssignment::drunk`].
    pub fn start_game_assign_for_sketch(
        &mut self,
        assignments: Vec<(SeatId, Character)>,
    ) -> Result<(), GameError> {
        let mapped = assignments
            .into_iter()
            .map(|(seat, c)| {
                if c == Character::Drunk {
                    Err(GameError::IllegalAction(
                        "use RoleAssignment::drunk(seat, townsfolk_face) for Drunk",
                    ))
                } else {
                    Ok(RoleAssignment::normal(seat, c))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.start_game_assign(mapped)
    }

    /// Host: push a free-text lie for the next disabled info result (FIFO).
    pub fn host_queue_lie(&mut self, host: &Token, text: String) -> Result<(), GameError> {
        match self.tokens.resolve(host) {
            Some(Actor::Host) => {}
            _ => return Err(GameError::Unauthorized),
        }
        self.host_lie_queue.push_back(text);
        Ok(())
    }

    /// Host: resolve a pending Mayor bounce / starpass decision.
    pub fn host_decide(&mut self, host: &Token, decision: HostDecision) -> Result<(), GameError> {
        match self.tokens.resolve(host) {
            Some(Actor::Host) => {}
            _ => return Err(GameError::Unauthorized),
        }
        self.apply_host_decision(decision)
    }

    /// Pop the next host-authored lie, if any (used by disabled info paths).
    pub fn take_host_lie(&mut self) -> Option<String> {
        self.host_lie_queue.pop_front()
    }

    pub(crate) fn apply_host_decision(
        &mut self,
        decision: HostDecision,
    ) -> Result<(), GameError> {
        let pending = self
            .pending_host
            .clone()
            .ok_or(GameError::IllegalAction("no pending host decision"))?;
        match (pending, decision) {
            (
                PendingHostDecision::MayorRedirect { mayor, .. },
                HostDecision::MayorRedirect { choice },
            ) => {
                self.pending_host = None;
                crate::game::ability::evil::resolve_mayor_host_choice(self, mayor, choice);
                self.advance_after_host_decision();
                Ok(())
            }
            (
                PendingHostDecision::StarpassPick {
                    minions,
                    dead_imp,
                },
                HostDecision::StarpassPick { minion },
            ) => {
                if !minions.contains(&minion) {
                    return Err(GameError::IllegalAction(
                        "starpass pick must be a living minion from the pending list",
                    ));
                }
                self.pending_host = None;
                crate::game::ability::evil::complete_starpass(self, dead_imp, minion);
                self.advance_after_host_decision();
                Ok(())
            }
            _ => Err(GameError::IllegalAction(
                "host decision does not match pending decision type",
            )),
        }
    }

    pub(crate) fn apply_default_host_decision(&mut self) -> Result<(), GameError> {
        let pending = self
            .pending_host
            .clone()
            .ok_or(GameError::IllegalAction("no pending host decision"))?;
        match pending {
            PendingHostDecision::MayorRedirect { .. } => {
                // Skip default: nobody dies. Host may still pick kill_mayor / kill_other.
                self.apply_host_decision(HostDecision::MayorRedirect {
                    choice: MayorRedirectChoice::Nobody,
                })
            }
            PendingHostDecision::StarpassPick { minions, dead_imp } => {
                // Random among living minions (previous default).
                let mut sorted = minions;
                sorted.sort_by_key(|id| id.0);
                let label = format!("starpass:c{}", self.night_cursor);
                let mut rng = self.rng.substream(&label);
                use rand::seq::SliceRandom;
                let minion = *sorted
                    .choose(&mut rng)
                    .ok_or(GameError::IllegalAction("no minions for starpass default"))?;
                let _ = dead_imp;
                self.apply_host_decision(HostDecision::StarpassPick { minion })
            }
        }
    }

    fn advance_after_host_decision(&mut self) {
        // DemonKill player step already cleared pending_night; advance cursor and continue.
        self.night_cursor += 1;
        match &mut self.phase {
            Phase::FirstNight { cursor } => *cursor = self.night_cursor,
            Phase::Night { cursor, .. } => *cursor = self.night_cursor,
            _ => {}
        }
        self.night_tick();
    }
}

fn apply_drunk_face_overrides(
    assignments: &mut [RoleAssignment],
    bag_set: &[Character],
    faces: &[(SeatId, Character)],
) -> Result<(), GameError> {
    let in_bag: std::collections::HashSet<Character> = bag_set.iter().copied().collect();
    for &(seat, face) in faces {
        if face.character_type() != CharacterType::Townsfolk {
            return Err(GameError::IllegalAction(
                "drunk face override must be Townsfolk",
            ));
        }
        if face == Character::Drunk {
            return Err(GameError::IllegalAction("drunk face cannot be Drunk"));
        }
        if in_bag.contains(&face) {
            return Err(GameError::IllegalAction(
                "drunk face override must not be in the bag",
            ));
        }
        let a = assignments
            .iter_mut()
            .find(|a| a.seat == seat)
            .ok_or(GameError::NoSuchSeat)?;
        if a.true_character != Character::Drunk {
            return Err(GameError::IllegalAction(
                "drunk face override only valid for Drunk seats",
            ));
        }
        a.believed_character = Some(face);
    }
    Ok(())
}

fn validate_red_herring_override(
    assignments: &[RoleAssignment],
    bag_set: &[Character],
    rh: SeatId,
) -> Result<SeatId, GameError> {
    if !bag_set.contains(&Character::FortuneTeller) {
        return Err(GameError::IllegalAction(
            "red_herring override requires Fortune Teller in play",
        ));
    }
    let a = assignments
        .iter()
        .find(|a| a.seat == rh)
        .ok_or(GameError::NoSuchSeat)?;
    if a.true_character.team() != Team::Good {
        return Err(GameError::IllegalAction(
            "red_herring must be a good seat",
        ));
    }
    Ok(rh)
}

fn validate_demon_bluffs_override(
    bag_set: &[Character],
    assignments: &[RoleAssignment],
    bluffs: Vec<Character>,
) -> Result<Vec<Character>, GameError> {
    if bluffs.len() != 3 {
        return Err(GameError::IllegalAction(
            "demon_bluffs override must list exactly 3 characters",
        ));
    }
    let in_bag: std::collections::HashSet<Character> = bag_set.iter().copied().collect();
    let drunk_faces: std::collections::HashSet<Character> = assignments
        .iter()
        .filter(|a| a.true_character == Character::Drunk)
        .filter_map(|a| a.believed_character)
        .collect();
    for c in &bluffs {
        if c.team() != Team::Good {
            return Err(GameError::IllegalAction(
                "demon_bluffs must be good characters",
            ));
        }
        if in_bag.contains(c) {
            return Err(GameError::IllegalAction(
                "demon_bluffs must not be in the bag",
            ));
        }
        if drunk_faces.contains(c) {
            return Err(GameError::IllegalAction(
                "demon_bluffs must not match Drunk faces",
            ));
        }
    }
    Ok(bluffs)
}

/// Drop bluffs that match any Drunk face; fill back to 3 from not-in-play good chars.
fn refilter_demon_bluffs_for_faces(
    rng: &crate::rng::SeededRng,
    bag_set: &[Character],
    assignments: &[RoleAssignment],
    bluffs: Vec<Character>,
) -> Vec<Character> {
    use crate::roles::{all_outsiders, all_townsfolk};
    use rand::seq::SliceRandom;

    let drunk_faces: std::collections::HashSet<Character> = assignments
        .iter()
        .filter(|a| a.true_character == Character::Drunk)
        .filter_map(|a| a.believed_character)
        .collect();
    if drunk_faces.is_empty() {
        return bluffs;
    }

    let in_bag: std::collections::HashSet<Character> = bag_set.iter().copied().collect();
    let mut kept: Vec<Character> = bluffs
        .into_iter()
        .filter(|c| !drunk_faces.contains(c))
        .collect();
    let used: std::collections::HashSet<Character> = kept.iter().copied().collect();
    let mut pool: Vec<Character> = all_townsfolk()
        .iter()
        .chain(all_outsiders().iter())
        .copied()
        .filter(|c| !in_bag.contains(c) && !drunk_faces.contains(c) && !used.contains(c))
        .collect();
    let mut brng = rng.substream("demon_bluffs_refilter");
    pool.shuffle(&mut brng);
    for c in pool {
        if kept.len() >= 3 {
            break;
        }
        kept.push(c);
    }
    kept.truncate(3);
    kept
}
