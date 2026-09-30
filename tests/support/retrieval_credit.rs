use super::*;
use wasm_bindgen_test::wasm_bindgen_test;

async fn notified(listener: event_listener::EventListener) {
    async_std::future::timeout(Duration::from_millis(500), listener)
        .await
        .expect("credit change must wake an existing waiter");
}

#[wasm_bindgen_test]
async fn credit_notifications_coalesce_bursts_and_skip_idle_timers() {
    assert_eq!(crate::CREDIT_AVAILABLE.total_listeners(), 0);
    crate::notify_credit_available();
    assert!(
        async_std::future::timeout(Duration::from_millis(200), crate::CREDIT_AVAILABLE.listen(),)
            .await
            .is_err(),
        "no listener means no timer can wake a later waiter"
    );

    let changed = crate::CREDIT_AVAILABLE.listen();
    let other = crate::CREDIT_AVAILABLE.listen();
    for _ in 0..100 {
        crate::notify_credit_available();
    }
    pin_mut!(changed);
    assert!(
        futures::poll!(&mut changed).is_pending(),
        "bursts must be batched"
    );
    async_std::future::timeout(Duration::from_millis(500), changed)
        .await
        .unwrap();
    notified(other).await;
    assert!(
        async_std::future::timeout(Duration::from_millis(200), crate::CREDIT_AVAILABLE.listen(),)
            .await
            .is_err(),
        "one burst must not leave additional notifications scheduled"
    );
    assert_eq!(crate::CREDIT_AVAILABLE.total_listeners(), 0);
}

#[wasm_bindgen_test]
async fn credit_notifications_cover_threshold_refresh_surplus_and_rollback() {
    use futures::FutureExt;
    let wings = crate::Wings::default();
    let account = crate::get_or_create_accounting_peer(&wings, PeerId::random()).await;
    account.lock().await.connection_id = Some(libp2p::swarm::ConnectionId::new_unchecked(1));
    let (refresh, _incoming) = mpsc::bounded(8);

    // Both prices must wake: waking just one can strand the eligible request.
    let expensive = crate::CREDIT_AVAILABLE.listen();
    let cheap = crate::CREDIT_AVAILABLE.listen();
    crate::set_payment_threshold(&account, price(31), &refresh).await;
    notified(expensive).await;
    assert_eq!(cheap.now_or_never(), Some(()));
    assert!(reserve(&account, price(0)).await.is_none());
    assert!(reserve(&account, price(31)).await.is_some());
    let unchanged_credit = crate::CREDIT_AVAILABLE.listen();
    apply_credit(&account, price(31), &refresh).await;
    assert_eq!(unchanged_credit.now_or_never(), None);

    crate::set_payment_threshold(&account, 40_000, &refresh).await;
    assert!(reserve(&account, 10_000).await.is_some());
    let refreshed = crate::CREDIT_AVAILABLE.listen();
    crate::apply_refreshment(&account, 5_000).await;
    notified(refreshed).await;
    assert_eq!(account.lock().await.reserve, 10_000);
    assert_eq!(account.lock().await.balance, 5_000);
    let rolled_back = crate::CREDIT_AVAILABLE.listen();
    cancel_reserve(&account, 10_000).await;
    notified(rolled_back).await;

    crate::apply_refreshment(&account, 15_000).await;
    assert!(reserve(&account, 10_000).await.is_some());
    let surplus_spent = crate::CREDIT_AVAILABLE.listen();
    apply_credit(&account, 10_000, &refresh).await;
    notified(surplus_spent).await;
    assert_eq!(account.lock().await.balance, 0);
    let unchanged_credit = crate::CREDIT_AVAILABLE.listen();
    cancel_reserve(&account, 0).await;
    crate::apply_refreshment(&account, 0).await;
    crate::set_payment_threshold(&account, 40_000, &refresh).await;
    assert_eq!(unchanged_credit.now_or_never(), None);
}

#[wasm_bindgen_test]
async fn priced_peer_publication_wakes_existing_credit_waiters() {
    let node = crate::Weeb3::new();
    let peer = PeerId::random();
    let connection_id = libp2p::swarm::ConnectionId::new_unchecked(4);
    let (attempt, _ready) = crate::try_mark_connection_attempt(&node.wings, &peer)
        .await
        .unwrap();
    crate::record_physical_connection_established(
        &node.wings.physical_connections,
        &peer,
        connection_id,
    );
    let account = crate::get_or_create_accounting_peer(&node.wings, peer).await;
    account.lock().await.connection_id = Some(connection_id);
    account.lock().await.threshold = price(31);
    node.wings.connected_peers.lock().await.insert(
        peer,
        crate::PeerFile {
            peer_id: peer,
            overlay: [0; 32],
            beneficiary: Default::default(),
            connection_attempt_id: attempt,
            connection_id,
        },
    );
    let mut skiplist = HashMap::new();
    let selected = select_retrieve_peer(
        &[0; 32],
        &node.wings.overlay_peers,
        &node.wings.accounting_peers,
        &node.wings.physical_connections,
        &mut skiplist,
    )
    .await
    .0;
    assert!(selected.is_none());
    let changed = crate::CREDIT_AVAILABLE.listen();
    node.promote_priced_peer(&node.wings, peer).await;
    // Publication happened before the listener was polled: it cannot be lost.
    notified(changed).await;
    let selected = select_retrieve_peer(
        &[0; 32],
        &node.wings.overlay_peers,
        &node.wings.accounting_peers,
        &node.wings.physical_connections,
        &mut skiplist,
    )
    .await
    .0
    .unwrap();
    assert_eq!(selected.peer, peer);
    cancel_reserve(&selected.accounting, selected.price).await;
}

#[wasm_bindgen_test]
async fn no_peer_wait_exits_on_cancellation_and_admission_close() {
    let peers = Arc::default();
    let accounting = Arc::default();
    let physical = Arc::default();
    let (refresh, _incoming) = mpsc::bounded(1);
    for (cancel_stream, paused) in [(false, false), (true, false), (false, true), (true, true)] {
        let registry = crate::RetrieveCancelRegistry::default();
        let cancel = registry.register("credit-wait".into(), 1).await;
        let admission = RetrieveAdmission::new();
        let pause = Arc::new(TransferPause::default());
        if paused {
            assert!(pause.toggle());
        }
        let behaviour = libp2p_stream::Behaviour::new();
        let result = retrieve_chunk(
            &[0; 32],
            behaviour.new_control(),
            &peers,
            &accounting,
            &physical,
            &refresh,
            cancel,
            Some(admission.clone()),
            None,
            Some(pause),
        );
        pin_mut!(result);
        assert!(futures::poll!(&mut result).is_pending());
        if cancel_stream {
            registry.register("credit-wait".into(), 2).await;
        } else {
            admission.close();
        }
        assert!(
            async_std::future::timeout(Duration::from_millis(500), result)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[wasm_bindgen_test]
async fn managed_attempt_promotes_only_after_timeout_and_settles_closed_admission() {
    for stale_session in [true, false] {
        let wings = crate::Wings::default();
        let peer = PeerId::random();
        let connection = libp2p::swarm::ConnectionId::new_unchecked(8);
        crate::record_physical_connection_established(
            &wings.physical_connections,
            &peer,
            connection,
        );
        let accounting = crate::get_or_create_accounting_peer(&wings, peer).await;
        let req_price = price(31);
        {
            let mut account = accounting.lock().await;
            account.connection_id = Some(connection);
            account.threshold = req_price;
        }
        assert_eq!(reserve(&accounting, req_price).await, Some(connection));
        let selected = ReservedRetrievePeer {
            peer,
            price: req_price,
            accounting: accounting.clone(),
            session: OutboundProtocolSession::capture(
                peer,
                connection,
                wings.physical_connections.clone(),
            )
            .unwrap(),
        };
        if stale_session {
            crate::record_physical_connection_closed(
                &wings.physical_connections,
                &peer,
                connection,
            );
        }

        let admission =
            RetrieveAdmission::new_with_attempt_limit(RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS);
        assert!(admission.try_claim_physical_attempt());
        let demand = SharedRetrieveHedgeDemand::new(RetrieveHedgeDemand::DistinctShardManaged);
        let promoted = demand.wait_until_ordinary();
        pin_mut!(promoted);
        assert!(futures::poll!(&mut promoted).is_pending());
        // Keeping this behaviour alive but unpolled leaves the real stream-open future pending.
        let behaviour = libp2p_stream::Behaviour::new();
        let (refresh, _incoming) = mpsc::bounded(1);
        let attempt = retrieve_attempt(
            selected,
            vec![0; 32],
            behaviour.new_control(),
            refresh,
            Some(admission.clone()),
            Some(demand.clone()),
        );
        pin_mut!(attempt);
        if !stale_session {
            assert!(futures::poll!(&mut attempt).is_pending());
            assert_eq!(accounting.lock().await.reserve, req_price);
            assert_eq!(demand.current(), RetrieveHedgeDemand::DistinctShardManaged);
            assert_eq!(admission.timed_out_physical_attempts(), Some(0));
        }
        admission.close();
        if !stale_session {
            assert!(futures::poll!(&mut attempt).is_pending());
            assert_eq!(accounting.lock().await.reserve, req_price);
        }
        let maximum = if stale_session {
            500
        } else {
            RETRIEVE_ATTEMPT_TIMEOUT_MS + 2_000
        };
        assert!(
            async_std::future::timeout(Duration::from_millis(maximum), attempt)
                .await
                .unwrap()
                .complete(&mut HashMap::new())
                .is_none()
        );
        drop(behaviour);

        assert_eq!(
            demand.current(),
            if stale_session {
                RetrieveHedgeDemand::DistinctShardManaged
            } else {
                RetrieveHedgeDemand::Ordinary
            }
        );
        assert_eq!(futures::poll!(&mut promoted).is_ready(), !stale_session);
        assert_eq!(
            admission.timed_out_physical_attempts(),
            Some(usize::from(!stale_session))
        );
        assert_eq!(admission.claimed_physical_attempts(), Some(1));
        assert_eq!(admission.confirmed_empty_physical_attempts(), Some(0));
        assert!(!admission.is_open());
        assert!(!admission.try_claim_physical_attempt());
        assert_eq!(accounting.lock().await.reserve, 0);
        assert_eq!(accounting.lock().await.balance, 0);
    }
}

#[wasm_bindgen_test]
async fn replacement_peer_retries_before_or_after_old_failure_without_a_second_credit_wake() {
    use libp2p::swarm::{NetworkBehaviour, ToSwarm, behaviour::{DialFailure, FromSwarm}};

    async fn next_dial(behaviour: &mut libp2p_stream::Behaviour) -> PeerId {
        let event = libp2p::futures::future::poll_fn(|cx| behaviour.poll(cx)).await;
        let ToSwarm::Dial { opts, .. } = event else { panic!("expected a retrieval stream dial") };
        opts.get_peer_id().unwrap()
    }

    for ready_first in [true, false] {
        let wings = crate::Wings::default();
        let peer = PeerId::random();
        let denied = PeerId::random();
        let old = ConnectionId::new_unchecked(31);
        let replacement = ConnectionId::new_unchecked(32);
        let (refresh, _incoming) = mpsc::bounded(1);
        let old_account = crate::get_or_create_accounting_peer(&wings, peer).await;
        for (id, overlay, connection, threshold) in [
            (peer, [0; 32], old, price(31)),
            (denied, [0xff; 32], ConnectionId::new_unchecked(33), 0),
        ] {
            let account = crate::get_or_create_accounting_peer(&wings, id).await;
            account.lock().await.connection_id = Some(connection);
            account.lock().await.threshold = threshold;
            crate::record_physical_connection_established(&wings.physical_connections, &id, connection);
            Arc::make_mut(&mut *wings.overlay_peers.lock().await).insert(overlay, id);
        }
        let admission = RetrieveAdmission::new_with_attempt_limit(2);
        let mut behaviour = libp2p_stream::Behaviour::new();
        let retrieval = retrieve_chunk(
            &[0; 32], behaviour.new_control(), &wings.overlay_peers, &wings.accounting_peers,
            &wings.physical_connections, &refresh, None, Some(admission.clone()), None, None,
        );
        let recovery = async {
            assert_eq!(next_dial(&mut behaviour).await, peer);
            async_std::task::sleep(Duration::from_millis(RETRIEVE_HEDGE_AFTER_MS + 50)).await;
            assert_eq!(crate::CREDIT_AVAILABLE.total_listeners(), 1);
            assert_eq!(admission.claimed_physical_attempts(), Some(1));
            // Prevent reserve release from supplying a second, accidental recovery notification.
            old_account.lock().await.reserve = 0;
            let fail = |behaviour: &mut libp2p_stream::Behaviour, connection_id| {
                behaviour.on_swarm_event(FromSwarm::DialFailure(DialFailure {
                    peer_id: Some(peer), connection_id, error: &libp2p::swarm::DialError::NoAddresses,
                }));
            };
            if !ready_first {
                fail(&mut behaviour, old);
                async_std::task::sleep(Duration::ZERO).await;
            }
            crate::record_physical_connection_closed(&wings.physical_connections, &peer, old);
            crate::record_physical_connection_established(&wings.physical_connections, &peer, replacement);
            wings.accounting_peers.lock().await.remove(&peer);
            let account = crate::get_or_create_accounting_peer(&wings, peer).await;
            account.lock().await.connection_id = Some(replacement);
            account.lock().await.threshold = price(31);
            // The same event emitted by priced-peer publication, with no later credit wake.
            let published = crate::CREDIT_AVAILABLE.listen();
            crate::notify_credit_available();
            notified(published).await;
            if ready_first {
                async_std::task::sleep(Duration::ZERO).await;
                assert_eq!(admission.claimed_physical_attempts(), Some(1));
                assert_eq!(crate::CREDIT_AVAILABLE.total_listeners(), 1);
                fail(&mut behaviour, old);
            }
            assert_eq!(
                async_std::future::timeout(Duration::from_secs(1), next_dial(&mut behaviour)).await.unwrap(),
                peer,
                "the replacement must dispatch even when readiness preceded the old failure",
            );
            assert_eq!(admission.claimed_physical_attempts(), Some(2));
            assert!(!admission.is_open());
            fail(&mut behaviour, replacement);
        };
        let (result, ()) = async_std::future::timeout(
            Duration::from_secs(4), libp2p::futures::future::join(retrieval, recovery),
        ).await.unwrap();
        assert!(result.is_empty());
        assert_eq!(old_account.lock().await.reserve, 0);
        assert_eq!(wings.accounting_peers.lock().await[&peer].lock().await.reserve, 0);
        assert_eq!(crate::CREDIT_AVAILABLE.total_listeners(), 0);
    }
}
