/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Unit tests and scripted memory probes for engine admission.

use std::collections::VecDeque;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use super::Admission;
use super::types::{
    CapacityRefused, GIB, LOAD_SETTLE, MIB, MemoryProbe, Reservation, Shortfall, default_reserve,
};

/// A probe that reports `snapshots` in turn and then the last one again, for tests.
pub fn scripted_probe(snapshots: Vec<(u64, u64)>) -> MemoryProbe {
    let queue = Mutex::new(VecDeque::from(snapshots));
    Arc::new(move || {
        let mut queue = queue.lock().unwrap();
        if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().copied()
        }
    })
}

/// A host with `used` of `total` GiB in use.
fn host(used: u64, total: u64) -> (u64, u64) {
    ((total - used) * GIB, total * GIB)
}

/// With 80 of 100 GiB in use there are 5 GiB under the 85% limit: two engines of 2 GiB
/// fit, a third does not, and a returned reservation makes room again.
#[test]
fn reservations_of_loads_in_flight_count_against_the_limit() {
    let admission = Arc::new(Admission::with_probe(
        scripted_probe(vec![host(80, 100)]),
        2048,
        LOAD_SETTLE,
    ));
    let first = admission.try_reserve("rust").expect("first fits");
    let _second = admission.try_reserve("go").expect("second fits");
    assert_eq!(admission.reserved_bytes(), 4 * GIB);
    let refused = admission
        .try_reserve("rust")
        .expect_err("third passes the limit");
    assert_eq!(refused.reserved, 4 * GIB);
    assert_eq!(refused.loads, 2);
    assert_eq!(refused.excess(), GIB);
    drop(first);
    assert_eq!(admission.reserved_bytes(), 2 * GIB);
    let _third = admission
        .try_reserve("rust")
        .expect("fits once one is returned");
}

/// Admissions racing on one snapshot never share its headroom: of eight simultaneous
/// requests for 2 GiB with 5 GiB free under the limit, exactly two are admitted.
#[test]
fn simultaneous_admissions_never_overcommit() {
    let admission = Arc::new(Admission::with_probe(
        scripted_probe(vec![host(80, 100)]),
        2048,
        LOAD_SETTLE,
    ));
    let start = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let admission = Arc::clone(&admission);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                admission.try_reserve("rust").ok()
            })
        })
        .collect();
    let admitted: Vec<Reservation> = workers
        .into_iter()
        .filter_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(admitted.len(), 2);
    assert_eq!(admission.reserved_bytes(), 4 * GIB);
    drop(admitted);
    assert_eq!(admission.reserved_bytes(), 0);
    assert_eq!(admission.ledger().loads, 0);
}

/// A host already past the limit takes nothing; one whose memory is unknown takes all.
#[test]
fn a_host_past_the_limit_admits_nothing_and_an_unknown_one_everything() {
    let full = Arc::new(Admission::with_probe(
        scripted_probe(vec![host(86, 100)]),
        1,
        LOAD_SETTLE,
    ));
    assert!(full.try_reserve("text").is_err());
    let unknown = Arc::new(Admission::unbounded());
    let held: Vec<_> = (0..64)
        .map(|_| unknown.try_reserve("rust").expect("unknown host admits"))
        .collect();
    assert_eq!(unknown.reserved_bytes(), 64 * default_reserve("rust"));
    drop(held);
}

#[test]
fn engines_reserve_by_kind_unless_one_size_is_configured() {
    let defaults = Admission::host(0);
    assert_eq!(defaults.reserve_for("rust"), 4 * GIB);
    assert_eq!(defaults.reserve_for("cpp"), GIB);
    assert_eq!(defaults.reserve_for("text"), 256 * MIB);
    assert_eq!(Admission::host(512).reserve_for("rust"), 512 * MIB);
}

/// A configured reservation past any host saturates instead of overflowing: a known host
/// refuses every new engine with the usual message, an unknown one still admits, and
/// returning one of two such reservations leaves the other counted in full.
#[test]
fn a_reservation_past_any_host_saturates_and_refuses() {
    let full = Arc::new(Admission::with_probe(
        scripted_probe(vec![host(10, 100)]),
        u64::MAX,
        LOAD_SETTLE,
    ));
    assert_eq!(full.reserve_for("rust"), u64::MAX);
    let shortfall = full
        .try_reserve("rust")
        .expect_err("nothing holds u64::MAX bytes");
    assert!(shortfall.excess() > 0);
    let text = CapacityRefused {
        shortfall,
        reclaimed: 0,
    }
    .to_string();
    assert!(text.starts_with("capacity: "), "{text}");
    assert_eq!(full.reserved_bytes(), 0);

    let unknown = Arc::new(Admission::with_probe(
        Arc::new(|| None),
        u64::MAX,
        LOAD_SETTLE,
    ));
    let mut held: Vec<_> = (0..2)
        .map(|_| unknown.try_reserve("rust").expect("unknown host admits"))
        .collect();
    assert_eq!(unknown.reserved_bytes(), u64::MAX);
    drop(held.pop());
    assert_eq!(
        unknown.reserved_bytes(),
        u64::MAX,
        "the other is still held"
    );
    assert_eq!(unknown.ledger().loads, 1);
    drop(held);
    assert_eq!(unknown.reserved_bytes(), 0);
}

/// The refusal names capacity as the cause, the figures, and both ways forward.
#[test]
fn a_refusal_says_capacity_and_how_to_go_on() {
    let refused = CapacityRefused {
        shortfall: Shortfall {
            engine: "rust".to_string(),
            available: 16 * GIB,
            total: 100 * GIB,
            reserved: 4 * GIB,
            loads: 1,
            needed: 4 * GIB,
        },
        reclaimed: 2,
    };
    let text = refused.to_string();
    for part in [
        "capacity: this node has no memory for a new rust engine",
        "memory 84% used",
        "4.0 GiB held for 1 load(s) in flight",
        "the limit is 85% of 100.0 GiB",
        "2 idle engine(s) were unloaded",
        "keep answering",
        "Retry in a few minutes",
        "another node",
    ] {
        assert!(text.contains(part), "{part:?} missing from {text}");
    }
}

/// A reservation outlives its load by the settle time, and is then returned by a timer,
/// not by a poll.
#[tokio::test]
async fn a_reservation_is_returned_after_its_load_settles() {
    let admission = Arc::new(Admission::with_probe(
        scripted_probe(vec![host(10, 100)]),
        1024,
        Duration::from_millis(100),
    ));
    admission
        .try_reserve("rust")
        .unwrap()
        .release_after_settling();
    assert_eq!(
        admission.reserved_bytes(),
        GIB,
        "held while the load settles"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(admission.reserved_bytes(), 0);
    admission
        .try_reserve("rust")
        .unwrap()
        .release_after_settling();
    let unbounded = Arc::new(Admission::unbounded());
    unbounded
        .try_reserve("rust")
        .unwrap()
        .release_after_settling();
    assert_eq!(
        unbounded.reserved_bytes(),
        0,
        "no settle time, returned at once"
    );
}
