#![allow(dead_code)]

#[path = "../src/accounting.rs"]
mod accounting;
#[path = "../src/events.rs"]
mod events;
#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;

mod connection {
    use crate::accounting::{
        CONNECTION_BUILDUP_LIMIT, REFRESH_RATE, bee_reconnect_delay_seconds,
        connection_dial_capacity_available, connection_population_deficit, refreshment_due,
    };
    #[test]
    fn first_usable_connections_do_not_wait_for_the_population_target() {
        assert_eq!(CONNECTION_BUILDUP_LIMIT, 200);
        assert!(connection_dial_capacity_available(0, 0));
        assert!(connection_dial_capacity_available(1, 0));
        assert!(connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT - 1,
            0
        ));
    }

    #[test]
    fn retrieval_dispatches_at_the_first_priced_peer() {
        assert!(connection_dial_capacity_available(1, 0));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT,
            0
        ));

        let retrieval = include_str!("../src/retrieval.rs");
        let selection_start = retrieval
            .find("async fn select_retrieve_peer(")
            .expect("retrieve peer selector");
        let selection_end = retrieval[selection_start..]
            .find("\nfn reset_overdraft(")
            .map(|offset| selection_start + offset)
            .expect("retrieve peer selector end");
        let selection = &retrieval[selection_start..selection_end];
        assert!(
            include_str!("../src/lib.rs")
                .contains("overlay_peers: Arc<Mutex<HashMap<Vec<u8>, PeerId>>>")
        );
        assert!(selection.contains("if peers_map.is_empty()"));
        assert!(selection.contains("for (overlay, id) in peers_map.iter()"));
        assert!(selection.contains("current_po >= current_max_po || closest_peer_id.is_none()"));
        assert!(!selection.contains("peer_candidates"));
        assert!(!selection.contains(".collect()"));
        assert!(!selection.contains("hex::decode"));
        assert!(!selection.contains("CONNECTION_BUILDUP_LIMIT"));
        assert!(!selection.contains("CONNECTION_DIAL_CONCURRENCY_LIMIT"));
    }

    #[test]
    fn dial_storm_can_fill_but_never_exceed_the_peer_population() {
        assert!(connection_dial_capacity_available(
            0,
            CONNECTION_BUILDUP_LIMIT - 1
        ));
        assert!(!connection_dial_capacity_available(
            0,
            CONNECTION_BUILDUP_LIMIT
        ));
        assert!(!connection_dial_capacity_available(
            1,
            CONNECTION_BUILDUP_LIMIT - 1
        ));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT,
            0
        ));
        assert!(!connection_dial_capacity_available(u64::MAX, u64::MAX));

        let runtime = include_str!("../src/lib.rs");
        let feeder = runtime
            .split("let swarm_event_handle_0 = async")
            .nth(1)
            .and_then(|source| source.split("let swarm_event_handle_1 = async").next())
            .expect("peer dial feeder");
        assert!(feeder.contains("VecDeque::<QueuedPeerDial>::new()"));
        assert!(feeder.contains("HashSet::<(PeerId, Multiaddr)>::new()"));
        assert!(feeder.contains("try_reserve_connection_capacity("));
        assert!(feeder.contains("fresh_dials_since_retry >= FRESH_PEER_DIALS_PER_RETRY"));
        assert!(
            feeder
                .contains("take_eligible(&mut new_peers).or_else(|| take_eligible(&mut retries))")
        );
        assert!(feeder.contains("queue_peer_dial_retry("));
        assert!(runtime.contains("mpsc::bounded::<PeerDialInstruction>(MAX_QUEUED_PEER_DIALS)"));
        assert!(
            runtime.contains("remove_connection_attempt_for_dial(&wings, &peer_id, connection_id)")
        );
        let failed_dial = runtime
            .split("let retryable = !matches!(")
            .nth(1)
            .and_then(|source| source.split("SwarmEvent::ConnectionClosed {").next())
            .expect("outgoing dial error");
        let removal = failed_dial
            .find("remove_connection_attempt_for_dial")
            .expect("exact dial removal");
        let release = failed_dial
            .find("decrement_counter(&ongoing_connections)")
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
        assert_eq!(connection_population_deficit(55, 145), 0);
        let mut ongoing = 145_u64;
        for _ in 0..145 {
            ongoing = ongoing.saturating_sub(1);
        }
        assert_eq!(ongoing, 0);
        assert_eq!(connection_population_deficit(55, ongoing), 145);
        assert_eq!(connection_population_deficit(199, 0), 1);
        assert_eq!(connection_population_deficit(200, 0), 0);
        assert_eq!(connection_population_deficit(u64::MAX, u64::MAX), 0);
    }

    #[test]
    fn delayed_retry_backoff_is_registered_and_released_around_enqueue() {
        let runtime = include_str!("../src/lib.rs");
        let retry = runtime
            .split("async fn queue_peer_dial_retry(")
            .nth(1)
            .and_then(|source| source.split("fn failed_peer_retry_delay_ms(").next())
            .expect("delayed retry scheduler");
        let register = retry
            .find(".insert(peer, (expected_generation, retry_id))")
            .expect("retry not-before registration");
        let sleep = retry
            .find("failed_peer_retry_delay_ms(&address)")
            .expect("configured retry backoff");
        let ownership = retry
            .find("delayed.get(&peer) == Some(&(expected_generation, retry_id))")
            .expect("exact delayed retry ownership");
        let enqueue = retry
            .find(".send(PeerDialInstruction {")
            .expect("retry enqueue after backoff");
        let release = retry[enqueue..]
            .find("delayed.get(&peer) == Some(&(expected_generation, retry_id))")
            .map(|offset| enqueue + offset)
            .expect("retry exclusion release after enqueue");
        assert!(register < sleep && sleep < ownership && ownership < enqueue && enqueue < release);

        let feeder = runtime
            .split("let swarm_event_handle_0 = async")
            .nth(1)
            .and_then(|source| source.split("let swarm_event_handle_1 = async").next())
            .expect("peer dial feeder");
        assert!(feeder.contains("wings.delayed_peer_retries"));
        assert!(feeder.contains("*generation == queue_generation"));
        assert!(feeder.contains("retry.0 == queue_generation"));
    }

    #[test]
    fn stalled_pre_handshake_reservations_expire_and_known_peers_are_rescanned() {
        let runtime = include_str!("../src/lib.rs");
        assert!(runtime.contains("const PRE_HANDSHAKE_CONNECTION_TIMEOUT_MS: u64 = 60_000;"));
        assert!(runtime.contains("const PEER_POPULATION_RESCAN_MS: u64 = 2_000;"));

        let joiner = runtime
            .split("let hive_joiner = async")
            .nth(1)
            .and_then(|source| source.split("join!(").next())
            .expect("connection handshake joiner");
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
            .find("decrement_counter(&ongoing_connections)")
            .map(|offset| retry + offset)
            .expect("single ongoing-counter release");
        assert!(ready_timeout < ownership && ownership < ready_wait);
        assert!(ready_wait < removal && removal < retry && retry < release);

        let feeder = runtime
            .split("let swarm_event_handle_0 = async")
            .nth(1)
            .and_then(|source| source.split("let swarm_event_handle_1 = async").next())
            .expect("peer dial feeder");
        assert!(feeder.contains("current_connection_population_deficit("));
        assert!(feeder.contains("peers_instructions_chan_incoming.recv()"));
        assert!(feeder.contains("last_population_rescan_ms"));
        assert!(feeder.contains("wings.known_peer_underlays.lock().await"));
        assert!(feeder.contains("wings.known_peer_generations.lock().await.clone()"));
        assert!(feeder.contains("!connected.contains(*peer)"));
        assert!(feeder.contains("!attempts.contains(*peer)"));
        assert!(feeder.contains("!cooldowns.contains(*peer)"));
        assert!(feeder.contains("eligible.shuffle(&mut rand::thread_rng())"));
        assert!(feeder.contains("eligible.truncate(deficit)"));
        assert!(feeder.contains("retry: true"));

        let timeout_cleanup = &joiner[ready_wait..];
        let disconnect = timeout_cleanup
            .find("swarm.disconnect_peer_id(id).is_err()")
            .expect("pending transport abort");
        let removal = timeout_cleanup
            .find("remove_connection_attempt(&wings, &id, connection_attempt_id)")
            .expect("exact reservation removal");
        let release = timeout_cleanup
            .find("decrement_counter(&ongoing_connections)")
            .expect("reservation counter release");
        assert!(disconnect < removal && removal < release);

        let outgoing_error = runtime
            .split("let retryable = !matches!(")
            .nth(1)
            .and_then(|source| source.split("SwarmEvent::ConnectionClosed {").next())
            .expect("late outgoing dial failure");
        let late_removal = outgoing_error
            .find("if !remove_connection_attempt_for_dial(")
            .expect("late event ownership guard");
        let late_return = outgoing_error[late_removal..]
            .find("return;")
            .map(|offset| late_removal + offset)
            .expect("stale event return");
        let late_release = outgoing_error
            .find("decrement_counter(&ongoing_connections)")
            .expect("owned dial failure release");
        assert!(late_removal < late_return && late_return < late_release);
    }

    #[test]
    fn duplicate_overlay_peers_are_suppressed_until_the_owner_disconnects() {
        let runtime = include_str!("../src/lib.rs");
        let promotion = runtime
            .split("async fn promote_priced_peer(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub async fn post_upload_with_redundancy(")
                    .next()
            })
            .expect("priced peer promotion");
        let duplicate = promotion
            .split("} else if let Some(owner) = duplicate_owner {")
            .nth(1)
            .expect("duplicate overlay rejection");
        let reject = duplicate
            .find(".insert(peer, owner)")
            .expect("owner-bound duplicate suppression");
        let disconnect = duplicate
            .find("swarm.disconnect_peer_id(peer.clone())")
            .expect("duplicate disconnect");
        assert!(reject < disconnect);

        let feeder = runtime
            .split("let swarm_event_handle_0 = async")
            .nth(1)
            .and_then(|source| source.split("let swarm_event_handle_1 = async").next())
            .expect("peer dial feeder");
        assert!(feeder.contains("!rejected.contains_key(*peer)"));
        assert!(feeder.contains("rejected.contains_key(&candidate.peer)"));
        assert!(feeder.contains("rejected_peers.contains_key(&candidate.peer)"));
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
        assert!(profile.contains("pub(crate) const INITIAL_BOOTNODE_COUNT: usize = 160;"));
        assert!(
            profile.find("bootnodes.shuffle(&mut rand::thread_rng())")
                < profile.find("bootnodes.truncate(INITIAL_BOOTNODE_COUNT)")
        );

        let runtime = include_str!("../src/lib.rs");
        let address_filter = include_str!("../src/addresses.rs");
        assert!(runtime.contains("is_publicly_dialable_underlay(&source_addr)"));
        assert!(runtime.contains("!self.allow_private_gossip.load(Ordering::Acquire)"));
        assert!(
            runtime
                .contains("profile_for_swarm_network_id(*self.network_id.lock().await).is_some()")
        );
        assert!(address_filter.contains("|embedded| embedded == address"));
        assert!(address_filter.contains(".map(is_public_ipv4)"));
        assert!(address_filter.contains(".unwrap_or_else(|| is_public_dns_name(&hostname))"));
        assert!(address_filter.contains("!hostname.ends_with(\".local\")"));
        assert!(include_str!("../src/handlers.rs").contains("underlay: peer.underlay,"));
    }

    #[test]
    fn cold_bootnode_burst_drains_into_one_dispatch_task() {
        let runtime = include_str!("../src/lib.rs");
        let profile = include_str!("../src/network_profile.rs");
        let accounting = include_str!("../src/accounting.rs");
        let handler = runtime
            .split("let swarm_event_handle_2 = async")
            .nth(1)
            .and_then(|source| source.split("let accounting_event_handle = async").next())
            .expect("bootnode dial handler");

        assert!(profile.contains("pub(crate) const INITIAL_BOOTNODE_COUNT: usize = 160;"));
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
        let runtime = include_str!("../src/lib.rs");
        let handlers = include_str!("../src/handlers.rs");
        assert!(runtime.contains("handshake_signer: Arc<PrivateKeySigner>"));
        assert_eq!(runtime.matches("PrivateKeySigner::from_slice(").count(), 1);
        assert!(!handlers.contains("PrivateKeySigner::from_slice("));
    }

    #[test]
    fn bee_handshake_waits_for_the_exact_identify_push() {
        let runtime = include_str!("../src/lib.rs");
        let received = runtime
            .split("identify::Event::Received {")
            .nth(1)
            .and_then(|source| source.split("identify::Event::Pushed {").next())
            .expect("identify receive lifecycle");
        assert!(received.contains("pending_identify_addresses"));
        assert!(received.contains("identify_push_capacity.acquire_arc().await"));
        assert!(received.contains("info.observed_addr.clone()"));
        assert!(received.contains("swarm.add_external_address(info.observed_addr)"));
        assert!(received.contains(".identify\n                                        .push("));
        assert!(!received.contains("mark_handshake_ready_connection("));

        let pushed = runtime
            .split("identify::Event::Pushed {")
            .nth(1)
            .and_then(|source| source.split("identify::Event::Error {").next())
            .expect("identify push lifecycle");
        let ready = pushed
            .find("mark_handshake_ready_connection(")
            .expect("Bee handshake readiness");
        let cleanup = pushed
            .find("spawn_local(async move")
            .expect("asynchronous transient address cleanup");
        assert!(ready < cleanup);
        assert!(pushed.contains("Some(&info.listen_addrs)"));
        assert!(pushed.contains("remove_unreferenced_identify_address("));
        assert!(runtime.contains("const IDENTIFY_PUSH_CONCURRENCY: usize = 32;"));
        assert!(runtime.contains("const IDENTIFY_PUSH_TIMEOUT_MS: u64 = 5_000;"));
        assert!(received.contains("IDENTIFY_PUSH_TIMEOUT_MS"));
        assert!(received.contains("remove_pending_identify_address("));
        assert!(received.contains(".close_connection(connection_id)"));
        assert!(runtime.contains("SwarmEvent::Behaviour(BehaviourEvent::Identify(_))"));
        let safe_cleanup = runtime
            .split("async fn remove_unreferenced_identify_address(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("async fn close_failed_identify_connection(")
                    .next()
            })
            .expect("reference-safe external address cleanup");
        assert!(
            safe_cleanup.find("let mut swarm = swarm.lock().await;")
                < safe_cleanup.find("pending_identify_addresses")
        );
        assert!(safe_cleanup.contains("if !referenced"));
        assert!(safe_cleanup.contains("swarm.remove_external_address(address)"));

        let empty_observed = received
            .split("if info.observed_addr.is_empty() {")
            .nth(1)
            .expect("empty Identify observation handling");
        assert!(empty_observed.contains("close_failed_identify_connection("));
        let identify_error = runtime
            .split("identify::Event::Error {")
            .nth(1)
            .and_then(|source| source.split("SwarmEvent::OutgoingConnectionError").next())
            .expect("Identify error lifecycle");
        assert!(identify_error.contains("close_failed_identify_connection("));

        let exact_close = runtime
            .split("async fn close_failed_identify_connection(")
            .nth(1)
            .and_then(|source| source.split("async fn remove_connection_attempt(").next())
            .expect("exact failed Identify close");
        assert!(exact_close.contains("attempt.physical_connection_id == Some(connection_id)"));
        assert!(exact_close.contains("!attempt.identify_failed"));
        assert!(exact_close.contains("handshake_ready_connections"));
        assert!(exact_close.contains("connections.contains(&connection_id)"));
        assert!(exact_close.contains("attempt.identify_failed = true;"));
        assert!(exact_close.contains("swarm.lock().await.close_connection(connection_id)"));
    }

    #[test]
    fn only_private_custom_bootnodes_enable_private_gossip() {
        let runtime = include_str!("../src/lib.rs");
        let connect = runtime
            .split("pub(crate) async fn connect_bootnodes_for_current_network(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) async fn acquire_feed_envelope(")
                    .next()
            })
            .expect("bootnode connection setup");
        assert!(connect.contains("is_private_or_local_bootnode(address)"));
        assert!(connect.contains("!profile.bootnodes.contains(&address.as_str())"));
        assert!(connect.contains(".store(private_custom_bootnodes, Ordering::Release)"));
        assert!(!connect.contains(".store(custom_bootnodes, Ordering::Release)"));

        let private_check = runtime
            .split("fn is_private_or_local_bootnode(")
            .nth(1)
            .and_then(|source| source.split("pub(crate) fn chunk_retrieve_request(").next())
            .expect("private bootnode classification");
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
        let runtime = include_str!("../src/lib.rs");
        let accounting = runtime
            .split("let accounting_event_handle = async")
            .nth(1)
            .and_then(|source| source.split("let pricing_event_handle = async").next())
            .expect("accounting connection lifecycle should remain inspectable");
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
            .find("connected_peers.insert(peer.clone(), peer_file.clone())")
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
                && accounting_arc < connected
                && connected < threshold
                && threshold < promotion,
            "a handshake must own physical and counted capacity before publication and promotion"
        );
        assert!(
            accounting.contains("let accounting_peer_for_timeout = accounting_peer_lock.clone();")
        );
        assert!(accounting.contains("Arc::ptr_eq("));
        assert!(accounting.contains("peer_file.connection_attempt_id == timeout_attempt_id"));
        assert!(accounting.contains("timeout_attempt_id"));

        let promotion = runtime
            .split("async fn promote_priced_peer(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub async fn post_upload_with_redundancy(")
                    .next()
            })
            .expect("priced-peer promotion should remain inspectable");
        let connected_guard = promotion
            .find("let connected_peers_guard = wings.connected_peers.lock().await;")
            .expect("disconnect serialization guard");
        let physical = promotion
            .find("exclusive_physical_connection(&wings.physical_connections, &peer)")
            .expect("physical connection validation");
        let reservation_owner = promotion
            .find("attempts.get(&peer).map(|attempt| attempt.id)")
            .expect("attempt ownership validation");
        let reservation_remove = promotion
            .find("attempts.remove(&peer)")
            .expect("reservation transfer");
        let overlay_publish = promotion
            .find("overlay_peers_map.insert(overlay.clone(), peer.clone())")
            .expect("overlay publication");
        let counter_transfer = promotion
            .find("let mut connections = self.connections.lock().await;")
            .expect("counter transfer");
        let ongoing_transfer = promotion
            .find("let mut ongoing = self.ongoing_connections.lock().await;")
            .expect("ongoing-counter transfer");
        let connected_drop = promotion
            .find("drop(connected_peers_guard);")
            .expect("disconnect serialization release");
        let duplicate_cleanup = promotion
            .find("let mut connected = wings.connected_peers.lock().await;")
            .expect("duplicate cleanup");
        assert!(
            connected_guard < physical
                && physical < reservation_owner
                && reservation_owner < reservation_remove
                && reservation_remove < overlay_publish
                && overlay_publish < counter_transfer
                && counter_transfer < ongoing_transfer
                && ongoing_transfer < connected_drop
                && connected_drop < duplicate_cleanup,
            "disconnect must not interleave the reservation, overlay, and counter transfer"
        );
    }

    #[test]
    fn inbound_pricing_is_bound_to_the_exact_transport_session() {
        let runtime = include_str!("../src/lib.rs");
        let inbound = runtime
            .split("let pricing_inbound_handle = async move")
            .nth(1)
            .and_then(|source| source.split("let gossip_peers_instructions").next())
            .expect("inbound pricing lifecycle should remain inspectable");
        assert!(inbound.contains("exclusive_physical_connection("));
        assert!(inbound.contains("TransportConnectionSession::capture("));
        assert!(inbound.contains(
            "pricing_handler(peer, stream, pricing_session, &pricing_chan_outgoing).await"
        ));

        let handler_source = include_str!("../src/handlers.rs");
        let handler = handler_source
            .split("pub async fn pricing_handler(")
            .nth(1)
            .and_then(|source| source.split("pub async fn gossip_handler(").next())
            .expect("pricing handler should remain inspectable");
        assert!(handler.contains("session: TransportConnectionSession"));
        assert!(handler.contains("if !session.is_current()"));
        assert!(handler.contains("chan.try_send((peer, pt, session))"));

        let application = runtime
            .split("let pricing_event_handle = async")
            .nth(1)
            .and_then(|source| source.split("let cheques_active_cache").next())
            .expect("pricing application should remain inspectable");
        assert!(application.contains("let (peer, amount, pricing_session) = pricing;"));
        assert!(application.contains("pricing_session.is_current()"));
        assert!(
            application.contains("expected_connection == Some(pricing_session.connection_id())")
        );
    }

    #[test]
    fn only_the_connection_that_owns_the_session_tears_down_peer_state() {
        let runtime = include_str!("../src/lib.rs");
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

        let runtime = include_str!("../src/lib.rs");
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
            .find("*connection_generation.lock().await == retry_generation")
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

        let reservation = runtime
            .split("async fn try_mark_connection_attempt(")
            .nth(1)
            .and_then(|source| source.split("async fn remove_connection_attempt(").next())
            .expect("connection attempt reservation");
        assert!(reservation.contains("connection_cooldowns.contains(peer)"));
    }

    #[test]
    fn refresh_settlement_coalesces_per_account_and_rearms_until_debt_is_clear() {
        let runtime = include_str!("../src/lib.rs");
        let instruction = runtime
            .split("let refreshment_instruction_handle = async")
            .nth(1)
            .and_then(|source| source.split("let swap_price =").next())
            .expect("refresh instruction lifecycle");
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
        assert!(instruction.contains("if !refreshment_due("));
        assert!(instruction.contains("account.threshold,"));
        assert!(instruction.contains("account.refresh_scheduled = false;"));
        assert!(instruction.contains("RefreshmentOutcome::NotDispatched => {"));
        assert!(instruction.contains("RefreshmentOutcome::Acknowledged(0)"));
        assert!(instruction.contains("RefreshmentOutcome::AmbiguousAfterPayment"));
        assert!(instruction.contains("account.balance = 0;"));
        assert!(instruction.contains("account.threshold"));
        assert!(instruction[dispatch..].contains("attempted_amount,"));
        assert!(instruction.contains("quiesce_drain_and_close_accounting_session("));
        assert!(!instruction.contains("REFRESH_RATE * 100"));
        assert!(!instruction.contains("Duration::from_secs(15)"));
        assert!(!runtime.contains("ongoing_refreshments"));
        assert!(!runtime.contains("refreshment_apply_handle"));

        let ambiguous_close = runtime
            .split("async fn quiesce_drain_and_close_accounting_session(")
            .nth(1)
            .and_then(|source| source.split("impl Weeb3 {").next())
            .expect("draining exact-session close");
        let quiesce = ambiguous_close
            .find("account.connection_id = None;")
            .expect("new reservation quiescence");
        let drain = ambiguous_close
            .find("accounting_peer.lock().await.reserve == 0")
            .expect("dispatched accounting drain");
        let close = ambiguous_close
            .find("swarm.close_connection(connection_id)")
            .expect("exact physical close");
        assert!(quiesce < drain && drain < close);
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
    fn first_refresh_accumulates_two_seconds_then_the_normal_rate_applies() {
        assert!(!refreshment_due(0, 0.0, REFRESH_RATE * 3));
        assert!(!refreshment_due(REFRESH_RATE, 0.0, REFRESH_RATE * 3));
        assert!(refreshment_due(REFRESH_RATE * 2, 0.0, REFRESH_RATE * 3));
        assert!(refreshment_due(REFRESH_RATE, 0.0, REFRESH_RATE));
        assert!(!refreshment_due(REFRESH_RATE, 0.0, REFRESH_RATE + 123));
        assert!(refreshment_due(REFRESH_RATE + 123, 0.0, REFRESH_RATE + 123));
        assert!(!refreshment_due(REFRESH_RATE - 1, 1.0, REFRESH_RATE * 3));
        assert!(refreshment_due(REFRESH_RATE, 1.0, REFRESH_RATE * 3));
    }

    #[test]
    fn original_refresh_interface_logs_use_the_bounded_log() {
        let runtime = include_str!("../src/lib.rs");
        let instruction = runtime
            .split("let refreshment_instruction_handle = async")
            .nth(1)
            .and_then(|source| source.split("let swap_price =").next())
            .expect("refresh instruction lifecycle");

        assert!(runtime.contains("mpsc::bounded::<String>(LOG_QUEUE_CAPACITY)"));
        assert!(instruction.contains("interface_log_to("));
        for marker in [
            "Applied refreshment {}",
            "Refreshment attempt cleared {}",
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
        let runtime = include_str!("../src/lib.rs");
        let retrieve = runtime
            .split("let retrieve_chunk_handle = async")
            .nth(1)
            .and_then(|source| source.split("let hive_joiner = async").next())
            .expect("retrieve dispatcher");
        assert!(retrieve.contains("let retrieve_dispatch_yield_every = 128usize;"));
        assert!(retrieve.contains("let mut retrieve_dispatches_since_browser_yield = 0usize;"));
        assert!(retrieve.contains("async_std::task::sleep(Duration::ZERO).await;"));
        assert!(!retrieve.contains("wave_done"));
        assert!(!retrieve.contains("Completed {} of {} chunk retrieval requests"));

        let refresh = runtime
            .split("let refreshment_instruction_handle = async")
            .nth(1)
            .and_then(|source| source.split("let swap_price =").next())
            .expect("refresh dispatcher");
        assert!(refresh.contains("refresh_dispatches % 8 == 0"));
        assert!(refresh.contains("async_std::task::sleep(Duration::ZERO).await;"));

        let accounting = include_str!("../src/accounting.rs");
        assert!(
            accounting.contains("async_std::task::sleep(std::time::Duration::ZERO).await"),
            "a threshold-crossing completion must return to the browser event loop"
        );
    }

    #[test]
    fn handshake_and_cheque_lifecycles_are_session_bound() {
        let runtime = include_str!("../src/lib.rs");
        assert!(runtime.contains("Duration::from_millis(HANDSHAKE_PROTOCOL_TIMEOUT_MS)"));

        let handlers = include_str!("../src/handlers.rs");
        assert!(handlers.contains("let Some(ack) = rec_0.ack.as_ref()"));
        assert!(handlers.contains("let Some(peer_address) = ack.address.as_ref()"));
        let handshake = handlers
            .split("pub async fn ceive(")
            .nth(1)
            .and_then(|source| source.split("pub async fn pricing_handler(").next())
            .expect("handshake initiator");
        assert!(handshake.contains("deserialize_underlays(&syn.observed_underlay)"));
        assert!(handshake.contains("try_from_multiaddr(underlay).as_ref() != Some(&local_peer)"));
        assert!(handshake.contains("let underlay = syn.observed_underlay.clone();"));
        assert!(handshake.contains("if ack.network_id != network_id"));

        let connection = handlers
            .split("pub async fn connection_handler(")
            .nth(1)
            .and_then(|source| source.split("pub async fn refresh_handler(").next())
            .expect("handshake connection binding");
        let open = connection
            .find("control.open_stream(")
            .expect("stream open");
        let capture = connection
            .find("TransportConnectionSession::capture(")
            .expect("physical session capture");
        assert!(open < capture);
        assert!(connection.contains("peer, connection_id, physical_connections"));
        assert!(!runtime.contains("self_ephemerals"));
        assert!(handlers.contains("read_control_protocol_frame(&mut stream).await"));
        assert!(handlers.contains("stream.read_exact(&mut frame).await"));
        assert!(handlers.contains("enum RefreshmentOutcome"));
        assert!(handlers.contains("if refr_am > amount"));
        assert!(handlers.contains("RefreshmentOutcome::AmbiguousAfterPayment"));
        let refresh_handler = handlers
            .split("pub async fn refresh_handler(")
            .nth(1)
            .and_then(|source| source.split("pub async fn issue_handler(").next())
            .expect("refresh dispatch");
        assert!(!refresh_handler.contains("timeout("));
        let pricing = handlers
            .split("pub async fn pricing_handler(")
            .nth(1)
            .and_then(|source| source.split("pub async fn gossip_handler(").next())
            .expect("pricing handler");
        assert_eq!(
            pricing
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2
        );
        let refresh = handlers
            .split("pub async fn fresh(")
            .nth(1)
            .and_then(|source| source.split("pub async fn issue(").next())
            .expect("refresh handler");
        assert_eq!(
            refresh
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2
        );
        assert!(!handlers.contains("rec_0.ack.clone().unwrap()"));

        assert!(runtime.contains("cheques.insert("));
        assert!(runtime.contains("claim_current_cheque("));
        assert!(runtime.contains("(cheque_amt, cheque_generation)"));
        assert!(runtime.contains("map.get(&peer).copied() == Some((amount, cheque_generation))"));
        assert!(runtime.contains("generation == cheque_generation"));

        let cheque_claim = runtime
            .split("async fn claim_current_cheque(")
            .nth(1)
            .and_then(|source| source.split("#[wasm_bindgen]").next())
            .expect("exact-session cheque claim");
        let lifecycle = cheque_claim
            .find("wings.connected_peers.lock().await")
            .expect("peer lifecycle guard");
        let account = cheque_claim
            .find("wings.accounting_peers.lock().await")
            .expect("exact account lookup");
        let claims = cheque_claim
            .find("wings.ongoing_cheques.lock().await")
            .expect("cheque claim map");
        let physical = cheque_claim
            .find("exclusive_physical_connection(")
            .expect("last physical-session check");
        let publish = cheque_claim
            .find("cheques.insert(")
            .expect("claim publication");
        assert!(lifecycle < account && account < claims && claims < physical && physical < publish);

        let cheque_dispatch = runtime
            .split("let cheque_instruction_handle = async")
            .nth(1)
            .and_then(|source| source.split("let cheque_apply_handle = async").next())
            .expect("cheque dispatch lifecycle");
        let capture = cheque_dispatch
            .find("OutboundProtocolSession::capture(")
            .expect("cheque transport-session capture");
        let post_capture_claim = cheque_dispatch[capture..]
            .find("map.get(&peer).copied() == Some((amount, cheque_generation))")
            .map(|offset| capture + offset)
            .expect("post-capture cheque claim validation");
        let dispatch = cheque_dispatch[post_capture_claim..]
            .find("issue_handler(")
            .map(|offset| post_capture_claim + offset)
            .expect("cheque protocol dispatch");
        assert!(
            capture < post_capture_claim && post_capture_claim < dispatch,
            "a stale cheque claim must not cross onto a replacement peer session"
        );
    }

    #[test]
    fn retrieval_reads_complete_length_delimited_frames_despite_transport_fragmentation() {
        let handlers = include_str!("../src/handlers.rs");
        let retrieval = handlers
            .split("pub async fn trieve(")
            .nth(1)
            .and_then(|source| source.split("pub async fn connection_handler(").next())
            .expect("retrieval protocol handler");

        assert_eq!(
            retrieval
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2,
            "both Headers and Delivery must use exact length-delimited framing"
        );
        assert!(!retrieval.contains("stream.read("));
        assert!(retrieval.contains("etiquette_6::Delivery::decode("));
        assert!(!retrieval.contains("Delivery::decode_length_delimited"));
    }

    #[test]
    fn hive_reads_complete_length_delimited_frames_despite_transport_fragmentation() {
        let handlers = include_str!("../src/handlers.rs");
        let hive = handlers
            .split("pub async fn gossip_handler(")
            .nth(1)
            .and_then(|source| source.split("pub async fn fresh(").next())
            .expect("Hive protocol handler");

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
            .split("pub async fn sync(")
            .nth(1)
            .expect("pushsync protocol handler");

        assert_eq!(
            pushsync
                .matches("read_control_protocol_frame(&mut stream).await")
                .count(),
            2,
            "both Headers and Receipt must use exact length-delimited framing"
        );
        assert!(pushsync.contains("stream.write_all(&bufw_1).await"));
        assert!(!pushsync.contains("stream.write(&bufw_1"));
        assert!(!pushsync.contains("stream.read("));
        assert!(pushsync.contains("etiquette_7::Receipt::decode("));
        assert!(!pushsync.contains("Receipt::decode_length_delimited"));
    }

    #[test]
    fn accounting_protocol_streams_cannot_implicitly_redial_or_cross_sessions() {
        let runtime = include_str!("../src/lib.rs");
        assert!(runtime.contains("Poll::Ready(ToSwarm::Dial { opts })"));
        assert!(runtime.contains("FromSwarm::DialFailure(DialFailure"));
        assert!(runtime.contains("let error = DialError::NoAddresses;"));

        let handlers = include_str!("../src/handlers.rs");
        let open_current = handlers
            .split("async fn open_current_outbound_stream(")
            .nth(1)
            .and_then(|source| source.split("pub async fn refresh_handler(").next())
            .expect("session-bound stream helper");
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
        let reserve = retrieval
            .find("reserve(&accounting_peer, req_price).await")
            .expect("retrieve reserve");
        let capture = retrieval[reserve..]
            .find("OutboundProtocolSession::capture(")
            .map(|offset| reserve + offset)
            .expect("retrieve session capture");
        let dispatch = retrieval[capture..]
            .find("retrieve_handler(")
            .map(|offset| capture + offset)
            .expect("retrieve dispatch");
        assert!(reserve < capture && capture < dispatch);

        let upload = include_str!("../src/upload.rs");
        assert!(upload.contains("OutboundProtocolSession::capture("));
        assert!(upload.contains("pushsync_handler("));
    }
}

mod retrieve_group_stream {
    const RETRIEVAL_SOURCE: &str = include_str!("../src/retrieval.rs");

    fn source_section(start: &str, end: &str) -> &'static str {
        let start = RETRIEVAL_SOURCE
            .find(start)
            .unwrap_or_else(|| panic!("missing retrieval source marker: {start}"));
        let tail = &RETRIEVAL_SOURCE[start..];
        let end = tail
            .find(end)
            .unwrap_or_else(|| panic!("missing retrieval source marker: {end}"));
        &tail[..end]
    }

    #[test]
    fn requested_children_are_published_before_group_terminal_completion() {
        let group = source_section(
            "async fn fetch_data_group_indices_streaming(",
            "#[derive(Clone)]\nstruct TraversalNode",
        );
        assert_eq!(
            group.matches("child_emitter.emit(").count(),
            4,
            "rolling cache variants, the legacy cache fast path, and reconstruction all publish"
        );
        assert!(
            group.contains("if requested_count == data_count")
                && group.contains("dispatch_group_parity(")
                && group.contains("usize::MAX"),
            "the conservative legacy fallback must retain its full-group parity hedge"
        );

        let traversal = source_section(
            "async fn retrieve_data_range_from_root_with_prefix_cancellable(",
            "async fn retrieve_data_joined(",
        );
        let fetch = traversal
            .find("fetch_data_group_indices_streaming(")
            .expect("streaming group fetch");
        let terminal = traversal
            .find("terminal_emitter.finish(success)")
            .expect("terminal event");
        assert!(
            fetch < terminal,
            "terminal must follow every child emission"
        );
        assert!(
            !traversal.contains("spawn_local"),
            "group coordinators must remain owned so dropping the join closes admission guards"
        );
    }

    #[test]
    fn unconsumed_terminals_keep_the_join_alive_and_failure_is_all_or_nothing() {
        let traversal = source_section(
            "async fn retrieve_data_range_from_root_with_prefix_cancellable(",
            "async fn retrieve_data_joined(",
        );
        assert!(traversal.contains("while !pending.is_empty() || active_groups > 0"));
        assert!(traversal.contains("active_groups = active_groups.checked_add(1)?"));
        assert!(traversal.contains("active_groups = active_groups.checked_sub(1)?"));

        let terminal_branch = source_section(
            "GroupFetchEvent::Terminal { success } => {",
            "GroupFetchEvent::Child {",
        );
        assert!(
            terminal_branch.contains("if !success") && terminal_branch.contains("return None"),
            "a terminal group failure must reject the complete join"
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
    fn conservative_ranges_are_serial_direct_and_bounded() {
        let policies = source_section(
            "struct DataRangeTraversalPolicy",
            "struct RetrieveAttemptResult",
        );
        assert!(policies.contains("group_concurrency: RETRIEVE_DATA_GROUP_CONCURRENCY"));
        assert!(policies.contains("erasure_recovery: true"));
        assert!(policies.contains("maximum_requested_children: usize::MAX"));
        assert!(policies.contains("group_concurrency: 1"));
        assert!(policies.contains("erasure_recovery: false"));
        assert!(policies.contains("maximum_requested_children: CONSERVATIVE_RANGE_MAX_CHILDREN"));
        assert!(
            RETRIEVAL_SOURCE
                .contains("CONSERVATIVE_RANGE_MAX_CHILDREN * RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS")
        );

        let group = source_section(
            "async fn fetch_data_group_indices_streaming(",
            "#[derive(Clone)]\nstruct TraversalNode",
        );
        assert!(group.contains("requested_indices.len() > policy.maximum_requested_children"));
        assert!(group.contains("recovery_dispatched && policy.erasure_recovery"));
        assert!(group.contains("|| !policy.erasure_recovery"));
        assert!(group.contains("let waiter_admission = RetrieveAdmission::new();"));
        assert!(group.contains("shared_physical_admission"));

        let raw_flights =
            source_section("struct RawFetchKey", "#[inline]\nfn decryption_segment_key");
        assert!(raw_flights.contains("conservative_scope: Option<u64>"));
        assert!(raw_flights.contains("shared_physical_admission"));
        assert!(raw_flights.contains("admission.clone()"));
        assert!(raw_flights.contains("key.conservative_scope.is_none()"));

        let conservative = source_section(
            "pub(crate) async fn retrieve_data_range_from_root_conservative(",
            "pub(crate) async fn retrieve_data_range_from_root_cancellable(",
        );
        assert!(conservative.contains("requested_len > CONSERVATIVE_RANGE_BYTES"));
        assert!(conservative.contains("CONSERVATIVE_DATA_RANGE_TRAVERSAL"));

        let traversal = source_section(
            "async fn retrieve_data_range_from_root_with_prefix_cancellable(",
            "async fn retrieve_data_joined(",
        );
        assert!(traversal.contains("RetrieveAdmission::new_with_attempt_limit("));
        assert!(traversal.contains("CONSERVATIVE_RANGE_MAX_PHYSICAL_ATTEMPTS"));
        assert!(traversal.contains("admission.close_on_drop()"));
        assert!(traversal.contains("group_shared_physical_admission"));

        let ordinary = source_section(
            "pub(crate) async fn retrieve_data_range_from_root_cancellable(",
            "async fn retrieve_data_range_from_root_with_prefix_cancellable(",
        );
        assert!(ordinary.contains("ORDINARY_DATA_RANGE_TRAVERSAL"));
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;"));
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_CHUNK_MAX_ATTEMPT_ERRORS: usize = 20;"));
        assert!(
            RETRIEVAL_SOURCE
                .contains("pub(crate) const CONSERVATIVE_RANGE_MAX_PHYSICAL_ATTEMPTS: u64")
        );
    }
}

mod rolling_erasure_tail {
    use crate::retrieval_conventions::{
        RetrieveHedgeDemand, SharedRetrieveHedgeDemand, retrieve_attempt_start_allowed,
        rolling_full_group_eligible, rolling_full_group_static_candidate,
        rolling_next_parity_index, rolling_parity_admission_count,
    };

    const GATE_MS: u64 = 1_000;
    const RETRIEVAL_SOURCE: &str = include_str!("../src/retrieval.rs");
    const RUNTIME_SOURCE: &str = include_str!("../src/lib.rs");

    fn group_source() -> &'static str {
        let start = RETRIEVAL_SOURCE
            .find("async fn fetch_data_group_indices_streaming(")
            .expect("group fetch start");
        let end = RETRIEVAL_SOURCE[start..]
            .find("#[derive(Clone)]\nstruct TraversalNode")
            .map(|offset| start + offset)
            .expect("group fetch end");
        &RETRIEVAL_SOURCE[start..end]
    }

    #[test]
    fn rolling_requires_a_full_recoverable_mixed_cache_group() {
        // A production Medium full group is 119 data + 9 parity. Eight decoded-only
        // hits leave one parity beyond the cache-basis deficit; nine do not.
        assert!(rolling_full_group_eligible(true, 119, 119, 9, 0, 119));
        assert!(rolling_full_group_eligible(true, 119, 119, 9, 8, 1));

        assert!(!rolling_full_group_eligible(false, 119, 119, 9, 0, 119));
        assert!(!rolling_full_group_eligible(true, 118, 119, 9, 0, 118));
        assert!(!rolling_full_group_eligible(true, 119, 119, 0, 0, 119));
        assert!(!rolling_full_group_eligible(true, 119, 119, 9, 9, 1));
        assert!(!rolling_full_group_eligible(true, 119, 119, 9, 0, 0));
    }

    #[test]
    fn partial_and_conservative_groups_never_pay_for_a_raw_basis_scan() {
        assert!(rolling_full_group_static_candidate(true, 119, 119, 9));
        assert!(!rolling_full_group_static_candidate(true, 118, 119, 9));
        assert!(!rolling_full_group_static_candidate(false, 119, 119, 9));
        assert!(!rolling_full_group_static_candidate(true, 119, 119, 0));

        let group = group_source();
        let candidate = group
            .find("if static_rolling_candidate {")
            .expect("static candidate branch");
        let legacy = group[candidate..]
            .find("} else {\n        // Preserve the legacy fast path")
            .map(|offset| candidate + offset)
            .expect("legacy fast path");
        assert!(group[candidate..legacy].contains("requested_shard_cache("));
        assert!(!group[legacy..].contains("requested_shard_cache("));
        let legacy_source = &group[legacy..];
        let reference = legacy_source
            .find("let reference = &data_references[index];")
            .expect("legacy child reference");
        let cache_hit = legacy_source
            .find("cached_decoded_chunk(reference)")
            .expect("legacy decoded cache lookup");
        let queue = legacy_source
            .find("queue_initial_data_shard(index, RetrieveHedgeDemand::Ordinary)")
            .expect("legacy raw registration");
        assert!(reference < cache_hit && cache_hit < queue);

        let cache = RETRIEVAL_SOURCE
            .split("impl DecodedChunkCache {")
            .nth(1)
            .and_then(|source| source.split("fn get_decoded_and_raw(").next())
            .expect("ordinary decoded cache accessor");
        assert!(!cache.contains("get_decoded_and_raw(reference)"));
        assert!(cache.contains("let raw = decoded.is_none()"));
    }

    #[test]
    fn rolling_parity_waits_for_the_existing_gate_and_never_exceeds_width() {
        assert_eq!(
            rolling_parity_admission_count(GATE_MS - 1, GATE_MS, false, 4, 0, 4),
            0
        );
        assert_eq!(
            rolling_parity_admission_count(GATE_MS, GATE_MS, false, 4, 4, 4),
            0
        );

        let mut dispatched = vec![true, true, true, true, false, false, false];
        let mut rolling_active = 1usize;
        let mut admitted = Vec::new();
        while rolling_active < 4 {
            assert_eq!(
                rolling_parity_admission_count(
                    GATE_MS,
                    GATE_MS,
                    false,
                    4,
                    rolling_active,
                    dispatched[4..].iter().filter(|sent| !**sent).count(),
                ),
                1,
                "one pre-completed structural slot admits one replacement per turn"
            );
            let index = rolling_next_parity_index(4, &dispatched).expect("unique parity");
            assert!(!admitted.contains(&index));
            dispatched[index] = true;
            admitted.push(index);
            rolling_active += 1;
            assert!(rolling_active <= 4);
        }
        assert_eq!(admitted, vec![4, 5, 6]);
        assert_eq!(rolling_next_parity_index(4, &dispatched), None);
    }

    #[test]
    fn a_ready_terminal_result_wins_the_freed_slot_before_parity() {
        let mut requested_ready = [false];
        let rolling_active_before_result = 1usize;

        // Settle the simultaneously ready requested result first: it both frees the active slot
        // and proves terminal. The admission function must then reject the apparent free slot.
        requested_ready[0] = true;
        let rolling_active = rolling_active_before_result - 1;
        let terminal = requested_ready.iter().all(|ready| *ready);
        assert_eq!(
            rolling_parity_admission_count(GATE_MS, GATE_MS, terminal, 1, rolling_active, 1,),
            0
        );

        let group = group_source();
        let loop_start = group.find("loop {").expect("coordinator loop");
        let ready = group[loop_start..]
            .find("result_in.try_recv()")
            .map(|offset| loop_start + offset)
            .expect("ready result poll");
        let terminal_check = group[loop_start..]
            .find("let terminal =")
            .map(|offset| loop_start + offset)
            .expect("terminal check");
        let parity_admission = group[loop_start..]
            .find("rolling_parity_admission_count(")
            .map(|offset| loop_start + offset)
            .expect("parity admission");
        assert!(ready < terminal_check && terminal_check < parity_admission);
        assert!(group.contains("while let Ok(result) = result_in.try_recv()"));
        assert!(group.contains("Re-evaluate terminal state before any replacement admission."));
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
    fn an_ordinary_follower_monotonically_promotes_a_managed_flight() {
        let demand = SharedRetrieveHedgeDemand::new(RetrieveHedgeDemand::DistinctShardManaged);
        assert_eq!(demand.current(), RetrieveHedgeDemand::DistinctShardManaged);
        demand.promote(RetrieveHedgeDemand::Ordinary);
        assert_eq!(demand.current(), RetrieveHedgeDemand::Ordinary);
        demand.promote(RetrieveHedgeDemand::DistinctShardManaged);
        assert_eq!(demand.current(), RetrieveHedgeDemand::Ordinary);
    }

    #[test]
    fn rolling_and_legacy_paths_keep_their_required_boundaries() {
        let group = group_source();
        let rolling_start = group.find("if rolling {").expect("rolling branch");
        let legacy_start = group[rolling_start..]
            .find("} else if recovery_dispatched && policy.erasure_recovery {")
            .map(|offset| rolling_start + offset)
            .expect("legacy branch");
        let rolling_branch = &group[rolling_start..legacy_start];
        let legacy_branch = &group[legacy_start..];

        assert!(rolling_branch.contains("dispatch_one_rolling_group_parity("));
        assert!(!rolling_branch.contains("dispatch_group_parity("));
        assert!(!rolling_branch.contains("RETRIEVE_RS_HEDGE_AFTER_MS"));
        assert!(legacy_branch.contains("dispatch_group_parity("));
        assert!(legacy_branch.contains("RETRIEVE_RS_HEDGE_AFTER_MS"));
        assert!(group.contains("requested_count == data_count"));
        assert!(group.contains("let started = Date::now();"));

        let raw_queue = RETRIEVAL_SOURCE
            .split("fn queue_drained_raw_chunk(")
            .nth(1)
            .and_then(|source| source.split("#[inline]\nfn decryption_segment_key").next())
            .expect("raw singleflight queue");
        let promote = raw_queue
            .find("shared_demand.promote(hedge_demand)")
            .unwrap();
        let follower_return = raw_queue
            .find("return RawFetchRegistration::Joined")
            .unwrap();
        let detached = raw_queue.find("The detached producer").unwrap();
        assert!(promote < follower_return && follower_return < detached);
        assert!(raw_queue.contains("hedge_demand: registration.shared.hedge_demand.clone()"));

        let retrieve_chunk = RETRIEVAL_SOURCE
            .split("pub async fn retrieve_chunk(")
            .nth(1)
            .and_then(|source| source.split("pub async fn retrieve_check_chunk(").next())
            .expect("retrieve chunk");
        assert!(retrieve_chunk.contains("map(SharedRetrieveHedgeDemand::current)"));
        assert!(retrieve_chunk.contains("unwrap_or(RetrieveHedgeDemand::Ordinary)"));
        assert!(RETRIEVAL_SOURCE.contains("const RETRIEVE_ATTEMPT_TIMEOUT_MS: u64 = 10_000;"));
        assert!(RUNTIME_SOURCE.contains("hedge_demand: None"));
    }
}

mod progress_events {
    use crate::events::ProgressStore;

    #[test]
    fn late_updates_do_not_reopen_finished_progress() {
        let mut store = ProgressStore::new();
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
        PendingGenerationRelation, cancel_generation_is_current, generation_is_newer,
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
        assert!(!cancel_generation_is_current(Some(sought), created));
        assert!(cancel_generation_is_current(Some(sought), recreated));
    }

    #[test]
    fn wrapping_never_emits_the_reserved_zero_generation() {
        assert_eq!(next_nonzero_generation(u64::MAX), 1);
        assert!(generation_is_newer(1, u64::MAX));
        assert!(!generation_is_newer(u64::MAX, 1));
        assert!(cancel_generation_is_current(Some(u64::MAX), 1));
        assert!(!cancel_generation_is_current(Some(1), u64::MAX));
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
}

mod retrieve_admission {
    use crate::retrieval_conventions::{
        RetrieveAdmission, acquire_retrieve_permit, retrieve_admission_current,
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
        admission.close();
        admission.close();

        assert!(!admission.is_open());
        assert!(!queued.is_open());
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
    use crate::retrieval_conventions::{RetrieveAdmission, SingleflightRegistry};
    use std::{
        cell::Cell,
        collections::HashMap,
        rc::Rc,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    #[derive(Default)]
    struct AttemptLedger {
        next_attempt: u64,
        reserved: u64,
        credited: u64,
        cancelled: u64,
        settled: usize,
        pending: HashMap<u64, u64>,
    }

    impl AttemptLedger {
        fn dispatch(&mut self, price: u64) -> u64 {
            self.next_attempt += 1;
            let attempt = self.next_attempt;
            assert!(self.pending.insert(attempt, price).is_none());
            self.reserved += price;
            attempt
        }

        fn settle(&mut self, attempt: u64, success: bool) {
            let price = self
                .pending
                .remove(&attempt)
                .expect("each physical attempt settles exactly once");
            self.reserved -= price;
            if success {
                self.credited += price;
            } else {
                self.cancelled += price;
            }
            self.settled += 1;
        }
    }

    #[test]
    fn finite_admission_caps_dispatch_without_cancelling_claimed_attempt_settlement() {
        const PRICE: u64 = 17;
        let admission = RetrieveAdmission::new_with_attempt_limit(2);
        let mut ledger = AttemptLedger::default();

        assert!(admission.try_claim_physical_attempt());
        let primary = ledger.dispatch(PRICE);
        assert!(admission.try_claim_physical_attempt());
        let hedge = ledger.dispatch(PRICE);

        assert!(!admission.is_open());
        assert!(!admission.try_claim_physical_attempt());
        assert_eq!(ledger.pending.len(), 2);

        // Closing future admission does not alter either dispatched reserve. Each physical
        // attempt still reaches the same terminal accounting paths.
        ledger.settle(primary, true);
        ledger.settle(hedge, false);

        assert_eq!(ledger.reserved, 0);
        assert_eq!(ledger.credited, PRICE);
        assert_eq!(ledger.cancelled, PRICE);
        assert_eq!(ledger.settled, 2);
        assert!(ledger.pending.is_empty());
    }

    #[test]
    fn logical_timeout_releases_once_while_late_accounting_settles_once() {
        const PRICE: u64 = 17;
        let mut ledger = AttemptLedger::default();
        let attempt = ledger.dispatch(PRICE);
        let mut logical_in_flight = 1_usize;
        let mut logical_failures = 0_usize;

        // The ten-second notification is terminal only for the logical retrieval.
        logical_in_flight = logical_in_flight.saturating_sub(1);
        logical_failures += 1;
        assert_eq!(logical_in_flight, 0);
        assert_eq!(logical_failures, 1);
        assert_eq!(ledger.pending.len(), 1);
        assert_eq!(ledger.settled, 0);

        // A later authenticated response belongs exclusively to detached accounting settlement;
        // it must not re-enter or resurrect the completed logical retrieval.
        ledger.settle(attempt, true);
        assert_eq!(logical_in_flight, 0);
        assert_eq!(logical_failures, 1);
        assert_eq!(ledger.reserved, 0);
        assert_eq!(ledger.credited, PRICE);
        assert_eq!(ledger.cancelled, 0);
        assert_eq!(ledger.settled, 1);
        assert!(ledger.pending.is_empty());
    }

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
    fn detached_waiter_does_not_ghost_dispatched_accounting_or_replace_the_producer() {
        async_std::task::block_on(async {
            const PRICE: u64 = 17;
            type Flights = SingleflightRegistry<&'static str, Arc<AtomicUsize>, Arc<AtomicBool>>;

            let logical_producers = Arc::new(AtomicUsize::new(0));
            let detached_deliveries = Arc::new(AtomicUsize::new(0));
            let joined_deliveries = Arc::new(AtomicUsize::new(0));
            let flights = Arc::new(Mutex::new(Flights::default()));
            let ledger = Arc::new(Mutex::new(AttemptLedger::default()));

            let first = flights
                .lock()
                .unwrap()
                .register("chunk", detached_deliveries.clone(), {
                    let logical_producers = logical_producers.clone();
                    move || {
                        logical_producers.fetch_add(1, Ordering::SeqCst);
                        Arc::new(AtomicBool::new(true))
                    }
                });
            assert!(first.leader);
            let first_flight_id = first.flight_id;
            let first_waiter_id = first.waiter_id;

            // Both reservations represent real, dispatched transports. The second is an
            // intentional physical hedge, not another logical/raw-fetch producer.
            let primary_attempt = ledger.lock().unwrap().dispatch(PRICE);
            let hedge_attempt = ledger.lock().unwrap().dispatch(PRICE);
            let (primary_out, primary_in) = async_std::channel::bounded::<bool>(1);
            let (hedge_out, hedge_in) = async_std::channel::bounded::<bool>(1);

            let primary = async_std::task::spawn({
                let flights = flights.clone();
                let ledger = ledger.clone();
                async move {
                    let success = primary_in.recv().await.expect("primary terminal result");
                    ledger.lock().unwrap().settle(primary_attempt, success);
                    if success {
                        let completed = flights
                            .lock()
                            .unwrap()
                            .take(&"chunk", first_flight_id)
                            .expect("the dispatched producer still owns its flight");
                        for waiter in completed.waiters {
                            waiter.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            });
            let hedge = async_std::task::spawn({
                let ledger = ledger.clone();
                async move {
                    let success = hedge_in.recv().await.expect("hedge terminal result");
                    ledger.lock().unwrap().settle(hedge_attempt, success);
                }
            });

            async_std::task::yield_now().await;
            let shared = flights
                .lock()
                .unwrap()
                .remove_waiter(&"chunk", first_flight_id, first_waiter_id)
                .expect("the last browser waiter releases admission only");
            shared.store(false, Ordering::SeqCst);

            let joined = flights
                .lock()
                .unwrap()
                .register("chunk", joined_deliveries.clone(), {
                    let logical_producers = logical_producers.clone();
                    move || {
                        logical_producers.fetch_add(1, Ordering::SeqCst);
                        Arc::new(AtomicBool::new(true))
                    }
                });
            assert!(!joined.leader);
            assert_eq!(joined.flight_id, first_flight_id);
            assert!(!joined.shared.load(Ordering::SeqCst));
            assert_eq!(logical_producers.load(Ordering::SeqCst), 1);

            // A response arriving after listener detachment still credits the dispatch.
            primary_out.send(true).await.unwrap();
            primary.await;
            // Completion of the logical read cannot cancel an already-dispatched hedge.
            hedge_out.send(false).await.unwrap();
            hedge.await;

            let ledger = ledger.lock().unwrap();
            assert_eq!(ledger.reserved, 0);
            assert_eq!(ledger.credited, PRICE);
            assert_eq!(ledger.cancelled, PRICE);
            assert_eq!(ledger.settled, 2);
            assert!(ledger.pending.is_empty());
            assert_eq!(detached_deliveries.load(Ordering::SeqCst), 0);
            assert_eq!(joined_deliveries.load(Ordering::SeqCst), 1);
            assert_eq!(logical_producers.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn completion_detaches_every_waiter_atomically() {
        let mut flights = SingleflightRegistry::<u8, usize, ()>::default();
        let first = flights.register(9, 4, || ());
        flights.register(9, 2, || ());

        let mut waiters = flights
            .take(&9, first.flight_id)
            .expect("active flight")
            .waiters;
        waiters.sort_unstable();
        assert_eq!(waiters, vec![2, 4]);
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
        assert_eq!(old_flight.waiters, Vec::<&'static str>::new());

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
        assert_eq!(flight.waiters, vec!["new"]);
    }
}
