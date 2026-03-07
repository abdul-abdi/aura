use aura_overlay::easing::AuraEasing;

#[test]
fn test_breathe_cycle() {
    let start = AuraEasing::breathe(0.0);
    let peak = AuraEasing::breathe(0.5);
    let end = AuraEasing::breathe(1.0);
    assert!(start < 0.01, "Breathe should start near 0");
    assert!(peak > 0.99, "Breathe should peak near 1");
    assert!(end < 0.01, "Breathe should end near 0");
}

#[test]
fn test_drift_eases_out() {
    assert!(AuraEasing::drift(0.0) < 0.01);
    assert!(AuraEasing::drift(1.0) > 0.99);
    assert!(AuraEasing::drift(0.5) > 0.5);
}

#[test]
fn test_materialize_s_curve() {
    assert!(AuraEasing::materialize(0.0) < 0.01);
    assert!(AuraEasing::materialize(1.0) > 0.99);
    assert!(AuraEasing::materialize(0.25) < AuraEasing::drift(0.25));
}

#[test]
fn test_dissolve_fast_start() {
    assert!(AuraEasing::dissolve(0.0) > 0.99);
    assert!(AuraEasing::dissolve(1.0) < 0.01);
    assert!(AuraEasing::dissolve(0.3) < 0.75);
}

#[test]
fn test_pulse_exponential_decay() {
    let t0 = AuraEasing::pulse(0.0);
    let t1 = AuraEasing::pulse(0.5);
    let t2 = AuraEasing::pulse(1.0);
    assert!(t0 > 0.99, "Pulse starts at 1");
    assert!(t1 < t0, "Pulse decays");
    assert!(t2 < t1, "Pulse continues decaying");
    assert!(t2 < 0.01, "Pulse near zero at end");
}
