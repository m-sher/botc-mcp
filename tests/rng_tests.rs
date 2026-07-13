use rand::Rng;

#[test]
fn same_seed_same_substream_bytes() {
    let a = botc_mcp::rng::SeededRng::from_seed(42);
    let b = botc_mcp::rng::SeededRng::from_seed(42);
    let x: u64 = a.substream("washerwoman").gen();
    let y: u64 = b.substream("washerwoman").gen();
    assert_eq!(x, y);
    let z: u64 = a.substream("false_info").gen();
    assert_ne!(x, z); // different labels diverge (with overwhelming probability)
}

#[test]
fn substream_with_salt_differs_from_salt_zero() {
    let plain = botc_mcp::rng::SeededRng::from_seed_and_salt(42, 0);
    let salted = botc_mcp::rng::SeededRng::from_seed_and_salt(42, 0xDEAD_BEEF);
    let x: u64 = plain.substream("washerwoman").gen();
    let y: u64 = salted.substream("washerwoman").gen();
    assert_ne!(
        x, y,
        "public label + seed alone must not match salted substream"
    );
}

#[test]
fn create_game_assigns_nonzero_secret_salt() {
    // Salt is CSPRNG; vanishingly unlikely to be 0, but seed may still be fixed.
    let lobby = botc_mcp::game::Game::create(
        vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
        99,
    )
    .unwrap();
    assert_eq!(lobby.game.seed, 99);
    assert_eq!(lobby.game.rng.salt(), lobby.game.secret_salt);
    // create_with_salt keeps determinism for evals
    let fixed = botc_mcp::game::Game::create_with_salt(
        vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
        99,
        12345,
    )
    .unwrap();
    assert_eq!(fixed.game.secret_salt, 12345);
    assert_eq!(fixed.game.rng.salt(), 12345);
}
