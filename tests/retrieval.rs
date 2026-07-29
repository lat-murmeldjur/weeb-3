#![allow(dead_code)]

#[path = "../src/connection_conventions.rs"]
mod connection_conventions;
#[path = "../src/events.rs"]
mod events;
#[path = "../src/retrieval_conventions.rs"]
mod retrieval_conventions;

mod connection {
    use crate::connection_conventions;

    use std::collections::HashMap;

    use connection_conventions::{
        CONNECTION_BUILDUP_LIMIT, CONNECTION_DIAL_CONCURRENCY_LIMIT, ConnectionCounterRelease,
        bee_reconnect_delay_seconds, connection_counter_release,
        connection_dial_capacity_available, remove_overlay_owner, retrieval_dispatch_available,
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
        assert!(!retrieval_dispatch_available(0));
        assert!(retrieval_dispatch_available(1));
        assert!(!connection_dial_capacity_available(
            1,
            CONNECTION_DIAL_CONCURRENCY_LIMIT
        ));
        assert!(retrieval_dispatch_available(1));
        assert!(!connection_dial_capacity_available(
            CONNECTION_BUILDUP_LIMIT,
            0
        ));
        assert!(retrieval_dispatch_available(1));
        assert!(retrieval_dispatch_available(
            CONNECTION_BUILDUP_LIMIT as usize
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
        assert!(selection.contains("retrieval_dispatch_available(peers_map.len())"));
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
        assert_eq!(CONNECTION_DIAL_CONCURRENCY_LIMIT, 128);
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
        assert!(dial_feeder.contains("PeerCondition::DisconnectedAndNotDialing"));
        assert!(dial_feeder.contains("try_mark_connection_attempt(&wings, &candidate.peer)"));
        assert!(
            dial_feeder.contains(
                ".try_send((candidate.bzzaddr, false, candidate.generation, attempt_id))"
            )
        );

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
            .and_then(|source| source.split("pub async fn post_upload(").next())
            .expect("priced-peer promotion");
        let transfer = promotion
            .split("Transfer the counted peer from ongoing to connected")
            .nth(1)
            .expect("atomic promotion transfer");
        assert!(transfer.contains("let mut connections = self.connections.lock().await;"));
        assert!(transfer.contains("let mut ongoing = self.ongoing_connections.lock().await;"));
        assert!(transfer.contains("*ongoing = ongoing.saturating_sub(1);"));
        assert!(transfer.contains("*connections = connections.saturating_add(1);"));
    }

    #[test]
    fn only_the_peer_that_owns_an_overlay_can_remove_it() {
        let mut overlays = HashMap::from([("overlay", "original-peer")]);
        assert!(!remove_overlay_owner(
            &mut overlays,
            &"overlay",
            &"foreign-peer"
        ));
        assert_eq!(overlays.get("overlay"), Some(&"original-peer"));
        assert!(remove_overlay_owner(
            &mut overlays,
            &"overlay",
            &"original-peer"
        ));
        assert!(!overlays.contains_key("overlay"));
    }

    #[test]
    fn peer_lifecycle_releases_exactly_one_counter() {
        use ConnectionCounterRelease::{Connected, None, Ongoing};

        assert_eq!(connection_counter_release(true, false, false), Ongoing);
        assert_eq!(connection_counter_release(true, true, false), Ongoing);
        assert_eq!(connection_counter_release(true, false, true), Ongoing);
        assert_eq!(connection_counter_release(false, true, false), Connected);
        assert_eq!(connection_counter_release(false, false, true), Connected);
        assert_eq!(connection_counter_release(false, false, false), None);
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
            .and_then(|source| source.split("pub async fn post_upload(").next())
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
            .split("// `connected_peers` is the peer-lifecycle guard.")
            .nth(1)
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
            .split("// `connected_peers` is the peer-lifecycle guard.")
            .nth(1)
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
            .find("*connection_generation.lock().await != retry_generation")
            .expect("post-backoff generation check");
        let cooldown_release = close[sleep..]
            .find(".remove(&peer_id)")
            .expect("same-generation cooldown release");
        let enqueue = close[sleep..]
            .find(".try_send((bzzaddr, retry_generation))")
            .expect("reconnect enqueue");
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
            .and_then(|source| source.split("#[wasm_bindgen]").next())
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
            .find("chan.try_send(instruction)")
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
        assert!(runtime.contains("HANDSHAKE_PROTOCOL_TIMEOUT_MS"));
        assert!(runtime.contains("Handshake timed out peer="));

        let handlers = include_str!("../src/handlers.rs");
        assert!(handlers.contains("let Some(ack) = rec_0.ack.as_ref()"));
        assert!(handlers.contains("let Some(peer_address) = ack.address.as_ref()"));
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
            .and_then(|source| source.split("#[allow(dead_code)]").next())
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
        assert!(runtime.contains("Ignored stale cheque result"));

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
        let connection_conventions = include_str!("../src/connection_conventions.rs");
        assert!(connection_conventions.contains("Poll::Ready(ToSwarm::Dial { opts })"));
        assert!(connection_conventions.contains("FromSwarm::DialFailure(DialFailure"));
        assert!(connection_conventions.contains("let error = DialError::NoAddresses;"));

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
