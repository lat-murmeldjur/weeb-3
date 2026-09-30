#![allow(dead_code)]

const NETWORK_SOURCE: &str = include_str!("../src/network_conventions.rs");
const RUNTIME_SOURCE: &str = concat!(
    include_str!("../src/lib.rs"),
    include_str!("../src/network_conventions.rs"),
    include_str!("../src/runtime_conventions.rs"),
);

#[path = "../src/accounting.rs"]
mod accounting;
#[path = "../src/events.rs"]
mod events;
#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;
#[path = "support/source.rs"]
pub mod source;

mod connection {
    use crate::accounting::{
        CONNECTION_BUILDUP_LIMIT, REFRESH_RATE, bee_reconnect_delay_seconds,
        connection_dial_capacity_available, connection_population_deficit, price, refreshment_due,
    };
    #[test]
    fn first_usable_connections_do_not_wait_for_the_population_target() {
        assert_eq!(CONNECTION_BUILDUP_LIMIT, 200);
        assert!(connection_dial_capacity_available(0, 0, 0));
        assert!(connection_dial_capacity_available(1, 0, 0));
        assert!(connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT - 1,
            0,
            0
        ));
    }

    #[test]
    fn retrieval_uses_the_shared_overlay_snapshot() {
        let retrieval = include_str!("../src/retrieval.rs");
        let runtime = crate::RUNTIME_SOURCE;
        assert!(
            runtime.contains("type OverlayPeerMap = Arc<Mutex<Arc<BTreeMap<[u8; 32], PeerId>>>>")
        );
        assert!(runtime.contains("overlay_peers: OverlayPeerMap"));
        assert!(retrieval.contains("let peers = peers.lock().await.clone();"));
    }

    #[test]
    fn dial_storm_can_fill_but_never_exceed_the_peer_population() {
        assert!(connection_dial_capacity_available(
            0,
            CONNECTION_BUILDUP_LIMIT - 1,
            0
        ));
        assert!(!connection_dial_capacity_available(
            0,
            CONNECTION_BUILDUP_LIMIT,
            0
        ));
        assert!(!connection_dial_capacity_available(
            1,
            CONNECTION_BUILDUP_LIMIT - 1,
            0
        ));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT,
            0,
            0
        ));
        assert!(!connection_dial_capacity_available(u64::MAX, u64::MAX, 0));

        let runtime = crate::RUNTIME_SOURCE;
        let feeder = crate::source::between(
            runtime,
            "let peer_dial_scheduler =",
            "let swarm_event_loop = async",
        );
        crate::source::assert_contains(feeder, &[
            "VecDeque::<QueuedPeerDial>::new()",
            "HashSet::<(PeerId, Multiaddr)>::new()",
            "try_reserve_connection_capacity(",
            "queue.pop_front()",
            "queue.push_back(candidate)",
            "Box::pin(capacity_changed)",
            "Box::pin(peers_instructions_chan_incoming.recv())",
        ]);
        assert!(!feeder.contains("task::sleep"));
        assert!(feeder.contains("queue_peer_dial_retry("));
        assert!(runtime.contains("mpsc::bounded::<PeerDialInstruction>(PEER_DIAL_INGEST_BATCH)"));
        assert!(!runtime.contains("MAX_QUEUED_PEER_DIALS"));
        assert!(runtime.contains("remove_connection_attempt_for_connection("));
        let failed_dial = crate::source::between(
            runtime,
            "let retryable = !matches!(",
            "SwarmEvent::ConnectionClosed {",
        );
        let removal = failed_dial
            .find("remove_connection_attempt_for_connection")
            .expect("exact dial removal");
        let release = failed_dial
            .find("release_connection_reservation(")
            .expect("dial capacity release");
        let retry = failed_dial
            .find("queue_peer_dial_retry(")
            .expect("retry registration before capacity release");
        assert!(removal < retry && retry < release);
        assert!(runtime.contains("self.swarm.next_event().await"));
        assert!(!runtime.contains("CONNECTION_BUILDUP_SWARM_POLL_MS"));
        assert!(
            runtime.contains(
                ".outbound_timeout(Duration::from_millis(OUTBOUND_CONNECTION_TIMEOUT_MS))"
            )
        );
        assert!(runtime.contains("const OUTBOUND_CONNECTION_TIMEOUT_MS: u64 = 8_000;"));
        assert!(!runtime.contains("FRESH_GOSSIP_DIAL_TIMEOUT_MS"));
        assert!(!feeder.contains("disconnect_peer_id"));
    }

    #[test]
    fn drained_reload_reservations_expose_the_exact_population_deficit() {
        let mut ongoing = CONNECTION_BUILDUP_LIMIT - 55;
        assert_eq!(connection_population_deficit(55, ongoing, 0), 0);
        for _ in 0..ongoing {
            ongoing = ongoing.saturating_sub(1);
        }
        assert_eq!(ongoing, 0);
        assert_eq!(connection_population_deficit(55, ongoing, 0), CONNECTION_BUILDUP_LIMIT - 55);
        assert_eq!(connection_population_deficit(CONNECTION_BUILDUP_LIMIT - 1, 0, 0), 1);
        assert_eq!(connection_population_deficit(CONNECTION_BUILDUP_LIMIT, 0, 0), 0);
        assert_eq!(connection_population_deficit(u64::MAX, u64::MAX, 0), 0);
    }

    #[test]
    fn every_hundred_replacements_reduces_the_target_until_thirty() {
        let targets = [200, 160, 128, 102, 81, 64, 51, 40, 32, 30];
        for (round, target) in targets.into_iter().enumerate() {
            let lost = round as u16 * 100;
            assert_eq!(connection_population_deficit(0, 0, lost), target);
            assert_eq!(connection_population_deficit(0, 0, lost + 99), target);
            assert!(connection_dial_capacity_available(target - 1, 0, lost));
            assert!(!connection_dial_capacity_available(target - 1, 1, lost));
            assert!(!connection_dial_capacity_available(target + 1, 0, lost));
        }
        assert_eq!(connection_population_deficit(0, 0, u16::MAX), 30);
        assert_eq!(connection_population_deficit(u64::MAX, u64::MAX, 900), 0);
    }

    #[test]
    fn delayed_retry_backoff_is_registered_and_released_around_enqueue() {
        let runtime = crate::RUNTIME_SOURCE;
        let retry = crate::source::between(
            runtime,
            "async fn queue_peer_dial_retry(",
            "fn failed_peer_retry_delay_ms(",
        );
        let register = retry
            .find(".insert(peer, (expected_generation, retry_id))")
            .expect("retry not-before registration");
        let sleep = retry
            .find("failed_peer_retry_delay_ms(&address)")
            .expect("configured retry backoff");
        let ownership = retry
            .find("delayed.get(&peer) != Some(&(expected_generation, retry_id))")
            .expect("exact delayed retry ownership");
        let enqueue = retry
            .find(".send(PeerDialInstruction {")
            .expect("retry enqueue after backoff");
        let release = retry
            .find("drop(delayed)")
            .expect("retry exclusion released before enqueue wakes the scheduler");
        assert!(register < sleep && sleep < ownership && ownership < release && release < enqueue);

        let feeder = crate::source::between(
            runtime,
            "let peer_dial_scheduler =",
            "let swarm_event_loop = async",
        );
        assert!(feeder.contains("wings.delayed_peer_retries"));
        assert!(feeder.contains("retry.0 == queue_generation"));
        let marker = crate::source::between(
            runtime,
            "async fn try_mark_connection_attempt(",
            "async fn mark_handshake_ready_connection(",
        );
        assert!(marker.find("delayed_peer_retries.contains_key(peer)").unwrap()
            < marker.find("connection_attempts.insert(").unwrap());
    }

    #[test]
    fn stalled_pre_handshake_reservations_expire_and_capacity_wakes_retained_candidates() {
        let runtime = crate::RUNTIME_SOURCE;
        assert!(runtime.contains("const PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS: u64 = 60_000;"));
        assert!(!runtime.contains("PEER_POPULATION_RESCAN_MS"));

        let joiner = crate::source::between(
            runtime,
            "let handshake_instruction_handle = async",
            "join!(",
        );
        let ready_timeout = joiner
            .find("Duration::from_millis(PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS)")
            .expect("pre-handshake reservation timeout");
        let ready_wait = joiner
            .find("ready_connection.recv().await")
            .expect("identify-ready wait");
        let ownership = joiner[ready_timeout..]
            .find("connection_attempt_is_current(")
            .map(|offset| ready_timeout + offset)
            .expect("exact attempt ownership check");
        let removal = joiner[ready_wait..]
            .find("remove_connection_attempt(&wings, &id, connection_attempt_id)")
            .map(|offset| ready_wait + offset)
            .expect("timed-out reservation release");
        let retry = joiner[removal..]
            .find("queue_peer_dial_retry(")
            .map(|offset| removal + offset)
            .expect("timed-out retry registration");
        let release = joiner[retry..]
            .find("release_connection_reservation(")
            .map(|offset| retry + offset)
            .expect("single reservation release");
        assert!(ready_timeout < ownership && ownership < ready_wait);
        assert!(ready_wait < removal && removal < retry && retry < release);

        let feeder = crate::source::between(
            runtime,
            "let peer_dial_scheduler =",
            "let swarm_event_loop = async",
        );
        crate::source::assert_contains(feeder, &[
            "peers_instructions_chan_incoming.recv()",
            "cooldowns.contains(&candidate.peer)",
            "queue.push_front(candidate)",
        ]);
        assert!(feeder.find("changed.listen()").unwrap()
            < feeder.find("try_reserve_connection_capacity(").unwrap());
        assert!(!feeder.contains("last_population_rescan_ms"));

        let timeout_cleanup = &joiner[ready_wait..];
        let disconnect = timeout_cleanup
            .find("swarm.disconnect_peer_id(id).is_err()")
            .expect("pending transport abort");
        let removal = timeout_cleanup
            .find("remove_connection_attempt(&wings, &id, connection_attempt_id)")
            .expect("exact reservation removal");
        let release = timeout_cleanup
            .find("release_connection_reservation(")
            .expect("reservation counter release");
        assert!(disconnect < removal && removal < release);

        let outgoing_error = crate::source::between(
            runtime,
            "let retryable = !matches!(",
            "SwarmEvent::ConnectionClosed {",
        );
        let late_removal = outgoing_error
            .find("if !remove_connection_attempt_for_connection(")
            .expect("late event ownership guard");
        let late_return = outgoing_error[late_removal..]
            .find("return;")
            .map(|offset| late_removal + offset)
            .expect("stale event return");
        let late_release = outgoing_error
            .find("release_connection_reservation(")
            .expect("owned dial failure release");
        assert!(late_removal < late_return && late_return < late_release);
    }

    #[test]
    fn duplicate_overlay_peers_are_suppressed_until_the_owner_disconnects() {
        let runtime = crate::RUNTIME_SOURCE;
        let promotion = crate::source::between(
            crate::NETWORK_SOURCE,
            "pub(super) async fn promote_priced_peer(",
            "pub(crate) struct StreamBehaviour {",
        );
        let duplicate = promotion
            .split("} else if let Some(owner) = duplicate_owner {")
            .nth(1)
            .expect("duplicate overlay rejection");
        let reject = duplicate
            .find(".insert(peer, owner)")
            .expect("owner-bound duplicate suppression");
        let disconnect = duplicate
            .find(".disconnect_peer_id(peer)")
            .expect("duplicate disconnect");
        assert!(reject < disconnect);
        assert!(duplicate[..disconnect].contains("known_peers.lock().await.remove(&peer)"));
        assert!(
            duplicate[..disconnect].contains("delayed_peer_retries.lock().await.remove(&peer)")
        );

        let feeder = crate::source::between(
            runtime,
            "let peer_dial_scheduler =",
            "let swarm_event_loop = async",
        );
        assert!(feeder.contains("rejected.contains_key(&candidate.peer)"));
        assert!(runtime.contains("wings.rejected_duplicate_peers.lock().await.clear()"));
        assert!(runtime.contains(".retain(|_, owner| owner != &peer_id)"));
    }

    #[test]
    fn mainnet_startup_samples_the_complete_dns_bootnode_database() {
        use std::collections::HashSet;

        let profile = include_str!("../src/network_profile.rs");
        let mainnet = profile
            .split_once("pub(crate) const MAINNET_BOOTNODES: &[&str] = &[")
            .and_then(|(_, source)| source.split_once("pub(crate) const TESTNET_PROFILE"))
            .map(|(source, _)| source)
            .expect("mainnet bootnodes");
        let addresses = mainnet
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix('"')
                    .and_then(|line| line.strip_suffix("\","))
            })
            .collect::<Vec<_>>();
        assert_eq!(addresses.len(), 319);
        assert_eq!(addresses.iter().copied().collect::<HashSet<_>>().len(), 319);
        assert!(addresses.iter().all(|address| {
            address.starts_with("/dns4/")
                && address.contains(".libp2p.direct/tcp/")
                && address.contains("/tls/ws/p2p/")
        }));
        assert!(profile.contains("pub(crate) const INITIAL_BOOTNODE_BURST: usize = 160;"));
        assert!(
            profile.find("bootnodes.shuffle(&mut rand::thread_rng())")
                < profile.find("bootnodes.truncate(INITIAL_BOOTNODE_BURST)")
        );
        assert!(!profile.contains("bootnodes.retain("));

        let runtime = crate::RUNTIME_SOURCE;
        let address_filter = include_str!("../src/addresses.rs");
        crate::source::assert_contains(runtime, &[
            "is_publicly_dialable_underlay(&source_addr)",
            "browser_dial_address(source_addr).ok()?",
            "browser_dial_address(addr33).unwrap_or_else",
        ]);
        assert!(address_filter.contains("pub(crate) fn browser_dial_address("));
        assert!(!address_filter.contains("enum UnderlayFormat"));
        assert!(!address_filter.contains("fn beewss_to_dns_transformed"));
        assert!(runtime.contains("!self.allow_private_gossip.load(Ordering::Acquire)"));
        assert!(runtime.contains("self.service_worker_network_id() != 0"));
        crate::source::assert_contains(address_filter, &[
            "|embedded| embedded == address",
            ".map(is_public_ipv4)",
            ".unwrap_or_else(|| is_public_dns_name(&hostname))",
            "&& !ends_with(\".local\")",
            "eq_ignore_ascii_case",
        ]);
        assert!(include_str!("../src/handlers.rs").contains("underlay: peer.underlay,"));
    }

    #[test]
    fn cold_bootnode_burst_drains_into_one_dispatch_task() {
        let runtime = crate::RUNTIME_SOURCE;
        let profile = include_str!("../src/network_profile.rs");
        let accounting = include_str!("../src/accounting.rs");
        let handler = crate::source::between(
            runtime,
            "let bootnode_change_handle = async",
            "let accounting_event_handle = async",
        );

        crate::source::assert_contains(profile, &[
            "bootnodes.shuffle(&mut rand::rng())",
            "pub(crate) const INITIAL_BOOTNODE_BURST: usize = 160;",
            "bootnodes.truncate(INITIAL_BOOTNODE_BURST)",
        ]);
        assert!(accounting.contains("pub(crate) const CONNECTION_BUILDUP_LIMIT: u64 = 200;"));
        assert!(handler.contains("let mut bootnode_changes = vec![first_change];"));
        assert!(handler.contains("while let Ok(change) = self.bootnode_port.1.try_recv()"));

        let spawn = handler
            .find("spawn_local(async move")
            .expect("single batch dispatch task");
        let batch_loop = handler
            .find("for (baddr, usable, request_generation) in bootnode_changes")
            .expect("drained bootnode batch loop");
        let dial = handler
            .find("start_owned_connection_attempt(")
            .expect("owned swarm dial");
        assert!(spawn < batch_loop && batch_loop < dial);
        assert_eq!(
            handler.matches("spawn_local(async move").count(),
            1,
            "the handler must spawn one task per drained burst, not one per bootnode"
        );
        assert!(handler[batch_loop..].contains("reserve_connection_capacity("));
        assert!(handler[batch_loop..].contains("try_mark_connection_attempt("));
        assert!(handler[batch_loop..].contains("queue_peer_dial_retry("));
    }

    #[test]
    fn handshake_signer_is_derived_once_per_node() {
        let runtime = crate::RUNTIME_SOURCE;
        let handlers = include_str!("../src/handlers.rs");
        assert!(runtime.contains("handshake_signer: Arc<PrivateKeySigner>"));
        assert_eq!(runtime.matches("PrivateKeySigner::from_slice(").count(), 1);
        let handshake = crate::source::between(
            handlers,
            "async fn handshake_exchange(",
            "pub async fn pricing_handler(",
        );
        assert!(!handshake.contains("PrivateKeySigner::from_slice("));
    }

    #[test]
    fn bee_handshake_starts_after_queueing_one_canonical_observed_address() {
        let runtime = crate::RUNTIME_SOURCE;
        let received = runtime
            .rsplit("identify::Event::Received {")
            .next()
            .and_then(|source| source.split("identify::Event::Error {").next())
            .expect("identify receive lifecycle");
        crate::source::assert_contains(received, &[
            "canonical_identify_address",
            "canonical.is_none()",
            "let observed_addr = info.observed_addr;",
            "Some(observed_addr.clone())",
            "try_from_multiaddr(&info.observed_addr)",
            ".is_some_and(|peer| peer != identify_local_peer_id)",
        ]);
        assert_eq!(received.matches("physical_connections").count(), 1);
        assert_eq!(received.matches("handshake_ready_connections").count(), 1);
        assert!(received.contains("swarm.add_external_address(canonical)"));
        crate::source::assert_first_in_order(received, &[
            (".push(std::iter::once(peer_id))", "Identify push"),
            ("mark_handshake_ready_connection(", "Bee handshake readiness"),
        ]);
        crate::source::assert_excludes(runtime, &[
            "IDENTIFY_PUSH_CONCURRENCY",
            "identify_push_capacity",
            "IDENTIFY_PUSH_TIMEOUT_MS",
            "pending_identify_push",
            "identify::Event::Pushed {",
        ]);
        assert!(runtime.contains("identify::Event::Received { .. } | identify::Event::Error { .. }"));
        assert_eq!(
            runtime
                .matches("swarm.add_external_address(canonical)")
                .count(),
            1
        );
        assert!(runtime.contains("canonical_identify_address"));
        assert!(runtime.contains("swarm.remove_external_address(&address)"));

        let empty_observed = received
            .split("if info.observed_addr.is_empty()")
            .nth(1)
            .expect("empty Identify observation handling");
        assert!(empty_observed.contains("close_failed_identify_connection("));
        let identify_error = runtime
            .rsplit("identify::Event::Error {")
            .next()
            .and_then(|source| source.split("SwarmEvent::OutgoingConnectionError").next())
            .expect("Identify error lifecycle");
        assert!(identify_error.contains("close_failed_identify_connection("));

        let exact_close = crate::source::between(
            runtime,
            "async fn close_failed_identify_connection(",
            "async fn remove_connection_attempt(",
        );
        crate::source::assert_contains(exact_close, &[
            "attempt.physical_connection_id == Some(connection_id)",
            "!attempt.identify_failed",
            "handshake_ready_connections",
            "connections.contains(&connection_id)",
            "attempt.identify_failed = true;",
            "swarm.lock().await.close_connection(connection_id)",
        ]);
    }

    #[test]
    fn only_private_custom_bootnodes_enable_private_gossip() {
        let runtime = crate::RUNTIME_SOURCE;
        let connect = crate::source::between(
            crate::NETWORK_SOURCE,
            "pub(crate) async fn connect_bootnodes_for_current_network(",
            "pub(super) fn current_connection_generation(",
        );
        crate::source::assert_contains(connect, &[
            "is_private_or_local_bootnode(address)",
            "!profile.bootnodes.contains(&address.as_str())",
            ".store(private_custom_bootnodes, Ordering::Release)",
        ]);
        assert!(!connect.contains(".store(custom_bootnodes, Ordering::Release)"));

        let private_check = crate::source::between(
            runtime,
            "fn is_private_or_local_bootnode(",
            "pub(crate) struct BzzRangeRequest",
        );
        for classification in [
            "address.is_private()",
            "address.is_loopback()",
            "address.is_link_local()",
            "address.is_unspecified()",
        ] {
            assert!(private_check.contains(classification));
        }
    }

    #[test]
    fn early_pricing_is_reconciled_after_reservation_and_close_cannot_split_promotion() {
        let runtime = crate::RUNTIME_SOURCE;
        let accounting = crate::source::between(
            runtime,
            "let accounting_event_handle = async",
            "let pricing_event_handle = async",
        );
        let attempt = accounting
            .find("let connection_attempt_id = peer_file.connection_attempt_id;")
            .expect("handshake attempt identity");
        let connected_guard = accounting
            .find("let mut connected_peers = wings.connected_peers.lock().await;")
            .expect("peer lifecycle guard");
        let physical = accounting
            .find("let physical_session_current =")
            .expect("physical connection validation");
        let ownership = accounting
            .find("attempt.physical_connection_id")
            .expect("attempt reservation ownership");
        let accounting_arc = accounting
            .find("let accounting_peer_lock = {")
            .expect("saved accounting peer");
        let connected = accounting
            .find("connected_peers.insert(peer, peer_file)")
            .expect("connected-peer publication");
        let threshold = accounting
            .find("let threshold_ready = {")
            .expect("post-reservation pricing reconciliation");
        let promotion = accounting
            .find("self.promote_priced_peer(&wings, peer).await;")
            .expect("priced peer promotion");
        assert!(
            attempt < connected_guard
                && connected_guard < physical
                && physical < ownership
                && ownership < accounting_arc
                && accounting_arc < threshold
                && threshold < connected
                && threshold < promotion,
            "a handshake must own physical and counted capacity before publication and promotion"
        );
        crate::source::assert_contains(accounting, &[
            "let accounting_peer_for_timeout = accounting_peer_lock.clone();",
            "Arc::ptr_eq(",
            "peer_file.connection_attempt_id == connection_attempt_id",
        ]);
        assert!(!accounting.contains("timeout_attempt_id"));

        let promotion = crate::source::between(
            crate::NETWORK_SOURCE,
            "pub(super) async fn promote_priced_peer(",
            "pub(crate) struct StreamBehaviour {",
        );
        let connected_guard = promotion
            .find("let connected_peers_guard = wings.connected_peers.lock().await;")
            .expect("disconnect serialization guard");
        let physical = promotion
            .find("exclusive_physical_connection(&wings.physical_connections, &peer)")
            .expect("physical connection validation");
        let reservation_transfer = promotion
            .find("remove_connection_attempt(wings, &peer, peer_file.connection_attempt_id)")
            .expect("owned reservation transfer");
        let overlay_publish = promotion
            .find("Arc::make_mut(&mut overlay_peers_map).insert(peer_file.overlay, peer)")
            .expect("overlay publication");
        let population_transfer = promotion
            .find("complete_connection_reservation(")
            .expect("atomic reservation transfer");
        let connected_drop = promotion
            .find("drop(connected_peers_guard);")
            .expect("disconnect serialization release");
        let duplicate_cleanup = promotion
            .find("connected_peers.lock().await.remove(&peer)")
            .expect("duplicate cleanup");
        assert!(
            connected_guard < physical
                && physical < reservation_transfer
                && reservation_transfer < overlay_publish
                && overlay_publish < population_transfer
                && population_transfer < connected_drop
                && connected_drop < duplicate_cleanup,
            "disconnect must not interleave the reservation, overlay, and counter transfer"
        );
    }

    #[test]
    fn inbound_pricing_is_bound_to_the_exact_transport_session() {
        let runtime = crate::RUNTIME_SOURCE;
        let inbound = crate::source::between(
            runtime,
            "let pricing_inbound_handle = async move",
            "let gossip_peers_instructions",
        );
        crate::source::assert_contains(inbound, &[
            "exclusive_physical_connection(",
            "TransportConnectionSession::capture(",
            "pricing_handler(peer, stream, pricing_session, &pricing_chan_outgoing).await",
        ]);

        let handler_source = include_str!("../src/handlers.rs");
        let handler = crate::source::between(
            handler_source,
            "pub async fn pricing_handler(",
            "pub async fn gossip_handler(",
        );
        crate::source::assert_contains(handler, &[
            "session: TransportConnectionSession",
            "if !session.is_current()",
            "pricing_updates.try_send((peer, payment_threshold, session))",
        ]);

        let application = crate::source::between(
            runtime,
            "let pricing_event_handle = async",
            "let cheques_active_cache",
        );
        crate::source::assert_contains(application, &[
            "let (peer, amount, pricing_session) = pricing;",
            "pricing_session.is_current()",
            "expected_connection == Some(pricing_session.connection_id())",
        ]);
    }

    #[test]
    fn only_the_connection_that_owns_the_session_tears_down_peer_state() {
        let runtime = crate::RUNTIME_SOURCE;
        let close = runtime
            .split("SwarmEvent::ConnectionClosed {")
            .find(|source| source.contains("let close_owns_lifecycle ="))
            .and_then(|source| source.split("let accounting_peer =").next())
            .expect("connection-close lifecycle");
        let expected = close
            .find("let expected_peer_connection =")
            .expect("current session connection");
        let ownership = close
            .find("let close_owns_lifecycle =")
            .expect("connection ownership decision");
        let cleanup = close
            .find("let removed_peer_file = connected_peers.remove(&peer_id)")
            .expect("peer state cleanup");
        assert!(expected < ownership && ownership < cleanup);
        assert!(close.contains("expected == connection_id"));
        assert!(close.contains("expected_attempt_connection == Some(connection_id)"));
    }

    #[test]
    fn reconnect_waits_out_bee_accounting_blocklist() {
        const RATE: u64 = 450_000;
        const THRESHOLD: u64 = RATE * 3;

        assert_eq!(bee_reconnect_delay_seconds(0, 0, THRESHOLD, RATE), 4);
        assert_eq!(
            bee_reconnect_delay_seconds(RATE * 2, RATE, THRESHOLD, RATE),
            4
        );
        assert_eq!(
            bee_reconnect_delay_seconds(RATE * 20, RATE * 10, THRESHOLD, RATE),
            6
        );
        assert_eq!(bee_reconnect_delay_seconds(1, 2, 3, 0), 1);
        assert!(bee_reconnect_delay_seconds(u64::MAX, u64::MAX, u64::MAX, 1) > 0);

        let runtime = crate::RUNTIME_SOURCE;
        let close = runtime
            .split("SwarmEvent::ConnectionClosed {")
            .find(|source| source.contains("let close_owns_lifecycle ="))
            .and_then(|source| source.split("_ => {}").next())
            .expect("connection-close lifecycle");
        let snapshot = close
            .find("accounting_peer.balance")
            .expect("accounting balance snapshot");
        let backoff = close
            .find("bee_reconnect_delay_seconds(")
            .expect("Bee-compatible reconnect backoff");
        let cooldown = close
            .find(".connection_cooldowns")
            .expect("gossip-resistant reconnect cooldown");
        let guard_release = cooldown
            + close[cooldown..]
                .find("drop(connected_peers);")
                .expect("peer lifecycle guard release");
        let sleep = close
            .find("Duration::from_millis(reconnect_delay_ms)")
            .expect("backoff sleep");
        let generation_check = close[sleep..]
            .find("connection_generation.load(Ordering::Acquire) == retry_generation")
            .expect("post-backoff generation check");
        let cooldown_release = close[sleep..]
            .find(".remove(&peer_id)")
            .expect("same-generation cooldown release");
        let enqueue = close[sleep..]
            .find("PeerDialInstruction {")
            .expect("reconnect enqueue");
        assert!(close[enqueue..].contains("retry: true,"));
        assert!(
            snapshot < backoff
                && backoff < cooldown
                && cooldown < guard_release
                && guard_release < sleep
                && generation_check < cooldown_release
                && cooldown_release < enqueue
        );

        let reservation = crate::source::between(
            runtime,
            "async fn try_mark_connection_attempt(",
            "async fn remove_connection_attempt(",
        );
        assert!(reservation.contains("connection_cooldowns.contains(peer)"));
    }

    #[test]
    fn connection_generation_is_atomic_saturating_and_snapshotted_around_network_id() {
        let runtime = crate::RUNTIME_SOURCE;
        let context = crate::source::between(
            crate::NETWORK_SOURCE,
            "pub(super) async fn current_connection_context(&self)",
            "pub(super) fn bump_connection_generation(&self)",
        );
        crate::source::assert_first_in_order(context, &[
            ("let before = self.current_connection_generation();", "missing source marker"),
            ("let network_id = *self.network_id.lock().await;", "missing source marker"),
            ("let after = self.current_connection_generation();", "missing source marker"),
        ]);
        crate::source::assert_contains(runtime, &[
            ".fetch_update(",
            "Ordering::AcqRel",
            "Ordering::Acquire",
            "Some(generation.saturating_add(1))",
        ]);
        assert!(!runtime.contains("connection_generation.lock().await"));
    }

    #[test]
    fn refresh_settlement_coalesces_per_account_and_rearms_until_debt_is_clear() {
        let runtime = crate::RUNTIME_SOURCE;
        let instruction = crate::source::between(
            runtime,
            "let refreshment_instruction_handle = async",
            "let swap_price =",
        );
        let balance = instruction
            .find("let (balance, last_refreshment, payment_threshold) =")
            .expect("accounting snapshot");
        let session = instruction
            .find("current_accounting_protocol_session(")
            .expect("exact accounting session check");
        let dispatch = instruction
            .find("refresh_handler(")
            .expect("settlement dispatch");
        let completion_time = instruction
            .find("accounting_peer.lock().await.refreshment = Date::now();")
            .expect("completion-based attempt rate limit");
        let mutation = instruction
            .find("apply_refreshment(&accounting_peer, amount)")
            .expect("terminal accounting mutation");
        assert!(balance < session && session < dispatch);
        assert!(dispatch < completion_time && completion_time < mutation);
        crate::source::assert_contains(instruction, &[
            "if !refreshment_due(",
            "account.threshold,",
            "account.refresh_scheduled = false;",
            "RefreshmentOutcome::NotDispatched => {",
            "RefreshmentOutcome::Acknowledged(0)",
            "RefreshmentOutcome::AmbiguousAfterPayment",
            "account.balance = 0;",
            "account.threshold",
        ]);
        assert!(instruction[dispatch..].contains("attempted_amount,"));
        assert!(instruction.contains("quiesce_drain_and_close_accounting_session("));
        assert!(!instruction.contains("REFRESH_RATE * 100"));
        assert!(!instruction.contains("Duration::from_secs(15)"));
        assert!(!runtime.contains("ongoing_refreshments"));
        assert!(!runtime.contains("refreshment_apply_handle"));

        let ambiguous_close = crate::source::between(
            runtime,
            "async fn quiesce_drain_and_close_accounting_session(",
            "#[derive(NetworkBehaviour)]",
        );
        crate::source::assert_first_in_order(ambiguous_close, &[
            ("account.connection_id = None;", "new reservation quiescence"),
            ("accounting_peer.lock().await.reserve == 0", "dispatched accounting drain"),
            ("swarm.close_connection(connection_id)", "exact physical close"),
        ]);
        assert!(!ambiguous_close.contains("timeout("));

        let accounting = include_str!("../src/accounting.rs");
        let coalescing = accounting
            .find("if refreshment_due(")
            .expect("atomic refresh coalescing");
        let claim = accounting[coalescing..]
            .find("account.refresh_scheduled = true;")
            .map(|offset| coalescing + offset)
            .expect("refresh instruction claim");
        let enqueue = accounting[claim..]
            .find("refreshments.try_send(instruction)")
            .map(|offset| claim + offset)
            .expect("claimed refresh enqueue");
        assert!(coalescing < claim && claim < enqueue);
    }

    #[test]
    fn refresh_cadence_is_unchanged_when_the_peer_limit_leaves_headroom() {
        let threshold = REFRESH_RATE * 3;
        assert!(!refreshment_due(0, 0.0, threshold));
        assert!(!refreshment_due(REFRESH_RATE, 0.0, threshold));
        assert!(!refreshment_due(REFRESH_RATE * 2 - 1, 0.0, threshold));
        assert!(refreshment_due(REFRESH_RATE * 2, 0.0, threshold));
        assert!(!refreshment_due(REFRESH_RATE - 1, 1.0, threshold));
        assert!(refreshment_due(REFRESH_RATE, 1.0, threshold));
    }

    #[test]
    fn refresh_starts_before_debt_blocks_the_next_chunk() {
        // These balances cannot reach the old refresh target: another 260k
        // reservation already exceeds the announced limit.
        for (balance, threshold) in [(260_000, 450_000), (260_000, 450_123),
            (780_000, 900_000), (780_000, 1_000_000)] {
            assert!(balance + 260_000 > threshold);
            assert!(refreshment_due(balance, 0.0, threshold));
        }
        for threshold in [10_000, 260_000, 320_000, 450_000, 450_123,
            900_000, 1_000_000, 1_350_000] {
            for proximity in 0..=31 {
                let amount = price(proximity);
                if amount > threshold { continue; }
                let first_blocking_balance = threshold - amount + 1;
                for last in [0.0, 1.0] {
                    assert!(refreshment_due(first_blocking_balance, last, threshold),
                        "unscheduled debt at threshold {threshold}, price {amount}");
                }
            }
        }
    }

    #[test]
    fn zero_debt_never_refreshes_even_with_a_tiny_peer_limit() {
        for threshold in [0, 1, 10_000, 260_000, 320_000, 450_000, 1_350_000] {
            for last in [0.0, 1.0] {
                assert!(!refreshment_due(0, last, threshold));
            }
        }
    }

    #[test]
    fn chunk_prices_preserve_every_existing_proximity_value() {
        assert_eq!(price(0), 320_000);
        assert_eq!(price(31), 10_000);
        assert_eq!(price(u8::MAX), 10_000);
        for proximity in u8::MIN..=u8::MAX {
            // Historical price schedule, including its saturated tail.
            let previous = (u64::from(31_u8.saturating_sub(proximity)) + 1) * 10_000;
            assert_eq!(price(proximity), previous);
        }
    }

    #[test]
    fn original_refresh_interface_logs_use_the_bounded_log() {
        let runtime = crate::RUNTIME_SOURCE;
        let instruction = crate::source::between(
            runtime,
            "let refreshment_instruction_handle = async",
            "let swap_price =",
        );

        assert!(runtime.contains("mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)"));
        assert!(instruction.contains("interface_log_to("));
        for marker in [
            "Applied refreshment {}",
            "Refreshment attempt cleared 0",
            "Surplus balance increased for peer {} by {} to {}",
        ] {
            assert!(
                instruction.contains(marker),
                "missing original bounded refresh log {marker}"
            );
        }
        for replacement in [
            "Refresh dispatch peer={}",
            "Refresh not dispatched; retrying peer={}",
            "Refresh acknowledged peer={}",
            "Refresh ambiguous after payment peer={}",
        ] {
            assert!(
                !instruction.contains(replacement),
                "unexpected replacement refresh log {replacement}"
            );
        }
    }

    #[test]
    fn wasm_retrieval_yields_browser_turns_without_per_chunk_telemetry() {
        let runtime = crate::RUNTIME_SOURCE;
        let retrieve = runtime
            .split("let retrieve_chunk_handle = async")
            .nth(1)
            .and_then(|source| {
                source
                    .split("let handshake_instruction_handle = async")
                    .next()
            })
            .expect("retrieve dispatcher");
        crate::source::assert_contains(retrieve, &[
            "let retrieve_dispatch_yield_every = 128usize;",
            "let mut retrieve_dispatches_since_browser_yield = 0usize;",
            "async_std::task::sleep(Duration::ZERO).await;",
        ]);
        crate::source::assert_excludes(retrieve, &[
            "RETRIEVE_QUEUE_HOT_LOOP_GUARD_MS",
            "wave_done",
            "Completed {} of {} chunk retrieval requests",
        ]);

        let refresh = crate::source::between(
            runtime,
            "let refreshment_instruction_handle = async",
            "let swap_price =",
        );
        assert!(refresh.contains("refresh_dispatches % 8 == 0"));
        assert!(refresh.contains("async_std::task::sleep(Duration::ZERO).await;"));

    }

    #[test]
    fn direct_work_requests_do_not_cross_forwarding_queues() {
        let runtime = crate::RUNTIME_SOURCE;

        assert!(runtime.contains("chunk_push_port: AsyncPort<ChunkUploadRequest>"));
        assert!(runtime.contains("self.chunk_push_port.1.recv().await"));
        assert!(!runtime.contains("DirectChunkPushRequest"));
        assert!(!runtime.contains("push_chunk_port_handle"));

        let resolve = crate::source::between(
            runtime,
            "pub async fn resolve_bzz(&self, resource: String)",
            "pub async fn acquire_resolved_range(",
        );
        assert!(resolve.contains("bzz_stream::resolve_bzz(&resource, &self.chunk_port.0).await"));
        assert!(!runtime.contains("BzzResolveRequest"));
        assert!(!runtime.contains("resolve_bzz_handle"));
    }

    #[test]
    fn handshake_and_cheque_lifecycles_are_session_bound() {
        let runtime = crate::RUNTIME_SOURCE;
        assert!(runtime.contains("Duration::from_millis(HANDSHAKE_PROTOCOL_TIMEOUT_MS)"));

        let handlers = include_str!("../src/handlers.rs");
        assert!(handlers.contains("let ack = syn_ack.ack?;"));
        assert!(handlers.contains("let peer_address = ack.address?;"));
        let handshake = crate::source::between(
            handlers,
            "async fn handshake_exchange(",
            "pub async fn pricing_handler(",
        );
        crate::source::assert_contains(handshake, &[
            "deserialize_underlays(&syn.observed_underlay)",
            "try_from_multiaddr(underlay).as_ref() != Some(&local_peer)",
            "let underlay = syn.observed_underlay;",
        ]);
        assert!(!handshake.contains("underlay.clone()"));
        assert!(handshake.contains("if ack.network_id != network_id"));

        let connection = crate::source::between(
            handlers,
            "pub async fn connection_handler(",
            "pub async fn refresh_handler(",
        );
        let open = connection
            .find("control.open_stream(")
            .expect("stream open");
        let capture = connection
            .find("TransportConnectionSession::capture(")
            .expect("physical session capture");
        assert!(open < capture);
        assert!(connection.contains("peer, connection_id, physical_connections"));
        assert!(!runtime.contains("self_ephemerals"));
        crate::source::assert_contains(handlers, &[
            "read_control_protocol_frame(&mut stream).await",
            "stream.read_exact(&mut frame).await",
            "enum RefreshmentOutcome",
            "if acknowledged_amount > amount",
            "RefreshmentOutcome::AmbiguousAfterPayment",
        ]);
        let refresh_handler = crate::source::between(
            handlers,
            "pub async fn refresh_handler(",
            "pub async fn issue_handler(",
        );
        // Expiry may close an ambiguous refresh only after its physical work drains.
        assert!(refresh_handler.contains("Duration::from_secs(10)"));
        crate::source::assert_in_order(refresh_handler, &[
            "RefreshmentOutcome::NotDispatched",
            "async_std::future::timeout(",
            "open_current_outbound_stream(",
            "refreshment_exchange(",
        ]);
        let pricing = crate::source::between(
            handlers,
            "pub async fn pricing_handler(",
            "pub async fn gossip_handler(",
        );
        assert_eq!(
            pricing
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2
        );
        let refresh = crate::source::between(
            handlers,
            "async fn refreshment_exchange(",
            "async fn cheque_exchange(",
        );
        assert_eq!(
            refresh
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2
        );
        crate::source::assert_in_order(refresh, &[
            "read_control_protocol_frame(&mut stream).await",
            "timeout_outcome.set(RefreshmentOutcome::AmbiguousAfterPayment)",
            "stream.write_all(&payment_frame).await",
        ]);
        assert!(!handlers.contains("syn_ack.ack.clone().unwrap()"));

        crate::source::assert_contains(runtime, &[
            "cheques.insert(",
            "claim_current_cheque(",
            "(cheque_amt, cheque_generation)",
            "map.get(&peer).copied() == Some((amount, cheque_generation))",
            "generation == cheque_generation",
        ]);
        assert!(!runtime.contains("swap_beneficiaries"));
        assert!(!handlers.contains("beneficiaries:"));
        let cheque = handlers.split("async fn cheque_exchange(").nth(1).unwrap();
        crate::source::assert_in_order(cheque, &[
            "get_chequebook_last_issued_cheque_payout(",
            "stored_cumulative_payout.is_zero()",
            "checked_mul(price)?",
            ".checked_add(cheque_delta)?",
            ".checked_add(effective_deduction)?",
            "prepare_emit_cheque_bytes(",
            "stream.write_all(&bufw).await.ok()?",
            "set_chequebook_last_issued_cheque_payout(",
        ]);

        let cheque_claim =
            crate::source::between(runtime, "async fn claim_current_cheque(", "\npub(crate) ");
        crate::source::assert_first_in_order(cheque_claim, &[
            ("wings.connected_peers.lock().await", "peer lifecycle guard"),
            (".accounting_peers", "exact account lookup"),
            ("wings.ongoing_cheques.lock().await", "cheque claim map"),
            ("exclusive_physical_connection(", "last physical-session check"),
            ("cheques.insert(", "claim publication"),
        ]);

        let cheque_dispatch = crate::source::between(
            runtime,
            "let cheque_instruction_handle = async",
            "let cheque_apply_handle = async",
        );
        let capture = cheque_dispatch
            .find("OutboundProtocolSession::capture(")
            .expect("cheque transport-session capture");
        let beneficiary = cheque_dispatch[capture..]
            .find("peer_file.connection_id == protocol_session.connection_id()")
            .map(|offset| capture + offset)
            .expect("cheque beneficiary must belong to the captured connection");
        assert!(cheque_dispatch[beneficiary..].contains("peer_file.beneficiary"));
        let post_capture_claim = cheque_dispatch[capture..]
            .find("map.get(&peer).copied() == Some((amount, cheque_generation))")
            .map(|offset| capture + offset)
            .expect("post-capture cheque claim validation");
        let dispatch = cheque_dispatch[post_capture_claim..]
            .find("issue_handler(")
            .map(|offset| post_capture_claim + offset)
            .expect("cheque protocol dispatch");
        assert!(
            capture < beneficiary
                && beneficiary < post_capture_claim
                && post_capture_claim < dispatch,
            "a stale cheque claim must not cross onto a replacement peer session"
        );
    }

    #[test]
    fn retrieval_reads_complete_length_delimited_frames_despite_transport_fragmentation() {
        let handlers = include_str!("../src/handlers.rs");
        assert!(handlers.contains("const EMPTY_HEADERS_FRAME: &[u8] = &[0];"));
        let retrieval = crate::source::between(
            handlers,
            "pub async fn retrieve_handler(",
            "pub async fn pushsync_handler(",
        );

        assert_eq!(
            retrieval
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2,
            "both Headers and Delivery must use exact length-delimited framing"
        );
        assert!(!retrieval.contains("stream.read("));
    }

    #[test]
    fn hive_reads_complete_length_delimited_frames_despite_transport_fragmentation() {
        let handlers = include_str!("../src/handlers.rs");
        let hive = crate::source::between(
            handlers,
            "pub async fn gossip_handler(",
            "async fn refreshment_exchange(",
        );

        assert_eq!(
            hive.matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            1,
            "Headers must use exact length-delimited framing"
        );
        assert_eq!(
            hive.matches(
                "read_control_protocol_frame_bounded(&mut stream, HIVE_PROTOCOL_MAX_FRAME_BYTES)"
            )
            .count(),
            1,
            "Peers must use the Bee-compatible Hive frame bound"
        );
        assert!(!hive.contains("stream.read("));
        assert!(hive.contains("etiquette_2::Peers::decode("));
        assert!(!hive.contains("Peers::decode_length_delimited"));
    }

    #[test]
    fn pushsync_uses_exact_framing_and_cannot_drop_a_short_write() {
        let handlers = include_str!("../src/handlers.rs");
        let pushsync = handlers
            .split("async fn pushsync_exchange(")
            .nth(1)
            .expect("pushsync protocol handler");

        assert_eq!(
            pushsync
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2,
            "both Headers and Receipt must use exact length-delimited framing"
        );
        assert!(pushsync.contains("stream.write_all(&delivery_frame).await"));
        assert!(!pushsync.contains("stream.write(&delivery_frame"));
        assert!(!pushsync.contains("stream.read("));
        assert!(pushsync.contains("etiquette_7::Receipt::decode("));
        assert!(!pushsync.contains("Receipt::decode_length_delimited"));
    }

    #[test]
    fn accounting_protocol_streams_cannot_implicitly_redial_or_cross_sessions() {
        let runtime = crate::RUNTIME_SOURCE;
        crate::source::assert_contains(runtime, &[
            "Poll::Ready(ToSwarm::Dial { opts })",
            "FromSwarm::DialFailure(DialFailure",
            "let error = DialError::NoAddresses;",
        ]);

        let handlers = include_str!("../src/handlers.rs");
        let open_current = crate::source::between(
            handlers,
            "async fn open_current_outbound_stream(",
            "pub async fn refresh_handler(",
        );
        assert_eq!(open_current.matches("session.is_current()").count(), 2);
        let open = open_current
            .find(".open_stream(")
            .expect("stream negotiation");
        let post_open = open_current[open..]
            .find("session.is_current()")
            .map(|offset| open + offset)
            .expect("post-open session validation");
        assert!(open < post_open);

        for call in [
            "open_current_outbound_stream(peer, control, PSEUDOSETTLE_PROTOCOL, &session)",
            "open_current_outbound_stream(peer, control, SWAP_PROTOCOL, &session)",
            "open_current_outbound_stream(peer, control, RETRIEVAL_PROTOCOL, &session)",
            "open_current_outbound_stream(peer, control, PUSHSYNC_PROTOCOL, &session)",
        ] {
            assert!(
                handlers.contains(call),
                "accounting protocol wrapper must use the session-bound stream helper: {call}"
            );
        }

        let retrieval = include_str!("../src/retrieval.rs");
        let selection = crate::source::between(
            retrieval,
            "async fn select_retrieve_peer(",
            "async fn retrieve_attempt(",
        );
        crate::source::assert_in_order(
            &crate::source::compact(selection),
            &[
                "letSome(connection_id)=reserve(accounting_peer,req_price).await",
                "OutboundProtocolSession::capture(",
                "letselected=ReservedRetrievePeer{",
                "return(Some(selected),false);",
            ],
        );
        assert!(retrieval.contains("retrieve_handler(peer, &request, control, session)"));

        let upload = include_str!("../src/upload.rs");
        assert!(upload.contains("OutboundProtocolSession::capture("));
        assert!(upload.contains("pushsync_handler("));
    }
}

mod retrieve_group_stream {
    use crate::source::between;

    const RETRIEVAL_SOURCE: &str = include_str!("../src/retrieval.rs");

    fn source_section(start: &str, end: &str) -> &'static str {
        between(RETRIEVAL_SOURCE, start, end)
    }

    #[test]
    fn requested_children_are_published_before_group_terminal_completion() {
        let group = source_section(
            "async fn fetch_data_group_indices_streaming(",
            "#[derive(Clone)]\nstruct TraversalNode",
        );
        assert_eq!(
            group.matches("child_emitter.emit(").count(),
            3,
            "cache hits, received data, and reconstruction all publish"
        );
        assert!(
            group.contains("if requested_count == data_count")
                && group.contains("dispatch_group_shards(")
                && group.contains("usize::MAX"),
            "the conservative legacy fallback must retain its full-group parity hedge"
        );

        let traversal = source_section(
            "async fn retrieve_data_range_from_root(",
            "async fn retrieve_data_joined(",
        );
        assert!(traversal.contains("fetch_data_group_indices_streaming("));
        assert!(
            !traversal.contains("spawn_local"),
            "group coordinators must remain owned so dropping the join closes admission guards"
        );
    }

    #[test]
    fn queued_children_keep_the_join_alive_and_failure_is_all_or_nothing() {
        let traversal = source_section(
            "async fn retrieve_data_range_from_root(",
            "async fn retrieve_data_joined(",
        );
        let completion = traversal
            .split("Either::Right((completion, _)) => {")
            .nth(1)
            .expect("group result");
        assert!(
            completion.starts_with("\n                    completion??;"),
            "a failed group must reject the complete join before processing more children"
        );
        assert!(
            traversal.contains("(written == requested_len).then_some(output)"),
            "partial output must never be returned"
        );
    }

    #[test]
    fn peer_hedges_wait_past_normal_swarm_response_latency() {
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_HEDGE_AFTER_MS: u64 = 1_000;"));
        assert!(
            RETRIEVAL_SOURCE
                .contains("const RETRIEVE_RS_HEDGE_AFTER_MS: u64 = RETRIEVE_HEDGE_AFTER_MS * 2;")
        );
    }

    #[test]
    fn range_traversal_has_one_generic_recovery_policy() {
        let group = source_section(
            "async fn fetch_data_group_indices_streaming(",
            "#[derive(Clone)]\nstruct TraversalNode",
        );
        assert!(!group.contains("DataRangeTraversalPolicy"));
        assert!(!group.contains("maximum_requested_children"));
        assert!(group.contains("let mut raw_fetches = RawFetchQueue::new("));

        let raw_flights = source_section("struct RawFetchKey", "fn decrypt_join_chunk");
        assert!(raw_flights.contains("admission.clone()"));
        assert!(!raw_flights.contains("wait_closed().await"));

        let traversal = source_section(
            "async fn retrieve_data_range_from_root(",
            "async fn retrieve_data_joined(",
        );
        assert!(traversal.contains("groups.len() < RETRIEVE_DATA_GROUP_CONCURRENCY"));
        assert!(!traversal.contains("shared_physical_admission"));
        assert!(
            !RETRIEVAL_SOURCE
                .to_ascii_lowercase()
                .contains("conservative")
        );
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;"));
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS: usize = 20;"));
    }
}

mod rolling_erasure_tail {
    use crate::retrieval_conventions::{
        RetrieveHedgeDemand, SharedRetrieveHedgeDemand, retrieve_attempt_start_allowed,
        rolling_full_group_eligible, rolling_full_group_static_candidate,
    };
    use crate::source::between;

    const RETRIEVAL_SOURCE: &str = include_str!("../src/retrieval.rs");
    const RUNTIME_SOURCE: &str = crate::RUNTIME_SOURCE;

    fn group_source() -> &'static str {
        between(
            RETRIEVAL_SOURCE,
            "async fn fetch_data_group_indices_streaming(",
            "#[derive(Clone)]\nstruct TraversalNode",
        )
    }

    #[test]
    fn rolling_requires_a_full_recoverable_mixed_cache_group() {
        // A production Medium full group is 119 data + 9 parity. Eight decoded-only
        // hits leave one parity beyond the cache-basis deficit; nine do not.
        assert!(rolling_full_group_eligible(119, 119, 9, 0, 119));
        assert!(rolling_full_group_eligible(119, 119, 9, 8, 1));

        assert!(!rolling_full_group_eligible(118, 119, 9, 0, 118));
        assert!(!rolling_full_group_eligible(119, 119, 0, 0, 119));
        assert!(!rolling_full_group_eligible(119, 119, 9, 9, 1));
        assert!(!rolling_full_group_eligible(119, 119, 9, 0, 0));
    }

    #[test]
    fn partial_groups_never_pay_for_a_raw_basis_scan() {
        assert!(rolling_full_group_static_candidate(119, 119, 9));
        assert!(!rolling_full_group_static_candidate(118, 119, 9));
        assert!(!rolling_full_group_static_candidate(119, 119, 0));

        let group = group_source();
        let candidate = group
            .find("if static_rolling_candidate {")
            .expect("static candidate branch");
        let dispatch = group[candidate..]
            .find("let mut cached_requested = cached_requested.into_iter();")
            .map(|offset| candidate + offset)
            .expect("initial dispatch");
        assert!(group[candidate..dispatch].contains("cache.get_decoded(reference, true)"));
        assert!(!group[dispatch..].contains("cache.get_decoded(reference, true)"));
        let dispatch_source = &group[dispatch..];
        let reference = dispatch_source
            .find("let reference = data_references.clone().nth(index)?;")
            .expect("requested child reference");
        let cache_hit = dispatch_source
            .find("cached_decoded_chunk(reference)")
            .expect("partial group decoded cache lookup");
        let queue = dispatch_source
            .find("raw_fetches.queue_data_shard(")
            .expect("raw registration");
        assert!(reference < cache_hit && cache_hit < queue);

        let cache =
            crate::source::between(RETRIEVAL_SOURCE, "impl DecodedChunkCache {", "fn get_raw(");
        assert!(cache.contains("let raw = include_raw.then(|| entry.raw.clone()).flatten()"));
        assert!(RETRIEVAL_SOURCE.contains("get_decoded(reference, false)"));
        assert!(RETRIEVAL_SOURCE.contains("get_decoded(reference, true)"));
    }

    #[test]
    fn managed_attempts_serialize_but_retry_and_ordinary_hedging_are_preserved() {
        assert!(retrieve_attempt_start_allowed(
            RetrieveHedgeDemand::DistinctShardManaged,
            0,
            false,
        ));
        assert!(!retrieve_attempt_start_allowed(
            RetrieveHedgeDemand::DistinctShardManaged,
            1,
            true,
        ));
        assert!(retrieve_attempt_start_allowed(
            RetrieveHedgeDemand::DistinctShardManaged,
            0,
            true,
        ));
        assert!(!retrieve_attempt_start_allowed(
            RetrieveHedgeDemand::Ordinary,
            1,
            false,
        ));
        assert!(retrieve_attempt_start_allowed(
            RetrieveHedgeDemand::Ordinary,
            1,
            true,
        ));
    }

    #[test]
    fn an_ordinary_follower_wakes_and_monotonically_promotes_a_managed_flight() {
        async_std::task::block_on(async {
            let demand = SharedRetrieveHedgeDemand::new(RetrieveHedgeDemand::DistinctShardManaged);
            assert_eq!(demand.current(), RetrieveHedgeDemand::DistinctShardManaged);
            let waiting = demand.clone();
            let waiter = async_std::task::spawn(async move {
                waiting.wait_until_ordinary().await;
            });
            async_std::task::yield_now().await;
            demand.promote(RetrieveHedgeDemand::Ordinary);
            async_std::future::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("promotion should wake a managed leader");
            assert_eq!(demand.current(), RetrieveHedgeDemand::Ordinary);
            demand.promote(RetrieveHedgeDemand::DistinctShardManaged);
            assert_eq!(demand.current(), RetrieveHedgeDemand::Ordinary);
            async_std::future::timeout(
                std::time::Duration::from_secs(1),
                demand.wait_until_ordinary(),
            )
            .await
            .expect("promotion before listening must not be missed");
        });
    }

    #[test]
    fn rolling_and_legacy_paths_keep_their_required_boundaries() {
        let group = group_source();
        crate::source::assert_contains(group, &[
            "let hedge_due = rolling && (Date::now() - started).max(0.0) as u64 >= hedge_after;",
            "let active = dispatched.checked_sub(completed)?;",
            "data_count.checked_sub(active)?",
            "if completed == dispatched && (!rolling || hedge_due)",
        ]);
        let rolling_start = group.find("if hedge_due {").expect("rolling branch");
        let legacy_start = group[rolling_start..]
            .find("} else if !rolling && recovery_dispatched {")
            .map(|offset| rolling_start + offset)
            .expect("legacy branch");
        let rolling_branch = &group[rolling_start..legacy_start];
        let legacy_branch = &group[legacy_start..];

        assert!(rolling_branch.contains("dispatch_group_shards("));
        assert!(rolling_branch.contains("RetrieveHedgeDemand::DistinctShardManaged"));
        assert!(!rolling_branch.contains("RETRIEVE_RS_HEDGE_AFTER_MS"));
        assert!(legacy_branch.contains("dispatch_group_shards("));
        assert!(legacy_branch.contains("RETRIEVE_RS_HEDGE_AFTER_MS"));
        assert!(group.contains("requested_count == data_count"));
        assert!(group.contains("let started = Date::now();"));

        let raw_queue = crate::source::between(
            RETRIEVAL_SOURCE,
            "fn queue_drained_raw_chunk(",
            "fn decrypt_join_chunk",
        );
        crate::source::assert_first_in_order(raw_queue, &[
            ("shared_demand.promote(hedge_demand)", "missing source marker"),
            ("if !registration.leader", "missing source marker"),
            ("self.chunks.try_send(", "missing source marker"),
        ]);
        assert!(raw_queue.contains("hedge_demand: registration.shared.hedge_demand.clone()"));

        let retrieve_chunk = crate::source::between(
            RETRIEVAL_SOURCE,
            "pub async fn retrieve_chunk(",
            "pub async fn retrieve_check_chunk(",
        );
        crate::source::assert_contains(retrieve_chunk, &[
            "map(SharedRetrieveHedgeDemand::current)",
            "unwrap_or(RetrieveHedgeDemand::Ordinary)",
            "wait_until_ordinary()",
        ]);
        assert!(!retrieve_chunk.contains("RETRIEVE_MANAGED_ADMISSION_POLL_MS"));
        assert!(retrieve_chunk.contains("mpsc::unbounded::<RetrieveAttemptResult>()"));
        let physical_attempt = crate::source::between(
            RETRIEVAL_SOURCE,
            "async fn retrieve_attempt(",
            "fn chunk_address_parts(",
        );
        assert!(physical_attempt.contains("retrieve_handler(peer, &request, control, session)"));
        assert!(!physical_attempt.contains("spawn_local"));
        crate::source::assert_in_order(physical_attempt, &[
            "Err(_)",
            "verify_chunk(&request.addr, &chunk)",
            "apply_credit(&accounting_peer, req_price, &refresh_chan).await",
            "return RetrieveAttemptResult::Found(chunk, soc)",
            "cancel_reserve(&accounting_peer, req_price).await",
        ]);
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;"));
        assert!(RUNTIME_SOURCE.contains("hedge_demand: None"));
    }
}

mod progress_events {
    use crate::events::ProgressStore;

    #[test]
    fn late_updates_do_not_reopen_finished_progress() {
        let mut store = ProgressStore::default();
        let id = store.start("upload", "file", "read", Some(0), "reading");
        store.finish(&id, "failed", "slice read failed", false);
        store.update(&id, "push", Some(50), "late chunk receipt");

        let (_, rows) = store.snapshot_if_changed(0).expect("progress changed");
        let row = rows.iter().find(|row| row.id == id).expect("row exists");
        assert!(row.done);
        assert!(!row.ok);
        assert_eq!(row.phase, "failed");
        assert_eq!(row.detail, "slice read failed");
    }
}

mod retrieve_generations {
    use crate::retrieval_conventions::{
        PendingGenerationRelation, RetrieveCancelRegistry, generation_is_newer,
        latest_registered_generation, next_nonzero_generation, pending_generation_relation,
    };

    #[test]
    fn advances_across_create_seek_evict_and_recreate() {
        let created = next_nonzero_generation(0);
        let sought = next_nonzero_generation(created);
        let recreated = next_nonzero_generation(sought);

        assert_eq!(created, 1);
        assert!(sought > created);
        assert!(recreated > sought);
    }

    #[test]
    fn wrapping_never_emits_the_reserved_zero_generation() {
        assert_eq!(next_nonzero_generation(u64::MAX), 1);
        assert!(generation_is_newer(1, u64::MAX));
        assert!(!generation_is_newer(u64::MAX, 1));
        assert_eq!(latest_registered_generation(u64::MAX, 1), 1);
        assert_eq!(latest_registered_generation(1, u64::MAX), 1);
    }

    #[test]
    fn pending_generation_order_is_wrap_safe() {
        assert_eq!(latest_registered_generation(0, 1), 1);
        assert_eq!(latest_registered_generation(7, 8), 8);
        assert_eq!(latest_registered_generation(8, 7), 8);
        assert_eq!(
            pending_generation_relation(u64::MAX, 1),
            PendingGenerationRelation::Replace
        );
        assert_eq!(
            pending_generation_relation(1, u64::MAX),
            PendingGenerationRelation::RejectStale
        );
        assert_eq!(
            pending_generation_relation(7, 7),
            PendingGenerationRelation::Join
        );
        assert_eq!(
            pending_generation_relation(0, 7),
            PendingGenerationRelation::Join
        );
    }

    #[test]
    fn replacement_wakes_the_old_token_and_stale_registration_stays_cancelled() {
        async_std::task::block_on(async {
            let registry = RetrieveCancelRegistry::default();
            let old = registry.register("stream".into(), u64::MAX).await.unwrap();
            let replacement = registry.register("stream".into(), 1);
            let (_, current) = futures::join!(old.cancelled(), replacement);
            let current = current.unwrap();

            assert!(!old.is_current());
            assert!(current.is_current());
            assert!(
                registry
                    .register("stream".into(), u64::MAX)
                    .await
                    .is_some_and(|stale| !stale.is_current())
            );
        });
    }
}

mod retrieve_admission {
    use crate::retrieval_conventions::{
        RetrieveAdmission, TransferPause, acquire_retrieve_permit, retrieve_admission_current,
        transfer_pause_enabled, wait_transfer_unpaused, wait_transfer_unpaused_for_admission,
    };
    use async_lock::Semaphore;
    use std::{
        future::Future,
        pin::pin,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    #[derive(Debug)]
    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn close_is_monotonic_and_visible_to_every_clone() {
        let admission = RetrieveAdmission::new();
        let queued = admission.clone();

        assert!(admission.is_open());
        assert!(queued.is_open());
        assert!(!queued.returned_cac());
        admission.record_returned_cac();
        admission.close();
        admission.close();

        assert!(!admission.is_open());
        assert!(!queued.is_open());
        assert!(queued.returned_cac());
    }

    #[test]
    fn ordinary_admission_keeps_its_unlimited_attempt_semantics() {
        let admission = RetrieveAdmission::new();

        for _ in 0..64 {
            assert!(admission.physical_attempt_available());
            assert!(admission.try_claim_physical_attempt());
        }

        admission.record_physical_attempt_timeout();
        admission.record_confirmed_empty_physical_attempt();
        assert!(admission.is_open());
        assert_eq!(admission.timed_out_physical_attempts(), None);
        assert_eq!(admission.confirmed_empty_physical_attempts(), None);
    }

    #[test]
    fn finite_attempt_budget_is_atomic_and_closes_after_exactly_two_claims() {
        const CONTENDERS: usize = 32;
        let admission = RetrieveAdmission::new_with_attempt_limit(2);
        let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
        let attempts = (0..CONTENDERS)
            .map(|_| {
                let admission = admission.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    admission.try_claim_physical_attempt()
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        let claimed = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("attempt contender"))
            .filter(|claimed| *claimed)
            .count();

        assert_eq!(claimed, 2);
        assert!(!admission.is_open());
        assert!(!admission.physical_attempt_available());
        assert!(!admission.try_claim_physical_attempt());
    }

    #[test]
    fn drop_guard_closes_early_return_paths() {
        let admission = RetrieveAdmission::new();
        {
            let _guard = admission.close_on_drop();
            assert!(admission.is_open());
        }

        assert!(!admission.is_open());
    }

    #[test]
    fn local_and_stream_cancellation_are_both_required_for_admission() {
        let admission = RetrieveAdmission::new();

        assert!(retrieve_admission_current(true, &None));
        assert!(!retrieve_admission_current(false, &None));
        assert!(retrieve_admission_current(true, &Some(admission.clone())));
        assert!(!retrieve_admission_current(false, &Some(admission.clone())));

        admission.close();
        assert!(!retrieve_admission_current(true, &Some(admission)));
    }

    #[test]
    fn close_wakes_pending_waiters() {
        let admission = RetrieveAdmission::new();
        let closer = admission.clone();
        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(admission.wait_closed());

        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Pending);
        closer.close();
        assert!(wake.0.load(Ordering::SeqCst) > 0);
        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn waiting_after_close_is_immediately_ready() {
        let admission = RetrieveAdmission::new();
        admission.close();
        admission.close();

        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake);
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(admission.wait_closed());

        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn resuming_wakes_all_paused_transfers_and_rechecks_a_new_pause() {
        let pause = Arc::new(TransferPause::default());
        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut context = Context::from_waker(&waker);
        assert_eq!(
            pin!(wait_transfer_unpaused(&pause)).poll(&mut context),
            Poll::Ready(())
        );

        assert!(pause.toggle());
        let mut first = pin!(wait_transfer_unpaused(&pause));
        let mut second = pin!(wait_transfer_unpaused(&pause));
        assert_eq!(first.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(second.as_mut().poll(&mut context), Poll::Pending);
        assert!(!pause.toggle());
        assert!(wake.0.load(Ordering::SeqCst) >= 2);
        assert!(pause.toggle());
        assert_eq!(first.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(second.as_mut().poll(&mut context), Poll::Pending);

        assert!(!pause.toggle());
        assert_eq!(first.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(second.as_mut().poll(&mut context), Poll::Ready(()));
        assert_eq!(
            pin!(wait_transfer_unpaused(&pause)).poll(&mut context),
            Poll::Ready(())
        );
    }

    #[test]
    fn admission_closure_wakes_a_paused_transfer_without_resuming_it() {
        let pause = Arc::new(TransferPause::default());
        pause.toggle();
        let admission = RetrieveAdmission::new();
        let admissions = Some(admission.clone());
        let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(wait_transfer_unpaused_for_admission(&pause, &admissions));

        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Pending);
        admission.close();
        assert!(wake.0.load(Ordering::SeqCst) > 0);
        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(false));
        assert!(transfer_pause_enabled(&pause));
    }

    #[test]
    fn closing_drops_a_waiter_before_retrieve_capacity_is_reserved() {
        async_std::task::block_on(async {
            let semaphore = Arc::new(Semaphore::new(0));
            let admission = RetrieveAdmission::new();
            let closer = admission.clone();
            let waiting = async_std::task::spawn({
                let semaphore = semaphore.clone();
                async move { acquire_retrieve_permit(&semaphore, Some(&admission)).await }
            });

            async_std::task::yield_now().await;
            closer.close();
            assert!(waiting.await.is_none());
        });
    }
}

mod retrieve_singleflight {
    use crate::retrieval_conventions::SingleflightRegistry;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn identical_keys_share_one_leader_and_one_resource() {
        let created = Rc::new(Cell::new(0));
        let mut flights = SingleflightRegistry::<String, usize, Rc<Cell<bool>>>::default();

        let first_created = Rc::clone(&created);
        let first = flights.register("chunk".to_string(), 3, move || {
            first_created.set(first_created.get() + 1);
            Rc::new(Cell::new(true))
        });
        let second_created = Rc::clone(&created);
        let second = flights.register("chunk".to_string(), 5, move || {
            second_created.set(second_created.get() + 1);
            Rc::new(Cell::new(true))
        });

        assert!(first.leader);
        assert!(!second.leader);
        assert_eq!(first.flight_id, second.flight_id);
        assert!(Rc::ptr_eq(&first.shared, &second.shared));
        assert_eq!(created.get(), 1);
    }

    #[test]
    fn last_waiter_closes_but_does_not_remove_the_producer_flight() {
        let mut flights = SingleflightRegistry::<u8, (), Rc<Cell<bool>>>::default();
        let first = flights.register(7, (), || Rc::new(Cell::new(true)));
        let second = flights.register(7, (), || Rc::new(Cell::new(true)));

        assert!(
            flights
                .remove_waiter(&7, first.flight_id, first.waiter_id)
                .is_none()
        );
        assert!(second.shared.get());
        let shared = flights
            .remove_waiter(&7, second.flight_id, second.waiter_id)
            .expect("last waiter returns the shared admission");
        shared.set(false);
        assert!(!first.shared.get());

        let follower = flights.register(7, (), || Rc::new(Cell::new(true)));
        assert!(!follower.leader);
        assert_eq!(follower.flight_id, first.flight_id);
        assert!(!follower.shared.get());

        let completed = flights
            .take(&7, first.flight_id)
            .expect("only the producer removes the flight");
        assert_eq!(completed.waiters.len(), 1);

        let successor = flights.register(7, (), || Rc::new(Cell::new(true)));
        assert!(successor.leader);
        assert_ne!(successor.flight_id, first.flight_id);
    }

    #[test]
    fn completion_detaches_every_waiter_atomically() {
        let mut flights = SingleflightRegistry::<u8, usize, ()>::default();
        let first = flights.register(9, 4, || ());
        let second = flights.register(9, 2, || ());

        let mut waiters = flights
            .take(&9, first.flight_id)
            .expect("active flight")
            .waiters;
        waiters.sort_unstable_by_key(|(_, waiter)| *waiter);
        assert_eq!(waiters, vec![(second.waiter_id, 2), (first.waiter_id, 4)]);
    }

    #[test]
    fn distinct_scopes_never_share() {
        let mut flights = SingleflightRegistry::<(&str, u64), (), ()>::default();
        assert!(flights.register(("video", 1), (), || ()).leader);
        assert!(flights.register(("video", 2), (), || ()).leader);
        assert!(flights.register(("audio", 1), (), || ()).leader);
    }

    #[test]
    fn stale_producer_cannot_take_a_successor_with_the_same_key() {
        let mut flights = SingleflightRegistry::<u8, &'static str, Rc<Cell<bool>>>::default();
        let old = flights.register(3, "old", || Rc::new(Cell::new(true)));
        let old_shared = flights
            .remove_waiter(&3, old.flight_id, old.waiter_id)
            .expect("old flight admission is returned");
        old_shared.set(false);
        let old_flight = flights
            .take(&3, old.flight_id)
            .expect("old producer completes its own flight");
        assert!(old_flight.waiters.is_empty());

        let successor = flights.register(3, "new", || Rc::new(Cell::new(true)));
        assert!(successor.leader);
        assert_ne!(old.flight_id, successor.flight_id);
        assert!(flights.take(&3, old.flight_id).is_none());
        assert!(
            flights
                .remove_waiter(&3, old.flight_id, old.waiter_id)
                .is_none()
        );

        let flight = flights
            .take(&3, successor.flight_id)
            .expect("successor remains registered");
        assert_eq!(flight.waiters, vec![(successor.waiter_id, "new")]);
    }
}
