use rand::Rng;

#[test]
fn same_seed_same_substream_bytes() {
    let mut a = botc_mcp::rng::SeededRng::from_seed(42);
    let mut b = botc_mcp::rng::SeededRng::from_seed(42);
    let x: u64 = a.substream("washerwoman").gen();
    let y: u64 = b.substream("washerwoman").gen();
    assert_eq!(x, y);
    let z: u64 = a.substream("false_info").gen();
    assert_ne!(x, z); // different labels diverge (with overwhelming probability)
}
