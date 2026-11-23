use sim_host::{PhaseProbe, SimError, SimMetrics, Simulation};

struct Legacy(u64);

impl Simulation for Legacy {
    type Config = ();
    type Command = ();
    fn new(_: u64, _: ()) -> Result<Self, SimError> {
        Ok(Self(0))
    }
    fn step(&mut self, _: &[()]) {
        self.0 += 1;
    }
    fn tick(&self) -> u64 {
        self.0
    }
    fn is_finished(&self) -> bool {
        false
    }
    fn metrics(&self) -> SimMetrics {
        SimMetrics {
            active_entities: 0,
            custom_gauges: vec![],
        }
    }
    fn state_hash(&self) -> u64 {
        self.0
    }
    fn check_invariants(&self) -> Result<(), SimError> {
        Ok(())
    }
}

struct Probe(usize);
impl PhaseProbe for Probe {
    fn enter(&mut self, _: &'static str) {
        self.0 += 1;
    }
    fn exit(&mut self, _: &'static str) {
        self.0 += 1;
    }
}

#[test]
fn required_methods_alone_are_sufficient() {
    let mut sim = Legacy::new(0, ()).unwrap();
    let mut probe = Probe(0);
    sim.step_profiled(&[], &mut probe);
    assert_eq!(sim.tick(), 1);
    assert_eq!(probe.0, 0);
    assert_eq!(sim.subsystem_hashes(), [("all", sim.state_hash())]);
    assert_eq!(sim.describe_entity(0), None);
    assert_eq!(Legacy::snapshot_version(), 0);
    assert_eq!(sim.content_fingerprint(), 0);
    assert!(matches!(
        sim.save_snapshot(&mut Vec::new()),
        Err(SimError::Unsupported("snapshots"))
    ));
    assert!(matches!(
        Legacy::load_snapshot(&[]),
        Err(SimError::Unsupported("snapshots"))
    ));
    assert!(matches!(
        sim.debug_dump(&mut String::new()),
        Err(SimError::Unsupported("debug_dump"))
    ));
}
