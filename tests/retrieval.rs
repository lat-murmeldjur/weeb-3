#![allow(dead_code)]

#[path = "../src/accounting.rs"]
mod accounting;
#[path = "../src/events.rs"]
mod events;
#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;

mod connection {
    use crate::accounting::{
        CONNECTION_BUILDUP_LIMIT, CONNECTION_DIAL_CONCURRENCY_LIMIT, bee_reconnect_delay_seconds,
        connection_dial_capacity_available,
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
        assert!(!connection_dial_capacity_available(
            1,
            CONNECTION_DIAL_CONCURRENCY_LIMIT
        ));
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
    fn dial_storm_is_bounded_independently_of_the_peer_population() {
        assert_eq!(CONNECTION_DIAL_CONCURRENCY_LIMIT, 160);
        assert!(connection_dial_capacity_available(
            0,
            CONNECTION_DIAL_CONCURRENCY_LIMIT - 1
        ));
        assert!(!connection_dial_capacity_available(
            0,
            CONNECTION_DIAL_CONCURRENCY_LIMIT
        ));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT - CONNECTION_DIAL_CONCURRENCY_LIMIT,
            CONNECTION_DIAL_CONCURRENCY_LIMIT
        ));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT,
            0
        ));
        assert!(!connection_dial_capacity_available(u64::MAX, u64::MAX));

        let runtime = include_str!("../src/lib.rs");
        let dial_feeder = runtime
            .split("let swarm_event_handle_0 = async")
            .nth(1)
            .and_then(|source| source.split("let swarm_event_handle_1 = async").next())
            .expect("peer dial feeder should remain inspectable");
        assert!(
            dial_feeder.contains("spawn_local(queue_peer_dial_retry("),
            "an immediate dial error must schedule its retry without sleeping the only dial feeder"
        );
        assert!(dial_feeder.contains("start_owned_connection_attempt("));
        assert!(dial_feeder.contains("try_mark_connection_attempt(&wings, &candidate.peer)"));
        assert!(dial_feeder.contains("let mut new_peers = VecDeque::<QueuedPeerDial>::new()"));
        assert!(dial_feeder.contains("let mut retries = VecDeque::<QueuedPeerDial>::new()"));
        assert!(dial_feeder.contains("let mut queued_underlays = HashSet::<Vec<u8>>::new()"));
        assert!(dial_feeder.contains("for _ in 0..PEER_DIAL_INGEST_BATCH"));
        assert!(
            dial_feeder
                .matches("current_connection_generation().await")
                .count()
                >= 2
        );
        assert!(dial_feeder.contains("queued_underlays.len() >= MAX_QUEUED_PEER_DIALS"));
        assert!(dial_feeder.contains("let underlay = candidate.dial_addr.to_vec()"));
        let ingestion_end = dial_feeder.find("let mut unavailable_peers").unwrap();
        assert!(!dial_feeder[..ingestion_end].contains("peer_already_connected_or_attempting"));
        assert!(dial_feeder.contains("let mut take_eligible ="));
        assert!(dial_feeder.contains("let retry_due ="));
        assert!(dial_feeder.contains("fresh_dials_since_retry >= FRESH_PEER_DIALS_PER_RETRY"));
        assert!(
            dial_feeder
                .contains("take_eligible(&mut new_peers).or_else(|| take_eligible(&mut retries))")
        );
        assert!(dial_feeder.contains("if candidate.peer == local_peer_id"));
        assert!(!dial_feeder.contains("prefer_new_peer"));
        assert!(dial_feeder.contains("candidate.retry = candidate.retry"));
        assert!(dial_feeder.contains("known_peer_underlays"));
        assert!(dial_feeder.contains("underlay == &candidate.dial_addr"));
        assert!(dial_feeder.contains("candidate.bootnode,"));
        assert!(include_str!("../src/handlers.rs").contains("underlay: peer.underlay,"));
        assert!(include_str!("../src/handlers.rs").contains(".send(PeerDialInstruction {"));
        assert!(runtime.contains("peers_instructions\n        .send(PeerDialInstruction {"));
        assert!(runtime.contains("mpsc::bounded::<PeerDialInstruction>(MAX_QUEUED_PEER_DIALS)"));
        assert!(runtime.contains("const FRESH_PEER_DIALS_PER_RETRY: usize = 32;"));

        let dial_attempt = runtime
            .split_once("async fn start_owned_connection_attempt(")
            .expect("owned connection attempt helper")
            .1;
        assert!(dial_attempt.contains("PeerCondition::DisconnectedAndNotDialing"));
        assert!(dial_attempt.contains("swarm.dial(options)?;"));

        let swarm_events = runtime
            .split_once("let swarm_event_handle_1 = async")
            .expect("swarm event loop")
            .1;
        assert!(runtime.contains("const CONNECTION_BUILDUP_SWARM_POLL_MS: u64 = 5;"));
        assert!(runtime.contains("const STARTUP_QUEUE_POLL_MS: u64 = 15;"));
        assert!(swarm_events.contains("let connection_building = ongoing > 0;"));
        assert!(swarm_events.contains("if connection_building"));
        assert!(swarm_events.contains("swarm.next().now_or_never().flatten()"));
        assert!(swarm_events.contains("Duration::from_millis(swarm_poll_ms)"));

        let reserve = runtime
            .split("async fn try_reserve_connection_capacity(")
            .nth(1)
            .and_then(|source| source.split("async fn queue_peer_dial_retry(").next())
            .expect("atomic connection reservation helper");
        let connected_guard = reserve
            .find("let connected_guard = connections.lock().await;")
            .expect("connected population guard");
        let ongoing_guard = reserve
            .find("let mut ongoing = ongoing_connections.lock().await;")
            .expect("ongoing population guard");
        let increment = reserve
            .find("*ongoing = (*ongoing).saturating_add(1);")
            .expect("ongoing reservation increment");
        assert!(
            connected_guard < ongoing_guard && ongoing_guard < increment,
            "capacity check and reservation must hold both counters in the promotion lock order"
        );

        let promotion = runtime
            .split("async fn promote_priced_peer(")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub async fn post_upload_with_redundancy(")
                    .next()
            })
            .expect("priced-peer promotion");
        let connections = promotion
            .find("let mut connections = self.connections.lock().await;")
            .expect("connected counter lock");
        let ongoing = promotion
            .find("let mut ongoing = self.ongoing_connections.lock().await;")
            .expect("ongoing counter lock");
        let release = promotion
            .find("*ongoing = ongoing.saturating_sub(1);")
            .expect("ongoing reservation release");
        let promote = promotion
            .find("*connections = connections.saturating_add(1);")
            .expect("connected counter promotion");
        assert!(connections < ongoing && ongoing < release && release < promote);
    }

    #[test]
    fn mainnet_startup_samples_the_complete_dns_bootnode_database() {
        use std::collections::HashSet;

        let profile_source = include_str!("../src/network_profile.rs");
        let mainnet = profile_source
            .split_once("pub(crate) const MAINNET_BOOTNODES: &[&str] = &[")
            .and_then(|(_, source)| source.split_once("pub(crate) const TESTNET_PROFILE"))
            .map(|(source, _)| source)
            .expect("mainnet bootnode constant");
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

        let mut peers = HashSet::new();
        for address in addresses {
            assert!(address.is_ascii());
            let parts = address.split('/').collect::<Vec<_>>();
            assert_eq!(parts.len(), 9, "invalid browser underlay: {address}");
            assert_eq!(parts[0], "");
            assert_eq!(parts[1], "dns4");
            assert!(parts[2].ends_with(".libp2p.direct"));
            assert_eq!(parts[3], "tcp");
            assert!(parts[4].parse::<u16>().is_ok());
            assert_eq!(&parts[5..8], &["tls", "ws", "p2p"]);
            assert!(parts[8].starts_with("Qm"));
            assert!(peers.insert(parts[8]), "duplicate peer: {}", parts[8]);
        }
        assert_eq!(peers.len(), 319);

        assert!(profile_source.contains("bootnodes.shuffle(&mut rand::thread_rng())"));
        assert!(!profile_source.contains("bootnodes.truncate("));

        let interface = include_str!("../src/interface_runtime_conventions.rs");
        assert!(interface.contains("for address in randomized_bootnodes(profile)"));
        assert!(interface.contains(".take(CONNECTION_DIAL_CONCURRENCY_LIMIT as usize)"));
        assert!(!interface.contains("profile.bootnodes.contains(&address.as_str())"));
        assert!(interface.contains("BOOTNODE_INPUT_IDS.iter().enumerate()"));
        assert!(interface.contains("profile.bootnodes.get(index)"));
        assert!(!interface.contains("backfill"));

        let library = include_str!("../src/library.rs");
        assert_eq!(library.matches("randomized_bootnodes(profile)").count(), 3);
        assert!(library.contains("for address in profile.bootnodes"));
        let scheduler = library
            .split_once("fn schedule_bootnode_dials(")
            .and_then(|(_, source)| source.split_once("#[wasm_bindgen]"))
            .map(|(source, _)| source)
            .expect("bootnode scheduler");
        assert!(scheduler.contains("crate::join_all(dials).await"));
        assert!(!scheduler.contains("backfill"));
        assert_eq!(
            library
                .matches(".take(CONNECTION_DIAL_CONCURRENCY_LIMIT as usize)")
                .count(),
            3
        );

        let runtime = include_str!("../src/lib.rs");
        assert!(runtime.contains("const MAX_QUEUED_PEER_DIALS: usize = 4_096;"));
        assert!(!runtime.contains("prefer_new_peer"));
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
    fn an_exact_failed_dial_releases_capacity_even_if_another_connection_exists() {
        let runtime = include_str!("../src/lib.rs");
        let outgoing_error = runtime
            .split("SwarmEvent::OutgoingConnectionError {")
            .nth(1)
            .and_then(|source| source.split("SwarmEvent::ConnectionClosed {").next())
            .expect("outgoing-error lifecycle");
        let exact_release = outgoing_error
            .find("let had_attempt = remove_connection_attempt_for_dial(")
            .expect("exact dial-attempt release");
        let decrement = outgoing_error[exact_release..]
            .find("decrement_counter(&ongoing_connections).await;")
            .map(|offset| exact_release + offset)
            .expect("ongoing-capacity release");
        let broad_connection_check = outgoing_error
            .find("let physically_connected =")
            .expect("broad peer connection check");
        assert!(
            exact_release < decrement && decrement < broad_connection_check,
            "another physical connection must not hide an exact failed dial or leak its capacity"
        );
        assert!(outgoing_error.contains("if physically_connected && !had_attempt"));
        assert!(outgoing_error.contains("let retryable = !matches!("));
        assert!(outgoing_error.contains("DialError::LocalPeerId { .. }"));
        assert!(outgoing_error.contains("DialError::WrongPeerId { .. }"));
        assert!(outgoing_error.contains("should_retry = retryable"));
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

        let admission = runtime
            .split("async fn peer_already_connected_or_attempting(")
            .nth(1)
            .and_then(|source| source.split("async fn try_mark_connection_attempt(").next())
            .expect("queued dial admission");
        assert!(admission.contains("connection_cooldowns.contains(peer)"));
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
            .find("let (balance, last_refreshment) =")
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
        assert!(instruction.contains("if account.balance < REFRESH_RATE"));
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
            .find("account.balance >= REFRESH_RATE && !account.refresh_scheduled")
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
        assert!(
            group.contains("emitter.emit(result_index, chunk.clone())"),
            "healthy requested shards must be emitted directly from the receive loop"
        );
        assert!(
            group.contains("emitter.emit(index, chunk.clone())"),
            "reconstructed requested shards must be emitted before terminal success"
        );
        assert!(
            group.contains("if requested_count == data_count")
                && group.contains("dispatch_group_parity(")
                && group.contains("usize::MAX"),
            "a full-group hedge must race all Bee parity without widening subset-of-child groups"
        );

        let traversal = source_section(
            "async fn retrieve_data_range_from_root_with_prefix_cancellable(",
            "pub(crate) async fn retrieve_data_range_join(",
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
            "pub(crate) async fn retrieve_data_range_join(",
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
            Arc,
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
