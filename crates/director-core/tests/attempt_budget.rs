use psf_guard_director_core::program::Program;

#[test]
fn successor_budget_never_refills_or_changes_quality_credit() {
    let p: Program = serde_json::from_str(include_str!("fixtures/execution-program.json")).unwrap();
    let mut g = p.assignment.goals[0].clone();
    g.pending = 1;
    g.carry_attempt_budget(2, 1);
    assert_eq!(g.attempts_remaining, 1);
    assert_eq!((g.accepted, g.pending), (0, 1));
    g.carry_attempt_budget(100, 0);
    assert_eq!(g.attempts_remaining, 1);
    g.carry_attempt_budget(2, 3);
    assert_eq!(g.attempts_remaining, 0);
}
