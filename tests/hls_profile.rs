#![recursion_limit = "256"]

use anyhow_crates_io::{Context, Result, anyhow};
use headless_chrome::protocol::cdp::Network::{ClearBrowserCache, SetCacheDisabled};
use headless_chrome::protocol::cdp::Page::AddScriptToEvaluateOnNewDocument;
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde::Serialize;
use serde_json_crates_io::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_SERVICE_WORKER_SCOPE: &str = "/weeb-3/";
const SERVICE_WORKER_PROTOCOL: u64 = 8;

const HLS_PROFILE_SCRIPT: &str = r#"
(() => {
    window.__weeb3HlsRetrieveProfileEnabled = true;
    performance.setResourceTimingBufferSize?.(8192);
    const RANGE_PROGRESS_TRACE_CAP = 256;
    const RAW_STARTUP_TRACE_CAP = 2048;
    const RAW_STARTUP_TRACE_DATA_CAP = RAW_STARTUP_TRACE_CAP - 1;
    const RAW_STARTUP_PROFILE_EVENT = 'weeb3-hls-raw-startup-profile';
    const HLS_STORAGE_WINDOW_BYTES = 512 * 1024;
    const profile = window.__weeb3HlsProfile = {
        marks: { navigation: 0 }, events: [], samples: [], errors: [],
        first_presented_frame: null,
        measurement_frozen_at_ms: null,
        performance_time_origin_ms: performance.timeOrigin,
        source_wasm_build_id: null,
        sampling: {
            pre_frame_interval_ms: 50,
            steady_interval_ms: 500
        },
        retrieval_rolling_group_trace: null,
        retrieval_rolling_group_trace_capture: {
            attempted: false,
            call_count: 0,
            getter_present: null,
            reason: null,
            at_ms: null,
            performance_time_origin_ms: performance.timeOrigin
        },
        raw_startup_trace: {
            schema_version: 3,
            event_name: RAW_STARTUP_PROFILE_EVENT,
            cap: RAW_STARTUP_TRACE_CAP,
            data_cap: RAW_STARTUP_TRACE_DATA_CAP,
            events: [],
            dropped: 0,
            collector_terminal_reason: null,
            emitter_terminal_reason: null,
            admission_close: null,
            terminal: null,
            latest: null,
            semantics: {
                raw_leader_dispatches:
                    'accepted RawFetch leader ChunkRetrieveRequest enqueues, not Bee peer attempts',
                raw_leader_completions:
                    'terminal RawFetch singleflight producer completions',
                credits_minted:
                    'canonical W0 seed leader completions that published one conserved credit',
                credits_available: 'published credits not currently held by a scout child',
                credits_held: 'credits acquired by the coordinator and not yet returned',
                credits_discarded: 'credits consumed or returned after scout admission closed',
                scout_active: 'held credits currently owned by newly-led scout RawFetch flights',
                raw_flight_id:
                    'lossless decimal RawFetch singleflight ID; null for Cached/control rows',
                bee_peer_attempts: 'unobservable at the RawFetch layer; always null',
                retrieval_permits: 'unobservable at the RawFetch layer; always null'
            }
        },
        service_worker_trace: {
            controller_changes: [],
            hls_requests: [],
            response_capture_mode: 'one-shot-own-port-postMessage-wrapper',
            range_progress: {
                cap: RANGE_PROGRESS_TRACE_CAP,
                first_reference: null,
                first_playing_at_ms: null,
                terminal_windows: Array.from({ length: 5 }, (_, index) => ({
                    horizon: `W${index}`,
                    range: `bytes=${index * HLS_STORAGE_WINDOW_BYTES}-${
                        (index + 1) * HLS_STORAGE_WINDOW_BYTES - 1}`,
                    reference: null,
                    state: null,
                    at_ms: null,
                    text: null
                })),
                transitions: [],
                observer: {
                    state: 'discovering',
                    installed_at_ms: performance.now(),
                    attached_at_ms: null,
                    disconnected_at_ms: null,
                    disconnect_reason: null
                }
            }
        },
        cold_start: {
            observed_at_ms: performance.now(),
            controller_present_at_document_start:
                navigator.serviceWorker ? Boolean(navigator.serviceWorker.controller) : null,
            registrations: { status: 'pending', count: null, scripts: [] },
            cache_storage: { status: 'pending', count: null, names: [] },
            http_cache_cleared_before_navigation: true,
            http_cache_disabled_before_navigation: true
        }
    };
    const rawStartupTrace = profile.raw_startup_trace;
    let retrievalProfileGetterErrorRecorded = false;
    const retrievalProfileSnapshot = () => {
        const getter = window.__weeb3GetHlsRetrieveProfileSnapshot;
        if (typeof getter !== 'function') return null;
        try { return getter(); }
        catch (error) {
            if (!retrievalProfileGetterErrorRecorded) {
                retrievalProfileGetterErrorRecorded = true;
                profile.errors.push({
                    at_ms: performance.now(),
                    type: 'retrieval-profile-getter',
                    message: String(error?.message || error)
                });
            }
            return null;
        }
    };
    const finalizeRollingGroupTrace = (
        reason, at = performance.now()
    ) => {
        const capture = profile.retrieval_rolling_group_trace_capture;
        if (capture.attempted) return profile.retrieval_rolling_group_trace;
        capture.attempted = true;
        capture.reason = reason;
        capture.at_ms = at;
        const finalizer = window.__weeb3FinalizeHlsRetrieveRollingGroupTrace;
        capture.getter_present = typeof finalizer === 'function';
        if (!capture.getter_present) {
            profile.errors.push({
                at_ms: at,
                type: 'retrieval-rolling-group-finalizer-missing',
                message: 'rolling-group finalizer was not installed'
            });
            return null;
        }
        capture.call_count++;
        try {
            profile.retrieval_rolling_group_trace = finalizer();
        } catch (error) {
            profile.errors.push({
                at_ms: at,
                type: 'retrieval-rolling-group-finalizer',
                message: String(error?.message || error)
            });
        }
        return profile.retrieval_rolling_group_trace;
    };
    const captureSourceWasmBuildId = () => {
        if (profile.source_wasm_build_id !== null ||
            profile.measurement_frozen_at_ms !== null) return;
        for (const entry of document.querySelectorAll('#logsField > *')) {
            const match = /\bInterface mounted, version ([0-9a-f]{16})\b/
                .exec(entry.textContent || '');
            if (match) {
                profile.source_wasm_build_id = match[1];
                return;
            }
        }
    };
    const rawStartupCounterFields = [
        'raw_leaders_led', 'raw_leader_dispatches', 'raw_leader_completions',
        'raw_leaders_active', 'logical_retrieve_dispatches', 'credits_minted',
        'credits_available', 'credits_held', 'credits_discarded', 'scout_active'
    ];
    addEventListener(RAW_STARTUP_PROFILE_EVENT, event => {
        if (profile.measurement_frozen_at_ms !== null) return;
        const detail = event instanceof CustomEvent ? event.detail : null;
        if (!detail || detail.schema_version !== 3 || detail.layer !== 'raw-singleflight')
            return;
        const eventName = String(detail.event);
        if (!['registration', 'completion', 'admission-close', 'trace-terminal']
            .includes(eventName)) return;
        const terminalReason = detail.terminal_reason === null
            ? null : String(detail.terminal_reason);
        if (eventName === 'trace-terminal') {
            if (!['admission-closed', 'cap-reached', 'dispatch-failed']
                .includes(terminalReason)) return;
        } else if (terminalReason !== null) {
            return;
        }
        const horizonIndex = Number(detail.horizon_index);
        if (!Number.isSafeInteger(horizonIndex) || horizonIndex < 0) return;
        const groupNumberFields = [
            'group_id', 'group_horizon_index', 'group_depth',
            'requested_first_index', 'requested_last_index', 'requested_count',
            'data_count', 'parity_count', 'decoded_raw_count', 'decoded_only_count',
            'cache_miss_count', 'child_index'
        ];
        const groupNumbers = Object.fromEntries(groupNumberFields.map(field => {
            if (detail[field] === null) return [field, null];
            const value = Number(detail[field]);
            return [field, Number.isSafeInteger(value) && value >= 0 ? value : undefined];
        }));
        if (Object.values(groupNumbers).some(value => value === undefined)) return;
        const groupStringFields = [
            'group_parent_start', 'group_parent_span', 'child_start', 'child_span'
        ];
        const groupStrings = Object.fromEntries(groupStringFields.map(field => {
            if (detail[field] === null) return [field, null];
            const value = detail[field];
            return [field, typeof value === 'string' && /^(0|[1-9]\d*)$/.test(value)
                ? value : undefined];
        }));
        if (Object.values(groupStrings).some(value => value === undefined)) return;
        const fullDataGroupCandidate = detail.full_data_group_candidate === null
            ? null : (typeof detail.full_data_group_candidate === 'boolean'
                ? detail.full_data_group_candidate : undefined);
        const fullDataGroupEligible = detail.full_data_group_eligible === null
            ? null : (typeof detail.full_data_group_eligible === 'boolean'
                ? detail.full_data_group_eligible : undefined);
        if (fullDataGroupCandidate === undefined || fullDataGroupEligible === undefined) return;
        const groupMetadata = {
            ...groupNumbers,
            ...groupStrings,
            full_data_group_candidate: fullDataGroupCandidate,
            full_data_group_eligible: fullDataGroupEligible
        };
        const hasGroup = groupNumbers.group_id !== null;
        const groupValues = Object.values(groupMetadata);
        if (hasGroup ? groupValues.some(value => value === null)
            : groupValues.some(value => value !== null)) return;
        if (hasGroup) {
            const expectedCandidate = groupNumbers.requested_first_index === 0 &&
                groupNumbers.requested_count === groupNumbers.data_count &&
                groupNumbers.parity_count > 0;
            const expectedEligible = expectedCandidate && groupNumbers.cache_miss_count > 0 &&
                groupNumbers.decoded_only_count < groupNumbers.parity_count;
            if (groupNumbers.group_id === 0 ||
                groupNumbers.group_horizon_index !== horizonIndex || horizonIndex === 0 ||
                groupNumbers.requested_count === 0 || groupNumbers.data_count === 0 ||
                groupNumbers.requested_first_index > groupNumbers.requested_last_index ||
                groupNumbers.requested_last_index >= groupNumbers.data_count ||
                groupNumbers.requested_count !== groupNumbers.requested_last_index -
                    groupNumbers.requested_first_index + 1 ||
                groupNumbers.decoded_raw_count + groupNumbers.decoded_only_count +
                    groupNumbers.cache_miss_count !== groupNumbers.requested_count ||
                groupNumbers.child_index < groupNumbers.requested_first_index ||
                groupNumbers.child_index > groupNumbers.requested_last_index ||
                fullDataGroupCandidate !== expectedCandidate ||
                fullDataGroupEligible !== expectedEligible) return;
        }
        const registration = detail.registration === null
            ? null : String(detail.registration);
        const rawFlightId = detail.raw_flight_id === null
            ? null : (typeof detail.raw_flight_id === 'string' &&
                /^(0|[1-9]\d*)$/.test(detail.raw_flight_id) &&
                BigInt(detail.raw_flight_id) > 0n &&
                BigInt(detail.raw_flight_id) <= 18446744073709551615n
                ? detail.raw_flight_id : undefined);
        if (rawFlightId === undefined) return;
        if (eventName === 'registration') {
            if (!['Cached', 'Joined', 'Led'].includes(registration) ||
                (registration === 'Cached') !== (rawFlightId === null)) return;
        } else if (eventName === 'completion') {
            if (registration !== 'Led' || rawFlightId === null) return;
        } else if (registration !== null || rawFlightId !== null) return;
        if (['registration', 'completion'].includes(eventName)) {
            if ((horizonIndex === 0) !== !hasGroup) return;
        } else if (hasGroup) return;
        const counters = Object.fromEntries(rawStartupCounterFields.map(field => {
            const value = Number(detail[field]);
            return [field, Number.isSafeInteger(value) && value >= 0 ? value : null];
        }));
        if (Object.values(counters).some(value => value === null)) return;
        if (eventName === 'trace-terminal') {
            if (rawStartupTrace.events.length >= RAW_STARTUP_TRACE_CAP) {
                rawStartupTrace.dropped++;
                rawStartupTrace.collector_terminal_reason = 'cap-reached';
                return;
            }
        } else if (rawStartupTrace.events.length >= RAW_STARTUP_TRACE_DATA_CAP) {
            rawStartupTrace.dropped++;
            rawStartupTrace.collector_terminal_reason = 'cap-reached';
            return;
        }
        const row = {
            at_ms: performance.now(),
            schema_version: 3,
            layer: 'raw-singleflight',
            event: eventName,
            horizon_index: horizonIndex,
            horizon: `W${horizonIndex}`,
            registration,
            raw_flight_id: rawFlightId,
            dispatch_accepted: typeof detail.dispatch_accepted === 'boolean'
                ? detail.dispatch_accepted : null,
            canonical_cac: typeof detail.canonical_cac === 'boolean'
                ? detail.canonical_cac : null,
            terminal_reason: terminalReason,
            admission_open: typeof detail.admission_open === 'boolean'
                ? detail.admission_open : null,
            ...groupMetadata,
            ...counters,
            bee_peer_attempts: null,
            retrieval_permits: null,
            priced_peers: Number(document.getElementById('connections')?.textContent) || 0,
            ongoing_connections: Number(document.getElementById('ongoing')?.textContent) || 0
        };
        rawStartupTrace.events.push(row);
        rawStartupTrace.latest = row;
        if (eventName === 'admission-close') rawStartupTrace.admission_close = row;
        if (eventName === 'trace-terminal') rawStartupTrace.terminal = row;
    }, true);
    window.__weeb3HlsRawStartupProfileEnabled = true;
    const coldStartProbes = [];
    if (!navigator.serviceWorker?.getRegistrations) {
        profile.cold_start.registrations.status = 'unavailable';
    } else {
        try {
            const registrations = navigator.serviceWorker.getRegistrations();
            coldStartProbes.push(registrations.then(value => {
                profile.cold_start.registrations = {
                    status: 'resolved',
                    count: value.length,
                    scripts: value.map(registration =>
                        registration.active?.scriptURL ||
                        registration.waiting?.scriptURL ||
                        registration.installing?.scriptURL || null)
                };
            }, error => {
                profile.cold_start.registrations = {
                    status: 'rejected', count: null, scripts: [],
                    error: String(error?.message || error)
                };
            }));
        } catch (error) {
            profile.cold_start.registrations = {
                status: 'threw', count: null, scripts: [],
                error: String(error?.message || error)
            };
        }
    }
    if (!globalThis.caches?.keys) {
        profile.cold_start.cache_storage.status = 'unavailable';
    } else {
        try {
            const cacheNames = globalThis.caches.keys();
            coldStartProbes.push(cacheNames.then(value => {
                profile.cold_start.cache_storage = {
                    status: 'resolved', count: value.length, names: value
                };
            }, error => {
                profile.cold_start.cache_storage = {
                    status: 'rejected', count: null, names: [],
                    error: String(error?.message || error)
                };
            }));
        } catch (error) {
            profile.cold_start.cache_storage = {
                status: 'threw', count: null, names: [],
                error: String(error?.message || error)
            };
        }
    }
    window.__weeb3ColdStartReady = Promise.allSettled(coldStartProbes);
    const mark = (name, at = performance.now()) => {
        if (profile.measurement_frozen_at_ms !== null)
            return profile.marks[name];
        return profile.marks[name] ??= at;
    };
    const recordAttributionError = (type, error, requestId = null) => {
        if (profile.measurement_frozen_at_ms !== null) return;
        try {
            profile.errors.push({
                at_ms: performance.now(), type,
                request_id: requestId,
                message: String(error?.message || error)
            });
        } catch (_) {}
    };
    const hlsRequest = data => {
        if (data?.type !== 'WEEB3_FETCH_REQUEST') return false;
        const range = typeof data.range === 'string' ? data.range : '';
        if (range && !/^bytes=[0-9]+-[0-9]+$/.test(range)) return false;
        try {
            return /\/hls\/bytes\/[a-fA-F0-9]{64,128}$/.test(
                new URL(String(data.url), location.href).pathname);
        } catch (_) {
            return false;
        }
    };
    const responseHeaderFields = rows => {
        const names = [
            'Content-Length',
            'Content-Range',
            'X-Weeb3-Stream-Start',
            'X-Weeb3-Stream-Token',
            'X-Weeb3-HLS-Critical-Prefix-Windows'
        ];
        const values = new Map();
        if (Array.isArray(rows)) {
            for (const row of rows) {
                if (Array.isArray(row) && row.length >= 2)
                    values.set(String(row[0]).toLowerCase(), String(row[1]));
            }
        }
        return Object.fromEntries(names.map(name => [
            name, values.get(name.toLowerCase()) ?? null
        ]));
    };
    const captureHlsRangeResponse = (port, entry) => {
        if (!port || typeof port.postMessage !== 'function') {
            entry.response_capture = 'request-only-no-port';
            return;
        }
        const hadOwnPostMessage = Object.prototype.hasOwnProperty.call(port, 'postMessage');
        const ownPostMessage = hadOwnPostMessage
            ? Object.getOwnPropertyDescriptor(port, 'postMessage') : null;
        const originalPostMessage = port.postMessage;
        let armed = true;
        const restore = () => {
            if (!armed) return;
            armed = false;
            if (hadOwnPostMessage && ownPostMessage)
                Object.defineProperty(port, 'postMessage', ownPostMessage);
            else
                delete port.postMessage;
        };
        const forwardingPostMessage = function(...args) {
            restore();
            const forwarded = Reflect.apply(originalPostMessage, this, args);
            if (profile.measurement_frozen_at_ms !== null) return forwarded;
            const at = performance.now();
            const response = args[0];
            try {
                entry.response_capture = 'captured';
                entry.response = {
                    at_ms: at,
                    elapsed_ms: at - entry.request_at_ms,
                    ok: typeof response?.ok === 'boolean' ? response.ok : null,
                    status: Number.isFinite(response?.status) ? response.status : null,
                    stream: typeof response?.stream === 'boolean' ? response.stream : null,
                    error: typeof response?.error === 'string' && response.error
                        ? response.error : null,
                    header_fields: responseHeaderFields(response?.headers)
                };
            } catch (error) {
                entry.response_capture = 'request-only-capture-error';
                recordAttributionError('service-worker-range-response', error, entry.id);
            }
            return forwarded;
        };
        try {
            Object.defineProperty(port, 'postMessage', {
                configurable: true,
                writable: true,
                value: forwardingPostMessage
            });
            entry.response_capture = 'armed';
        } catch (error) {
            entry.response_capture = 'request-only-wrapper-rejected';
            recordAttributionError('service-worker-range-port-wrapper', error, entry.id);
        }
    };
    if (navigator.serviceWorker) {
        navigator.serviceWorker.addEventListener('controllerchange', () => {
            if (profile.measurement_frozen_at_ms !== null) return;
            const at = performance.now();
            const controller = navigator.serviceWorker.controller;
            mark('service_worker_controllerchange', at);
            profile.service_worker_trace.controller_changes.push({
                at_ms: at,
                controller_present: Boolean(controller),
                script_url: controller?.scriptURL || null,
                state: controller?.state || null
            });
        });
        navigator.serviceWorker.addEventListener('message', event => {
            if (profile.measurement_frozen_at_ms !== null) return;
            try {
                const data = event.data;
                if (!hlsRequest(data)) return;
                const range = typeof data.range === 'string' && data.range
                    ? data.range : null;
                const entry = {
                    id: profile.service_worker_trace.hls_requests.length + 1,
                    request_at_ms: performance.now(),
                    request_kind: range ? 'range' : 'stream-open',
                    url: String(data.url),
                    method: typeof data.method === 'string' ? data.method : null,
                    range,
                    stream_token: typeof data.streamToken === 'string'
                        ? data.streamToken : null,
                    network_id: Number.isFinite(data.networkId) ? data.networkId : null,
                    response_capture: 'request-only-no-port'
                };
                profile.service_worker_trace.hls_requests.push(entry);
                captureHlsRangeResponse(event.ports?.[0] || null, entry);
            } catch (error) {
                recordAttributionError('service-worker-range-request', error);
            }
        });
    }
    const rangeProgressTrace = profile.service_worker_trace.range_progress;
    const lastRangeProgressText = new Map();
    let firstRangeReferenceKey = null;
    let rangeProgressContainer = null;
    let rangeProgressObserver = null;
    let rangeProgressDiscoveryObserver = null;
    let rangeProgressStopped = false;
    const disconnectRangeProgressObservers = (
        reason, at = performance.now()
    ) => {
        if (rangeProgressStopped) return;
        rangeProgressStopped = true;
        rangeProgressObserver?.disconnect();
        rangeProgressDiscoveryObserver?.disconnect();
        rangeProgressTrace.observer.state = 'disconnected';
        rangeProgressTrace.observer.disconnected_at_ms = at;
        rangeProgressTrace.observer.disconnect_reason = reason;
    };
    const exactRangeWindowIndex = range => {
        const match = /^bytes=([0-9]+)-([0-9]+)$/.exec(range);
        if (!match) return null;
        const start = Number(match[1]), end = Number(match[2]);
        if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) ||
            start % HLS_STORAGE_WINDOW_BYTES !== 0) return null;
        const index = start / HLS_STORAGE_WINDOW_BYTES;
        if (!Number.isSafeInteger(index) || index < 0 || index >= 5 ||
            end !== (index + 1) * HLS_STORAGE_WINDOW_BYTES - 1) return null;
        return index;
    };
    const maybeDisconnectRangeProgressObservers = at => {
        if (rangeProgressTrace.first_playing_at_ms === null ||
            !rangeProgressTrace.terminal_windows.every(window =>
                window.state === 'done' || window.state === 'failed')) return;
        disconnectRangeProgressObservers(
            'first-playing-and-w0-w4-terminal', at
        );
    };
    const scanRangeProgressRows = () => {
        if (rangeProgressStopped ||
            rangeProgressContainer !== document.getElementById('progressRows')) return;
        const at = performance.now();
        for (const line of (rangeProgressContainer.textContent || '').split('\n')) {
            const text = line.trim();
            const match = /^hls-segment-range\s+([a-fA-F0-9]{64,128})\s+(bytes=[0-9]+-[0-9]+)\s+\[([^\]]+)\](?:\s+.*)?$/.exec(text);
            if (!match) continue;
            const referenceKey = match[1].toLowerCase();
            if (firstRangeReferenceKey === null) {
                firstRangeReferenceKey = referenceKey;
                rangeProgressTrace.first_reference = match[1];
            }
            if (referenceKey !== firstRangeReferenceKey) continue;
            const key = `${referenceKey}|${match[2]}`;
            if (lastRangeProgressText.get(key) === text) continue;
            lastRangeProgressText.set(key, text);
            if (rangeProgressTrace.transitions.length >= RANGE_PROGRESS_TRACE_CAP) {
                disconnectRangeProgressObservers('cap', at);
                break;
            }
            rangeProgressTrace.transitions.push({
                at_ms: at,
                reference: match[1],
                range: match[2],
                state: match[3],
                text
            });
            const windowIndex = exactRangeWindowIndex(match[2]);
            if (windowIndex !== null &&
                ['done', 'failed'].includes(match[3]) &&
                rangeProgressTrace.terminal_windows[windowIndex].state === null) {
                rangeProgressTrace.terminal_windows[windowIndex] = {
                    horizon: `W${windowIndex}`,
                    range: match[2],
                    reference: match[1],
                    state: match[3],
                    at_ms: at,
                    text
                };
            }
            maybeDisconnectRangeProgressObservers(at);
            if (rangeProgressStopped) break;
            if (rangeProgressTrace.transitions.length >= RANGE_PROGRESS_TRACE_CAP) {
                disconnectRangeProgressObservers('cap', at);
                break;
            }
        }
    };
    const attachRangeProgressObserver = () => {
        if (rangeProgressStopped || rangeProgressContainer) return;
        const progressRows = document.getElementById('progressRows');
        if (!progressRows) return;
        rangeProgressContainer = progressRows;
        rangeProgressDiscoveryObserver?.disconnect();
        rangeProgressObserver = new MutationObserver(scanRangeProgressRows);
        rangeProgressObserver.observe(progressRows, {
            childList: true, subtree: true, characterData: true
        });
        rangeProgressTrace.observer.state = 'observing';
        rangeProgressTrace.observer.attached_at_ms = performance.now();
        scanRangeProgressRows();
    };
    rangeProgressDiscoveryObserver = new MutationObserver(
        attachRangeProgressObserver
    );
    attachRangeProgressObserver();
    if (!rangeProgressContainer) {
        rangeProgressDiscoveryObserver.observe(document, {
            childList: true, subtree: true
        });
    }
    const firstSegmentWindowStates = progress => {
        const windows = Array.from({ length: 5 }, (_, index) => ({
            horizon: `W${index}`,
            range: `bytes=${index * HLS_STORAGE_WINDOW_BYTES}-${
                (index + 1) * HLS_STORAGE_WINDOW_BYTES - 1}`,
            state: null
        }));
        let referenceKey = firstRangeReferenceKey;
        for (const line of progress) {
            const match = /^hls-segment-range\s+([a-fA-F0-9]{64,128})\s+(bytes=([0-9]+)-([0-9]+))\s+\[([^\]]+)\](?:\s+.*)?$/.exec(line.trim());
            if (!match) continue;
            referenceKey ??= match[1].toLowerCase();
            if (match[1].toLowerCase() !== referenceKey) continue;
            const start = Number(match[3]), end = Number(match[4]);
            if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || end < start ||
                start % HLS_STORAGE_WINDOW_BYTES !== 0) continue;
            const index = start / HLS_STORAGE_WINDOW_BYTES;
            if (!Number.isSafeInteger(index) || index < 0 || index >= windows.length ||
                windows[index].state !== null) continue;
            windows[index] = {
                horizon: `W${index}`,
                range: match[2],
                state: match[5]
            };
        }
        return windows;
    };
    const bufferAhead = media => {
        let forward = 0;
        for (let i = 0; media && i < media.buffered.length; i++) {
            const start = media.buffered.start(i), end = media.buffered.end(i);
            if (media.currentTime >= start - .05 && media.currentTime <= end + .05)
                forward = Math.max(0, end - media.currentTime);
        }
        return forward;
    };
    const snapshot = (event, at = performance.now(), exactMedia = null, extra = {}) => {
        if (profile.measurement_frozen_at_ms !== null) return null;
        captureSourceWasmBuildId();
        const media = exactMedia || document.querySelector('video');
        const progress = (document.getElementById('progressRows')?.textContent || '')
            .split('\n').filter(Boolean);
        if (media) mark('attach', at);
        const forward = bufferAhead(media);
        const state = media?.getAttribute('data-weeb3-hls-state') ?? null;
        const timeline = media?.getAttribute('data-weeb3-hls-timeline') ?? null;
        if (state === 'manifest-ready') mark('manifest', at);
        if (event === 'playing') mark('first_playing', at);
        const row = {
            event,
            at_ms: at,
            current_time_s: media?.currentTime ?? null,
            duration_s: media?.duration ?? null,
            forward_buffer_s: forward,
            paused: media?.paused ?? null,
            ready_state: media?.readyState ?? null,
            state,
            timeline,
            connections: Number(document.getElementById('connections')?.textContent) || 0,
            priced_peers: Number(document.getElementById('connections')?.textContent) || 0,
            ongoing_connections: Number(document.getElementById('ongoing')?.textContent) || 0,
            hls_body_running: progress.filter(line =>
                line.startsWith('hls-segment ') && line.includes('[running]')).length,
            hls_range_running: progress.filter(line =>
                line.startsWith('hls-segment-range ') && line.includes('[running]')).length,
            first_segment_windows: firstSegmentWindowStates(progress),
            raw_startup: rawStartupTrace.latest,
            retrieval: retrievalProfileSnapshot(),
            ...extra
        };
        (event === 'sample' ? profile.samples : profile.events).push(row);
        return row;
    };
    const PRE_FRAME_SAMPLE_INTERVAL_MS = 50;
    const STEADY_SAMPLE_INTERVAL_MS = 500;
    let sampleTimer = null;
    const scheduleProfileSample = delay => {
        sampleTimer = setTimeout(() => {
            if (profile.measurement_frozen_at_ms !== null) {
                sampleTimer = null;
                return;
            }
            snapshot('sample');
            if (profile.measurement_frozen_at_ms !== null) {
                sampleTimer = null;
                return;
            }
            scheduleProfileSample(profile.first_presented_frame
                ? STEADY_SAMPLE_INTERVAL_MS : PRE_FRAME_SAMPLE_INTERVAL_MS);
        }, delay);
    };
    const beginSteadySampling = () => {
        if (sampleTimer !== null) clearTimeout(sampleTimer);
        scheduleProfileSample(STEADY_SAMPLE_INTERVAL_MS);
    };
    const frameCallbacksArmed = new WeakSet();
    const armFirstPresentedFrame = media => {
        if (!media || frameCallbacksArmed.has(media) ||
            typeof media.requestVideoFrameCallback !== 'function') return;
        frameCallbacksArmed.add(media);
        try {
            media.requestVideoFrameCallback((now, metadata) => {
                if (profile.first_presented_frame ||
                    profile.measurement_frozen_at_ms !== null) return;
                scanRangeProgressRows();
                mark('first_presented_frame', now);
                const row = snapshot('presented-frame', now, media, {
                    media_time_s: Number.isFinite(metadata?.mediaTime)
                        ? metadata.mediaTime : null,
                    presented_frames: Number.isFinite(metadata?.presentedFrames)
                        ? metadata.presentedFrames : null,
                    expected_display_time_ms: Number.isFinite(metadata?.expectedDisplayTime)
                        ? metadata.expectedDisplayTime : null
                });
                profile.first_presented_frame = { ...row };
                beginSteadySampling();
            });
        } catch (error) {
            profile.errors.push({
                at_ms: performance.now(), type: 'requestVideoFrameCallback',
                message: String(error?.message || error)
            });
        }
    };
    const discoverMedia = () => {
        if (profile.measurement_frozen_at_ms !== null) return;
        const media = document.querySelector('video');
        if (!media) return;
        mark('attach');
        armFirstPresentedFrame(media);
    };
    for (const name of [
        'playing', 'waiting', 'stalled', 'seeking', 'seeked', 'durationchange',
        'weeb3-hls-warmup-start', 'weeb3-hls-timeline-rebase'
    ]) {
        document.addEventListener(name, () => {
            if (profile.measurement_frozen_at_ms !== null) return;
            const at = performance.now();
            discoverMedia();
            if (name === 'weeb3-hls-warmup-start') mark('manifest', at);
            if (name === 'playing' && profile.marks.first_playing === undefined) {
                mark('first_playing', at);
                rangeProgressTrace.first_playing_at_ms = at;
                finalizeRollingGroupTrace('first-playing', at);
                scanRangeProgressRows();
                maybeDisconnectRangeProgressObservers(at);
            }
            snapshot(name, at);
        }, true);
    }
    new MutationObserver(discoverMedia)
        .observe(document, { childList: true, subtree: true });
    discoverMedia();
    const recordError = (type, value) => {
        if (profile.measurement_frozen_at_ms !== null) return;
        profile.errors.push({
            at_ms: performance.now(), type, message: String(value)
        });
    };
    addEventListener('error', event =>
        recordError('error', event.error?.stack || event.message));
    addEventListener('unhandledrejection', event =>
        recordError('unhandledrejection', event.reason?.stack || event.reason));
    window.__weeb3FreezeHlsProfileMeasurement = () => {
        if (profile.measurement_frozen_at_ms !== null)
            return profile.measurement_frozen_at_ms;
        profile.measurement_frozen_at_ms = performance.now();
        if (sampleTimer !== null) {
            clearTimeout(sampleTimer);
            sampleTimer = null;
        }
        return profile.measurement_frozen_at_ms;
    };
    window.__weeb3FinalizeHlsProfileDeadline = () => {
        if (!profile.retrieval_rolling_group_trace_capture.attempted &&
            profile.marks.first_playing === undefined) {
            finalizeRollingGroupTrace('deadline');
        }
        scanRangeProgressRows();
        disconnectRangeProgressObservers('deadline');
    };
    scheduleProfileSample(PRE_FRAME_SAMPLE_INTERVAL_MS);
})();
"#;

const HLS_PROFILE_RESULT_SCRIPT: &str = r#"
(async () => {
    window.__weeb3FinalizeHlsProfileDeadline();
    const measurementFrozenAtMs = window.__weeb3FreezeHlsProfileMeasurement();
    const profile = window.__weeb3HlsProfile;
    const retrievalProfileGetter = window.__weeb3GetHlsRetrieveProfileSnapshot;
    if (profile) {
        try {
            profile.retrieval_profile_final =
                typeof retrievalProfileGetter === 'function'
                    ? retrievalProfileGetter() : null;
        } catch (error) {
            profile.errors.push({
                at_ms: performance.now(),
                type: 'retrieval-profile-final',
                message: String(error?.message || error)
            });
            profile.retrieval_profile_final = null;
        }
    }
    const navigation = performance.getEntriesByType('navigation')[0] || null;
    const media = document.querySelector('video');
    const allLogLines = Array.from(document.querySelectorAll('#logsField > *'))
        .map(entry => entry.textContent || '');
    const logLines = allLogLines.slice(0, 100);
    const sourceBuildMatch = allLogLines
        .map(line => /\bInterface mounted, version ([0-9a-f]{16})\b/.exec(line))
        .find(match => match !== null) || null;
    const timing = resource => ({
        name: resource.name,
        initiator_type: resource.initiatorType,
        start_ms: resource.startTime,
        request_start_ms: resource.requestStart,
        response_start_ms: resource.responseStart,
        duration_ms: resource.duration,
        transfer_size: resource.transferSize,
        encoded_body_size: resource.encodedBodySize,
        decoded_body_size: resource.decodedBodySize
    });
    const measuredMedia = media ? {
        current_time_s: media.currentTime,
        duration_s: media.duration,
        paused: media.paused,
        ready_state: media.readyState,
        network_state: media.networkState,
        state: media.getAttribute('data-weeb3-hls-state'),
        mode: media.getAttribute('data-weeb3-hls-mode'),
        timeline: media.getAttribute('data-weeb3-hls-timeline')
    } : null;
    const measuredDiagnostics = {
        progress: document.getElementById('progressRows')?.textContent || null,
        result: document.getElementById('resultField')?.textContent || null,
        logs: logLines
    };
    const measuredNavigation = navigation ? {
        response_status: typeof navigation.responseStatus === 'number'
            ? navigation.responseStatus : null,
        response_end_ms: navigation.responseEnd,
        dom_content_loaded_ms: navigation.domContentLoadedEventEnd,
        load_event_ms: navigation.loadEventEnd,
        duration_ms: navigation.duration,
        transfer_size: navigation.transferSize,
        encoded_body_size: navigation.encodedBodySize
    } : null;
    const measuredResources = performance.getEntriesByType('resource')
        .slice(-8192).map(timing);
    const frozenProfile = JSON.parse(JSON.stringify(profile || {
        marks: {}, events: [], samples: [], errors: []
    }));

    await window.__weeb3ColdStartReady;
    const ping = worker => new Promise(resolve => {
        if (!worker) return resolve(null);
        const channel = new MessageChannel();
        let settled = false;
        const finish = value => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            channel.port1.close();
            resolve(value);
        };
        const timer = setTimeout(() => finish({ timeout: true }), 2000);
        channel.port1.onmessage = event => finish(event.data || null);
        channel.port1.start();
        try { worker.postMessage({ type: 'WEEB3_PING' }, [channel.port2]); }
        catch (error) { finish({ error: String(error?.message || error) }); }
    });
    const registration = await navigator.serviceWorker?.getRegistration();
    const controller = navigator.serviceWorker?.controller || null;
    const measured = {
        href: location.href,
        measured_at_ms: measurementFrozenAtMs,
        measurement_frozen_at_ms: measurementFrozenAtMs,
        profile: frozenProfile,
        media: measuredMedia,
        diagnostics: measuredDiagnostics,
        service_worker: {
            controlled: Boolean(controller),
            controller_script: controller?.scriptURL || null,
            controller_state: controller?.state || null,
            scope: registration?.scope || null,
            active_script: registration?.active?.scriptURL || null,
            active_state: registration?.active?.state || null,
            protocol: await ping(controller)
        },
        navigation: measuredNavigation,
        resources: measuredResources
    };

    const identityFetchStartedAtMs = performance.now();
    const sha256Hex = async payload => Array.from(new Uint8Array(
        await crypto.subtle.digest('SHA-256', payload)
    )).map(byte => byte.toString(16).padStart(2, '0')).join('');
    const probeAsset = async requestedUrl => {
        try {
            const response = await fetch(requestedUrl, { cache: 'no-store' });
            const payload = await response.arrayBuffer();
            return {
                source_url: response.url || requestedUrl,
                status: response.status,
                bytes: payload.byteLength,
                sha256: await sha256Hex(payload),
                build_version: response.headers.get('X-Weeb3-Build-Version'),
                etag: response.headers.get('ETag')
            };
        } catch (_) {
            return {
                source_url: requestedUrl,
                status: null,
                bytes: null,
                sha256: null,
                build_version: null,
                etag: null
            };
        }
    };
    const scopeBase = registration?.scope || new URL('./', location.href).href;
    const assetUrls = {
        index: new URL('./', scopeBase).href,
        service_js: new URL('service.js', scopeBase).href,
        javascript: new URL('weeb_3.js', scopeBase).href,
        wasm: new URL('weeb_3_bg.wasm', scopeBase).href
    };
    const [index, serviceJs, javascript, wasm] = await Promise.all([
        probeAsset(assetUrls.index),
        probeAsset(assetUrls.service_js),
        probeAsset(assetUrls.javascript),
        probeAsset(assetUrls.wasm)
    ]);
    measured.served_identity = {
        cache_mode: 'no-store',
        identity_fetch_started_at_ms: identityFetchStartedAtMs,
        identity_fetch_completed_at_ms: performance.now(),
        source_wasm_build_id:
            profile?.source_wasm_build_id || sourceBuildMatch?.[1] || null,
        assets: { index, service_js: serviceJs, javascript, wasm }
    };
    return JSON.stringify(measured);
})()
"#;

#[test]
fn weeb3_hls_profile() -> Result<()> {
    let Some(target_url) = env::var("WEEB3_HLS_PROFILE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        println!("WEEB3_HLS_PROFILE_URL is not set; skipping browser HLS profile");
        return Ok(());
    };
    let profile_seconds = env_u64("WEEB3_HLS_PROFILE_SECONDS", 90)?;
    if profile_seconds == 0 {
        return Err(anyhow!("WEEB3_HLS_PROFILE_SECONDS must be positive"));
    }

    let timeout = Duration::from_secs(profile_seconds.saturating_add(90));
    let browser = launch_fresh_edge(timeout)?;
    let tab = browser
        .new_tab()
        .map_err(|error| anyhow!("failed to open HLS profile tab: {error:?}"))?;
    tab.set_default_timeout(timeout);
    tab.call_method(ClearBrowserCache(None))
        .map_err(|error| anyhow!("failed to clear Edge's HTTP cache: {error:?}"))?;
    tab.call_method(SetCacheDisabled {
        cache_disabled: true,
    })
    .map_err(|error| anyhow!("failed to disable Edge's HTTP cache: {error:?}"))?;
    tab.call_method(AddScriptToEvaluateOnNewDocument {
        source: HLS_PROFILE_SCRIPT.to_string(),
        world_name: None,
        include_command_line_api: None,
        run_immediately: None,
    })
    .map_err(|error| anyhow!("failed to install HLS instrumentation: {error:?}"))?;

    let started = Instant::now();
    tab.navigate_to(&target_url)
        .map_err(|error| anyhow!("failed to navigate to HLS profile URL: {error:?}"))?
        .wait_until_navigated()
        .map_err(|error| anyhow!("HLS profile navigation did not finish: {error:?}"))?;
    let deadline = started + Duration::from_secs(profile_seconds);
    thread::sleep(deadline.saturating_duration_since(Instant::now()));

    let remote = tab
        .evaluate(HLS_PROFILE_RESULT_SCRIPT, true)
        .map_err(|error| anyhow!("failed to read HLS profile: {error:?}"))?;
    let raw = remote
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("Edge did not return HLS profile JSON"))?;
    let browser_metrics: Value =
        serde_json_crates_io::from_str(&raw).context("failed to parse HLS profile JSON")?;

    let first_playing_ms = browser_metrics
        .pointer("/profile/marks/first_playing")
        .and_then(Value::as_f64);
    let first_playing_current_time_s =
        hls_first_event_value(&browser_metrics, "playing", "current_time_s");
    let samples = hls_metric_samples(&browser_metrics);
    let buffer = summarize_hls_buffer(
        &samples,
        first_playing_ms,
        env_u64("WEEB3_HLS_TREND_WINDOW_SECONDS", 15)? as f64 * 1000.0,
    );
    let playback_checkpoints =
        summarize_hls_playback_checkpoints(&browser_metrics, first_playing_ms);
    let cold_start_attribution = summarize_hls_cold_start_attribution(&browser_metrics);
    let waiting_episodes = summarize_hls_waiting_episodes(&browser_metrics, first_playing_ms);
    let duration_changes = summarize_hls_duration_changes(&browser_metrics, first_playing_ms);
    let retrieval_profile = summarize_hls_retrieval_profile(&browser_metrics, first_playing_ms);
    let post_start_waiting = hls_event_count(&browser_metrics, "waiting", first_playing_ms);
    let post_start_stalled = hls_event_count(&browser_metrics, "stalled", first_playing_ms);
    let post_start_seeking = hls_event_count(&browser_metrics, "seeking", first_playing_ms);
    let post_start_seeked = hls_event_count(&browser_metrics, "seeked", first_playing_ms);
    let timeline_rebases = hls_event_count(
        &browser_metrics,
        "weeb3-hls-timeline-rebase",
        first_playing_ms,
    );
    let summary = json!({
        "navigation_ms": 0.0,
        "attach_ms": browser_metrics.pointer("/profile/marks/attach").and_then(Value::as_f64),
        "manifest_ms": browser_metrics.pointer("/profile/marks/manifest").and_then(Value::as_f64),
        "first_playing_ms": first_playing_ms,
        "first_playing_current_time_s": first_playing_current_time_s,
        "first_presented_frame_ms": browser_metrics.pointer("/profile/marks/first_presented_frame").and_then(Value::as_f64),
        "cold_start_attribution": cold_start_attribution,
        "retrieval_profile": retrieval_profile,
        "playback_checkpoints": playback_checkpoints,
        "post_start_duration": duration_changes,
        "timeline": browser_metrics.pointer("/media/timeline").and_then(Value::as_str),
        "post_start_waiting_count": post_start_waiting,
        "post_start_waiting_episodes": waiting_episodes,
        "post_start_stalled_count": post_start_stalled,
        "post_start_seeking_count": post_start_seeking,
        "post_start_seeked_count": post_start_seeked,
        "timeline_rebase_count": timeline_rebases,
        "timeline_jump_count": buffer.timeline_jump_count,
        "first_minute": buffer.first_minute,
        "final": buffer.final_sample,
        "early_low_water_s": buffer.early_low_water_s,
        "late_low_water_s": buffer.late_low_water_s,
        "low_water_trend_s": buffer.low_water_trend_s,
        "matched_trend_window_ms": buffer.matched_window_ms
    });
    let report = json!({
        "target_url": target_url,
        "profile_seconds": profile_seconds,
        "summary": summary,
        "browser": browser_metrics
    });

    fs::create_dir_all("target/weeb3-hls-profile")
        .context("failed to create target/weeb3-hls-profile directory")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let output = format!("target/weeb3-hls-profile/hls-profile-{timestamp}.json");
    fs::write(
        &output,
        serde_json_crates_io::to_string_pretty(&report)
            .context("failed to serialize HLS profile")?,
    )
    .context("failed to write HLS profile")?;
    println!("HLS profile written to {output}");
    println!("HLS profile summary: {summary}");

    let expected_scope = env::var("WEEB3_HLS_EXPECT_SW_SCOPE")
        .ok()
        .filter(|scope| !scope.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_WORKER_SCOPE.to_string());
    validate_cold_browser_start(&report["browser"])?;
    validate_service_worker(&report["browser"], &expected_scope)?;
    validate_raw_startup_trace(&report["browser"])?;
    validate_retrieval_profile(&report["browser"])?;
    validate_rolling_group_trace(&report["browser"])?;
    validate_first_reference_terminal_windows(&report["browser"])?;
    validate_served_identity(&report["browser"])?;

    let first_playing_ms =
        first_playing_ms.ok_or_else(|| anyhow!("HLS playback never emitted playing"))?;
    if let Some(limit) = env_optional_u64("WEEB3_HLS_MAX_STARTUP_MS")? {
        if first_playing_ms > limit as f64 {
            return Err(anyhow!(
                "HLS startup {first_playing_ms:.0}ms exceeded {limit}ms"
            ));
        }
    }
    if env_bool("WEEB3_HLS_REQUIRE_NO_STALLS", false)
        && post_start_waiting + post_start_stalled != 0
    {
        return Err(anyhow!(
            "HLS playback waited {post_start_waiting} time(s) and stalled {post_start_stalled} time(s)"
        ));
    }
    if env_bool("WEEB3_HLS_REQUIRE_STABLE_TIMELINE", false) {
        if post_start_seeking != 0 || post_start_seeked != 0 {
            return Err(anyhow!(
                "HLS playback sought {post_start_seeking} time(s) and completed {post_start_seeked} seek(s) after starting"
            ));
        }
        if timeline_rebases != 0 {
            return Err(anyhow!(
                "HLS playback rebased its timeline {timeline_rebases} time(s)"
            ));
        }
        if buffer.timeline_jump_count != 0 {
            return Err(anyhow!(
                "HLS currentTime jumped {} time(s)",
                buffer.timeline_jump_count
            ));
        }
    }
    if env_bool("WEEB3_HLS_REQUIRE_ABSOLUTE_TIMELINE", false)
        && browser_metrics
            .pointer("/media/timeline")
            .and_then(Value::as_str)
            != Some("absolute")
    {
        return Err(anyhow!(
            "HLS playback did not establish an absolute media timeline"
        ));
    }
    if let Some(expected) = env_optional_f64("WEEB3_HLS_EXPECT_FIRST_TIME_SECONDS")? {
        let tolerance = env_optional_f64("WEEB3_HLS_FIRST_TIME_TOLERANCE_SECONDS")?.unwrap_or(1.0);
        if tolerance < 0.0 {
            return Err(anyhow!(
                "WEEB3_HLS_FIRST_TIME_TOLERANCE_SECONDS must be non-negative"
            ));
        }
        let actual = first_playing_current_time_s
            .ok_or_else(|| anyhow!("HLS playback never reported its first playing time"))?;
        if (actual - expected).abs() > tolerance {
            return Err(anyhow!(
                "HLS first playing time {actual:.6}s differed from expected {expected:.6}s by more than {tolerance:.6}s"
            ));
        }
    }
    if let Some(limit) = env_optional_f64("WEEB3_HLS_MAX_DURATION_JUMP_SECONDS")? {
        validate_hls_duration_jump(&duration_changes, limit)?;
    }
    if let Some(minimum) = env_optional_f64("WEEB3_HLS_MIN_LOW_WATER_TREND_SECONDS")? {
        let trend = buffer.low_water_trend_s.ok_or_else(|| {
            anyhow!("HLS profile is too short to calculate matched post-minute windows")
        })?;
        if trend < minimum {
            return Err(anyhow!(
                "HLS low-water trend {trend:.3}s was below {minimum:.3}s"
            ));
        }
    }

    Ok(())
}

fn launch_fresh_edge(timeout: Duration) -> Result<Browser> {
    let edge = edge_executable()?;
    let ignore_certificate_errors = env_bool("WEEB3_IGNORE_CERTIFICATE_ERRORS", true);
    let mut args = vec![
        OsStr::new("--disable-background-networking"),
        OsStr::new("--disable-cache"),
        OsStr::new("--disable-dev-shm-usage"),
        OsStr::new("--disable-extensions"),
        OsStr::new("--no-first-run"),
        OsStr::new("--autoplay-policy=no-user-gesture-required"),
    ];
    if ignore_certificate_errors {
        args.push(OsStr::new("--ignore-certificate-errors"));
    }

    let mut builder = LaunchOptionsBuilder::default();
    builder
        .path(Some(edge))
        .headless(!env_bool("WEEB3_HEADFUL", false))
        .ignore_certificate_errors(ignore_certificate_errors)
        .sandbox(!env_bool("WEEB3_CHROME_NO_SANDBOX", true))
        .window_size(Some((1280, 720)))
        .idle_browser_timeout(timeout + Duration::from_secs(30))
        .args(args);

    // With no explicit user_data_dir, headless_chrome creates and later removes
    // a new temporary Edge profile for every Browser instance.
    let options = builder
        .build()
        .map_err(|error| anyhow!("failed to build Edge launch options: {error:?}"))?;
    Browser::new(options).map_err(|error| anyhow!("failed to launch Edge: {error:?}"))
}

fn edge_executable() -> Result<PathBuf> {
    if let Some(path) = env::var_os("WEEB3_CHROME").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!(
            "WEEB3_CHROME does not identify an Edge executable: {}",
            path.display()
        ));
    }

    let mut candidates = Vec::new();
    for base in [
        env::var_os("ProgramFiles(x86)"),
        env::var_os("ProgramFiles"),
        env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(
            Path::new(&base)
                .join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe"),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!("Microsoft Edge was not found; set WEEB3_CHROME to the msedge executable")
        })
}

fn validate_service_worker(metrics: &Value, expected_scope: &str) -> Result<()> {
    let service_worker = metrics
        .get("service_worker")
        .ok_or_else(|| anyhow!("HLS profile did not report Service Worker state"))?;
    if service_worker.get("controlled").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            "HLS playback did not finish under a controlling Service Worker"
        ));
    }
    let pong = service_worker
        .get("protocol")
        .ok_or_else(|| anyhow!("controlling Service Worker did not answer WEEB3_PING"))?;
    if pong.get("type").and_then(Value::as_str) != Some("WEEB3_PONG") {
        return Err(anyhow!(
            "controlling Service Worker returned an invalid WEEB3_PING response: {pong}"
        ));
    }
    if pong.get("protocol").and_then(Value::as_u64) != Some(SERVICE_WORKER_PROTOCOL) {
        return Err(anyhow!(
            "controlling Service Worker did not answer protocol {SERVICE_WORKER_PROTOCOL}: {pong}"
        ));
    }
    if pong.get("scope").and_then(Value::as_str) != Some(expected_scope) {
        return Err(anyhow!(
            "controlling Service Worker answered for the wrong scope; expected {expected_scope}: {pong}"
        ));
    }
    Ok(())
}

fn validate_cold_browser_start(metrics: &Value) -> Result<()> {
    let cold_start = metrics
        .pointer("/profile/cold_start")
        .ok_or_else(|| anyhow!("HLS profile did not report its document-start browser state"))?;
    if cold_start
        .get("controller_present_at_document_start")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err(anyhow!(
            "HLS profile did not start without a Service Worker controller: {cold_start}"
        ));
    }
    let registrations = cold_start
        .get("registrations")
        .ok_or_else(|| anyhow!("HLS profile did not probe initial Service Worker registrations"))?;
    if registrations.get("status").and_then(Value::as_str) != Some("resolved")
        || registrations.get("count").and_then(Value::as_u64) != Some(0)
    {
        return Err(anyhow!(
            "HLS profile did not start with an empty Service Worker registration set: {registrations}"
        ));
    }
    let cache_storage = cold_start
        .get("cache_storage")
        .ok_or_else(|| anyhow!("HLS profile did not probe initial Cache Storage state"))?;
    if cache_storage.get("status").and_then(Value::as_str) != Some("resolved")
        || cache_storage.get("count").and_then(Value::as_u64) != Some(0)
    {
        return Err(anyhow!(
            "HLS profile did not start with empty Cache Storage: {cache_storage}"
        ));
    }
    for flag in [
        "http_cache_cleared_before_navigation",
        "http_cache_disabled_before_navigation",
    ] {
        if cold_start.get(flag).and_then(Value::as_bool) != Some(true) {
            return Err(anyhow!("HLS profile did not prove {flag}"));
        }
    }
    Ok(())
}

fn retrieval_profile_u64(snapshot: &Value, field: &str) -> Result<u64> {
    snapshot
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("retrieval profile has invalid or missing {field}"))
}

fn validate_retrieval_profile(metrics: &Value) -> Result<()> {
    let snapshot = metrics
        .pointer("/profile/retrieval_profile_final")
        .filter(|snapshot| snapshot.is_object())
        .ok_or_else(|| {
            anyhow!(
                "enabled retrieval profiler did not install its getter or expose a final snapshot"
            )
        })?;
    if snapshot.get("schema_version").and_then(Value::as_u64) != Some(1)
        || snapshot.get("enabled").and_then(Value::as_bool) != Some(true)
    {
        return Err(anyhow!("invalid retrieval profile schema: {snapshot}"));
    }
    for field in ["activation_at_ms", "snapshot_at_ms"] {
        if !snapshot
            .get(field)
            .and_then(Value::as_f64)
            .is_some_and(|value| value.is_finite() && value >= 0.0)
        {
            return Err(anyhow!("retrieval profile has invalid {field}"));
        }
    }

    let permit_capacity = retrieval_profile_u64(snapshot, "permit_capacity")?;
    if permit_capacity == 0 {
        return Err(anyhow!("retrieval profile reported zero permit capacity"));
    }
    for field in ["permits_current", "permits_high_water"] {
        let value = retrieval_profile_u64(snapshot, field)?;
        if value > permit_capacity {
            return Err(anyhow!(
                "retrieval profile {field} {value} exceeded capacity {permit_capacity}"
            ));
        }
    }

    let tickets_created = retrieval_profile_u64(snapshot, "tickets_created")?;
    let enqueue_resolved = retrieval_profile_u64(snapshot, "enqueue_accepted")?
        .checked_add(retrieval_profile_u64(snapshot, "enqueue_rejected")?)
        .ok_or_else(|| anyhow!("retrieval profile enqueue totals overflowed"))?;
    if tickets_created != enqueue_resolved {
        return Err(anyhow!(
            "retrieval profile has {tickets_created} tickets but {enqueue_resolved} enqueue resolutions"
        ));
    }

    let scope_mappings = [
        ("tickets_created", "tickets_created"),
        ("enqueue_accepted", "enqueue_accepted"),
        ("enqueue_rejected", "enqueue_rejected"),
        ("relay_forward_succeeded", "relay_forward_succeeded"),
        ("relay_forward_failed", "relay_forward_failed"),
        ("queue_dequeued", "queue_dequeued"),
        ("logical_completed", "logical_completed"),
        ("permit_wait_acquired", "permit_acquired"),
        ("physical_dispatched", "physical_dispatched"),
        (
            "physical_immediate_completed",
            "physical_immediate_completed",
        ),
        ("physical_timed_out", "physical_timed_out"),
        ("detached_completed", "detached_completed"),
        (
            "immediate_result_send_succeeded",
            "immediate_result_send_succeeded",
        ),
        (
            "immediate_result_send_failed",
            "immediate_result_send_failed",
        ),
        (
            "timeout_result_send_succeeded",
            "timeout_result_send_succeeded",
        ),
        ("timeout_result_send_failed", "timeout_result_send_failed"),
    ];
    let by_scope = snapshot
        .get("by_scope")
        .ok_or_else(|| anyhow!("retrieval profile did not expose by_scope counters"))?;
    let stream_scoped = by_scope
        .get("stream_scoped")
        .ok_or_else(|| anyhow!("retrieval profile did not expose stream_scoped counters"))?;
    let unscoped = by_scope
        .get("unscoped")
        .ok_or_else(|| anyhow!("retrieval profile did not expose unscoped counters"))?;
    for (aggregate_field, scoped_field) in scope_mappings {
        let aggregate = retrieval_profile_u64(snapshot, aggregate_field)?;
        let scoped_total = retrieval_profile_u64(stream_scoped, scoped_field)?
            .checked_add(retrieval_profile_u64(unscoped, scoped_field)?)
            .ok_or_else(|| anyhow!("retrieval profile scoped {scoped_field} total overflowed"))?;
        if aggregate != scoped_total {
            return Err(anyhow!(
                "retrieval profile aggregate {aggregate_field}={aggregate} differs from scoped {scoped_field}={scoped_total}"
            ));
        }
    }

    let conservation = snapshot
        .get("conservation")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("retrieval profile did not expose conservation balances"))?;
    if conservation.len() != RETRIEVAL_PROFILE_CONSERVATION_FIELDS.len() {
        return Err(anyhow!(
            "retrieval profile exposed an unexpected conservation schema"
        ));
    }
    for field in RETRIEVAL_PROFILE_CONSERVATION_FIELDS {
        let value = conservation.get(*field);
        if value.and_then(Value::as_i64) != Some(0) {
            return Err(anyhow!(
                "retrieval profile conservation balance {field} was {value:?}, expected zero"
            ));
        }
    }

    let bucket_bounds = snapshot
        .get("log2_bucket_upper_bounds_ms")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("retrieval profile did not expose log2 bucket bounds"))?;
    if bucket_bounds.len() != 32
        || bucket_bounds
            .iter()
            .enumerate()
            .any(|(index, bound)| bound.as_u64() != Some(1_u64 << index))
    {
        return Err(anyhow!("retrieval profile has invalid log2 bucket bounds"));
    }
    for field in RETRIEVAL_PROFILE_HISTOGRAM_FIELDS {
        let histogram = snapshot
            .get(*field)
            .ok_or_else(|| anyhow!("retrieval profile did not expose histogram {field}"))?;
        let count = retrieval_profile_u64(histogram, "count")?;
        let buckets = histogram
            .get("buckets")
            .and_then(Value::as_array)
            .filter(|buckets| buckets.len() == bucket_bounds.len())
            .ok_or_else(|| anyhow!("retrieval profile histogram {field} has invalid buckets"))?;
        let bucket_count = buckets
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(value.as_u64()?));
        if bucket_count != Some(count) {
            return Err(anyhow!(
                "retrieval profile histogram {field} count did not match its buckets"
            ));
        }
        let sum_ms = histogram.get("sum_ms").and_then(Value::as_f64);
        let max_ms = histogram.get("max_ms").and_then(Value::as_f64);
        if !sum_ms.is_some_and(|value| value.is_finite() && value >= 0.0)
            || !max_ms.is_some_and(|value| value.is_finite() && value >= 0.0)
            || (count == 0 && (sum_ms != Some(0.0) || max_ms != Some(0.0)))
            || (count > 0 && max_ms.unwrap_or_default() > sum_ms.unwrap_or_default())
        {
            return Err(anyhow!(
                "retrieval profile histogram {field} has invalid timing totals"
            ));
        }
    }
    Ok(())
}

fn required_u64(value: &Value, field: &str, context: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("{context} has invalid or missing {field}"))
}

fn required_finite_nonnegative(value: &Value, field: &str, context: &str) -> Result<f64> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| anyhow!("{context} has invalid or missing {field}"))
}

fn checked_counter_sum(value: &Value, fields: &[&str], context: &str) -> Result<u64> {
    fields.iter().try_fold(0_u64, |sum, field| {
        sum.checked_add(required_u64(value, field, context)?)
            .ok_or_else(|| anyhow!("{context} counters overflowed"))
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn captured_critical_prefix_windows(metrics: &Value) -> Result<Option<u64>> {
    let mut captured = None::<u64>;
    for value in metrics
        .pointer("/profile/service_worker_trace/hls_requests")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|request| request.get("response"))
        .filter_map(|response| response.get("header_fields"))
        .filter_map(|headers| headers.get("X-Weeb3-HLS-Critical-Prefix-Windows"))
        .filter_map(Value::as_str)
    {
        let windows = value.parse::<u64>().map_err(|_| {
            anyhow!("invalid captured X-Weeb3-HLS-Critical-Prefix-Windows value {value:?}")
        })?;
        if windows == 0 {
            return Err(anyhow!(
                "captured X-Weeb3-HLS-Critical-Prefix-Windows was zero"
            ));
        }
        captured = Some(captured.map_or(windows, |current| current.max(windows)));
    }
    Ok(captured)
}

#[derive(Debug)]
struct RollingGroupObserved {
    init_at_ms: f64,
    data_count: u64,
    parity_count: u64,
    miss_count: u64,
    terminal: bool,
    admissions: BTreeMap<u64, String>,
    results: BTreeSet<u64>,
    parity_cached: u64,
    parity_joined: u64,
    parity_led: u64,
    parity_valid: u64,
    parity_invalid: u64,
    last_active: u64,
    last_successes: u64,
    last_completed: u64,
}

fn validate_rolling_group_trace(metrics: &Value) -> Result<()> {
    const EXPECTED_CAP: u64 = 2_048;
    const PARITY_GATE_MS: u64 = 1_000;
    let profile = metrics
        .get("profile")
        .ok_or_else(|| anyhow!("HLS profile did not expose its measurement profile"))?;
    let profile_time_origin = required_finite_nonnegative(
        profile,
        "performance_time_origin_ms",
        "HLS measurement profile",
    )?;
    if profile_time_origin == 0.0 {
        return Err(anyhow!("HLS measurement profile has a zero time origin"));
    }
    let measurement_frozen_at = required_finite_nonnegative(
        profile,
        "measurement_frozen_at_ms",
        "HLS measurement profile",
    )?;
    let capture = profile
        .get("retrieval_rolling_group_trace_capture")
        .ok_or_else(|| anyhow!("HLS profile did not report rolling-group capture state"))?;
    if capture.get("attempted").and_then(Value::as_bool) != Some(true)
        || capture.get("getter_present").and_then(Value::as_bool) != Some(true)
        || required_u64(capture, "call_count", "rolling-group capture")? != 1
    {
        return Err(anyhow!(
            "rolling-group finalizer was not present and called exactly once: {capture}"
        ));
    }
    let capture_time_origin = required_finite_nonnegative(
        capture,
        "performance_time_origin_ms",
        "rolling-group capture",
    )?;
    if capture_time_origin != profile_time_origin {
        return Err(anyhow!(
            "rolling-group capture and profile disagree on performance.timeOrigin"
        ));
    }
    let capture_at = required_finite_nonnegative(capture, "at_ms", "rolling-group capture")?;
    if capture_at > measurement_frozen_at {
        return Err(anyhow!(
            "rolling-group capture occurred after measurement freeze"
        ));
    }
    let capture_reason = capture
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("rolling-group capture did not expose its reason"))?;
    match profile
        .pointer("/marks/first_playing")
        .and_then(Value::as_f64)
    {
        Some(first_playing) => {
            if capture_reason != "first-playing" || capture_at != first_playing {
                return Err(anyhow!(
                    "rolling-group trace was not frozen exactly at first playing"
                ));
            }
        }
        None if capture_reason == "deadline" => {}
        None => {
            return Err(anyhow!(
                "rolling-group deadline fallback has invalid reason {capture_reason:?}"
            ));
        }
    }

    let trace = profile
        .get("retrieval_rolling_group_trace")
        .filter(|trace| trace.is_object())
        .ok_or_else(|| anyhow!("rolling-group finalizer returned no snapshot"))?;
    if trace.get("schema_version").and_then(Value::as_u64) != Some(1)
        || trace.get("cap").and_then(Value::as_u64) != Some(EXPECTED_CAP)
    {
        return Err(anyhow!("invalid rolling-group trace schema: {trace}"));
    }
    let activation_at =
        required_finite_nonnegative(trace, "activation_at_ms", "rolling-group trace")?;
    let snapshot_at = required_finite_nonnegative(trace, "snapshot_at_ms", "rolling-group trace")?;
    if snapshot_at < activation_at {
        return Err(anyhow!(
            "rolling-group snapshot predates profiler activation"
        ));
    }
    let converted_capture_at = profile_time_origin + capture_at;
    if !converted_capture_at.is_finite() || (snapshot_at - converted_capture_at).abs() > 5_000.0 {
        return Err(anyhow!(
            "rolling-group epoch timestamp is incoherent with performance.timeOrigin"
        ));
    }
    let events = trace
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("rolling-group trace did not expose events"))?;
    let events_len = u64::try_from(events.len())
        .map_err(|_| anyhow!("rolling-group event count did not fit u64"))?;
    let events_attempted = required_u64(trace, "events_attempted", "rolling-group trace")?;
    let dropped = required_u64(trace, "dropped", "rolling-group trace")?;
    if events_len > EXPECTED_CAP
        || events_attempted != events_len.saturating_add(dropped)
        || dropped != 0
        || trace.get("truncated").and_then(Value::as_bool) != Some(false)
    {
        return Err(anyhow!(
            "rolling-group trace violated cap algebra or was truncated"
        ));
    }

    let groups_started = required_u64(trace, "groups_started", "rolling-group trace")?;
    let groups_dynamic_eligible =
        required_u64(trace, "groups_dynamic_eligible", "rolling-group trace")?;
    let groups_dynamic_ineligible =
        required_u64(trace, "groups_dynamic_ineligible", "rolling-group trace")?;
    let groups_active = required_u64(trace, "groups_active", "rolling-group trace")?;
    let groups_terminal = required_u64(trace, "groups_terminal", "rolling-group trace")?;
    if groups_started
        != groups_dynamic_eligible
            .checked_add(groups_dynamic_ineligible)
            .ok_or_else(|| anyhow!("rolling-group eligibility counters overflowed"))?
        || groups_started
            != groups_active
                .checked_add(groups_terminal)
                .ok_or_else(|| anyhow!("rolling-group lifecycle counters overflowed"))?
    {
        return Err(anyhow!(
            "rolling-group aggregate lifecycle counters do not balance"
        ));
    }
    let terminal_fields = [
        "terminal_direct_all_ready",
        "terminal_reconstruct_threshold",
        "terminal_stale",
        "terminal_channel_closed",
        "terminal_error",
    ];
    if checked_counter_sum(trace, &terminal_fields, "rolling-group trace")? != groups_terminal {
        return Err(anyhow!(
            "rolling-group terminal reason counters do not balance"
        ));
    }
    let parity_admitted = required_u64(trace, "parity_admitted", "rolling-group trace")?;
    if checked_counter_sum(
        trace,
        &["parity_cached", "parity_joined", "parity_led"],
        "rolling-group trace",
    )? != parity_admitted
    {
        return Err(anyhow!(
            "rolling-group parity registration counters do not balance"
        ));
    }
    let parity_results = checked_counter_sum(
        trace,
        &["parity_valid", "parity_invalid"],
        "rolling-group trace",
    )?;
    if parity_results > parity_admitted {
        return Err(anyhow!("rolling-group parity results exceeded admissions"));
    }

    let promotions = required_u64(
        trace,
        "managed_to_ordinary_promotions",
        "rolling-group trace",
    )?;
    let promotion_time = |field: &str| -> Result<Option<f64>> {
        match trace.get(field) {
            Some(value) if value.is_null() => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|value| {
                    value.is_finite() && *value >= activation_at && *value <= snapshot_at
                })
                .map(Some)
                .ok_or_else(|| anyhow!("rolling-group trace has invalid {field}")),
            None => Err(anyhow!("rolling-group trace did not expose {field}")),
        }
    };
    let first_promotion = promotion_time("first_managed_to_ordinary_promotion_at_ms")?;
    let last_promotion = promotion_time("last_managed_to_ordinary_promotion_at_ms")?;
    if (promotions == 0 && (first_promotion.is_some() || last_promotion.is_some()))
        || (promotions > 0
            && (!matches!((first_promotion, last_promotion), (Some(first), Some(last)) if first <= last)))
    {
        return Err(anyhow!(
            "rolling-group promotion count and timestamps disagree"
        ));
    }

    let mut groups = BTreeMap::<u64, RollingGroupObserved>::new();
    let mut previous_at = activation_at;
    let mut derived_dynamic_eligible = 0_u64;
    let derived_dynamic_ineligible = 0_u64;
    let mut derived_terminals = BTreeMap::<String, u64>::new();
    let mut derived_parity_cached = 0_u64;
    let mut derived_parity_joined = 0_u64;
    let mut derived_parity_led = 0_u64;
    let mut derived_parity_valid = 0_u64;
    let mut derived_parity_invalid = 0_u64;

    for (event_index, event) in events.iter().enumerate() {
        let event_name = event
            .get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("rolling-group event {event_index} has no event name"))?;
        let time_field = if event_name == "parity-admission" {
            "decision_at_ms"
        } else {
            "at_ms"
        };
        let at = required_finite_nonnegative(
            event,
            time_field,
            &format!("rolling-group event {event_index}"),
        )?;
        if at < previous_at || at > snapshot_at {
            return Err(anyhow!(
                "rolling-group event {event_index} has an out-of-order timestamp"
            ));
        }
        previous_at = at;
        let group_id = required_u64(
            event,
            "group_id",
            &format!("rolling-group event {event_index}"),
        )?;
        if group_id == 0 {
            return Err(anyhow!(
                "rolling-group event {event_index} has group id zero"
            ));
        }

        match event_name {
            "init" => {
                let expected_id = u64::try_from(groups.len())
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| anyhow!("rolling-group id sequence overflowed"))?;
                if group_id != expected_id || groups.contains_key(&group_id) {
                    return Err(anyhow!(
                        "rolling-group init {event_index} has a duplicate or discontinuous id"
                    ));
                }
                let requested = required_u64(event, "requested_count", "rolling init")?;
                let data = required_u64(event, "data_count", "rolling init")?;
                let parity = required_u64(event, "parity_count", "rolling init")?;
                let decoded_raw = required_u64(event, "decoded_raw_count", "rolling init")?;
                let decoded_only = required_u64(event, "decoded_only_count", "rolling init")?;
                let misses = required_u64(event, "miss_count", "rolling init")?;
                let initial_cached = required_u64(event, "initial_cached", "rolling init")?;
                let initial_joined = required_u64(event, "initial_joined", "rolling init")?;
                let initial_led = required_u64(event, "initial_led", "rolling init")?;
                let initial_active = required_u64(event, "initial_active", "rolling init")?;
                let initial_successes = required_u64(event, "initial_successes", "rolling init")?;
                let decoded_total = decoded_raw
                    .checked_add(decoded_only)
                    .and_then(|value| value.checked_add(misses))
                    .ok_or_else(|| anyhow!("rolling init shard counters overflowed"))?;
                let initial_registered = initial_cached
                    .checked_add(initial_joined)
                    .and_then(|value| value.checked_add(initial_led))
                    .ok_or_else(|| anyhow!("rolling init registration counters overflowed"))?;
                if requested == 0
                    || requested != data
                    || parity == 0
                    || data.checked_add(parity).is_none_or(|total| total > 256)
                    || decoded_total != requested
                    || misses == 0
                    || decoded_only >= parity
                    || initial_registered != misses
                    || initial_active != initial_joined.saturating_add(initial_led)
                    || initial_active > data
                    || initial_successes != decoded_raw
                    || event.get("static_candidate").and_then(Value::as_bool) != Some(true)
                    || event.get("dynamic_eligible").and_then(Value::as_bool) != Some(true)
                {
                    return Err(anyhow!(
                        "rolling-group init {event_index} violates full-group eligibility: {event}"
                    ));
                }
                derived_dynamic_eligible += 1;
                groups.insert(
                    group_id,
                    RollingGroupObserved {
                        init_at_ms: at,
                        data_count: data,
                        parity_count: parity,
                        miss_count: misses,
                        terminal: false,
                        admissions: BTreeMap::new(),
                        results: BTreeSet::new(),
                        parity_cached: 0,
                        parity_joined: 0,
                        parity_led: 0,
                        parity_valid: 0,
                        parity_invalid: 0,
                        last_active: initial_active,
                        last_successes: initial_successes,
                        last_completed: 0,
                    },
                );
            }
            "parity-admission" => {
                let group = groups.get_mut(&group_id).ok_or_else(|| {
                    anyhow!("rolling parity admission references unknown group {group_id}")
                })?;
                if group.terminal {
                    return Err(anyhow!("rolling parity admission followed group terminal"));
                }
                let gate_elapsed = required_u64(event, "gate_elapsed_ms", "parity admission")?;
                let shard = required_u64(event, "shard_index", "parity admission")?;
                let offset = required_u64(event, "parity_offset", "parity admission")?;
                let registration = event
                    .get("registration")
                    .and_then(Value::as_str)
                    .filter(|value| matches!(*value, "Cached" | "Joined" | "Led"))
                    .ok_or_else(|| anyhow!("rolling parity admission has invalid registration"))?;
                let active_before = required_u64(event, "active_before", "parity admission")?;
                let active_after = required_u64(event, "active_after", "parity admission")?;
                let successes = required_u64(event, "successes", "parity admission")?;
                let completed = required_u64(event, "completed", "parity admission")?;
                let gate_delta_ms = at - group.init_at_ms;
                let expected_active_after = if registration == "Cached" {
                    Some(active_before)
                } else {
                    active_before.checked_add(1)
                };
                if gate_elapsed < PARITY_GATE_MS
                    || gate_delta_ms < gate_elapsed as f64
                    || gate_delta_ms >= gate_elapsed.saturating_add(1) as f64
                    || offset >= group.parity_count
                    || group.data_count.checked_add(offset) != Some(shard)
                    || expected_active_after != Some(active_after)
                    || active_before >= group.data_count
                    || active_after > group.data_count
                    || active_before > group.last_active
                    || successes < group.last_successes
                    || completed < group.last_completed
                    || successes > group.data_count.saturating_add(group.parity_count)
                    || completed
                        > group
                            .miss_count
                            .saturating_add(group.admissions.len() as u64)
                    || group
                        .admissions
                        .insert(shard, registration.to_owned())
                        .is_some()
                {
                    return Err(anyhow!(
                        "rolling parity admission has invalid gate, identity, or counts: {event}"
                    ));
                }
                group.last_successes = successes;
                group.last_completed = completed;
                group.last_active = active_after;
                match registration {
                    "Cached" => {
                        group.parity_cached += 1;
                        derived_parity_cached += 1;
                    }
                    "Joined" => {
                        group.parity_joined += 1;
                        derived_parity_joined += 1;
                    }
                    "Led" => {
                        group.parity_led += 1;
                        derived_parity_led += 1;
                    }
                    _ => unreachable!(),
                }
            }
            "parity-result" => {
                let group = groups.get_mut(&group_id).ok_or_else(|| {
                    anyhow!("rolling parity result references unknown group {group_id}")
                })?;
                if group.terminal {
                    return Err(anyhow!("rolling parity result followed group terminal"));
                }
                let shard = required_u64(event, "shard_index", "parity result")?;
                let offset = required_u64(event, "parity_offset", "parity result")?;
                let registration = group
                    .admissions
                    .get(&shard)
                    .ok_or_else(|| anyhow!("rolling parity result has no matching admission"))?;
                let valid = event
                    .get("valid")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow!("rolling parity result has invalid validity"))?;
                let active_before = required_u64(event, "active_before", "parity result")?;
                let active_after = required_u64(event, "active_after", "parity result")?;
                let successes_before = required_u64(event, "successes_before", "parity result")?;
                let successes_after = required_u64(event, "successes_after", "parity result")?;
                let completed = required_u64(event, "completed", "parity result")?;
                let expected_active_after = if registration == "Cached" {
                    Some(active_before)
                } else {
                    active_before.checked_sub(1)
                };
                let expected_successes_after = successes_before.checked_add(u64::from(valid));
                if offset >= group.parity_count
                    || group.data_count.checked_add(offset) != Some(shard)
                    || !group.results.insert(shard)
                    || expected_active_after != Some(active_after)
                    || expected_successes_after != Some(successes_after)
                    || active_before > group.data_count
                    || active_after > group.data_count
                    || active_before > group.last_active
                    || successes_before < group.last_successes
                    || completed < group.last_completed
                    || completed == 0
                    || successes_after > group.data_count.saturating_add(group.parity_count)
                    || completed
                        > group
                            .miss_count
                            .saturating_add(group.admissions.len() as u64)
                {
                    return Err(anyhow!(
                        "rolling parity result has invalid identity or lifecycle counts: {event}"
                    ));
                }
                group.last_successes = successes_after;
                group.last_completed = completed;
                group.last_active = active_after;
                if valid {
                    group.parity_valid += 1;
                    derived_parity_valid += 1;
                } else {
                    group.parity_invalid += 1;
                    derived_parity_invalid += 1;
                }
            }
            "terminal" => {
                let group = groups.get_mut(&group_id).ok_or_else(|| {
                    anyhow!("rolling terminal references unknown group {group_id}")
                })?;
                if group.terminal {
                    return Err(anyhow!("rolling group has duplicate terminal events"));
                }
                let reason = event
                    .get("reason")
                    .and_then(Value::as_str)
                    .filter(|reason| {
                        matches!(
                            *reason,
                            "direct-all-ready"
                                | "reconstruct-threshold"
                                | "stale"
                                | "channel-closed"
                                | "error"
                        )
                    })
                    .ok_or_else(|| anyhow!("rolling terminal has invalid reason"))?;
                let successes = required_u64(event, "successes", "rolling terminal")?;
                let completed = required_u64(event, "completed", "rolling terminal")?;
                let active = required_u64(event, "active", "rolling terminal")?;
                let admitted = required_u64(event, "parity_admitted", "rolling terminal")?;
                let cached = required_u64(event, "parity_cached", "rolling terminal")?;
                let joined = required_u64(event, "parity_joined", "rolling terminal")?;
                let led = required_u64(event, "parity_led", "rolling terminal")?;
                let valid = required_u64(event, "parity_valid", "rolling terminal")?;
                let invalid = required_u64(event, "parity_invalid", "rolling terminal")?;
                let direct = event
                    .get("direct_completion")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow!("rolling terminal has invalid direct flag"))?;
                let reconstructed = event
                    .get("reconstructed")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow!("rolling terminal has invalid reconstruction flag"))?;
                let close_at = match event.get("close_at_ms") {
                    Some(value) if value.is_null() => None,
                    Some(value) => Some(
                        value
                            .as_f64()
                            .filter(|value| {
                                value.is_finite() && *value >= group.init_at_ms && *value <= at
                            })
                            .ok_or_else(|| anyhow!("rolling terminal has invalid close_at_ms"))?,
                    ),
                    None => {
                        return Err(anyhow!("rolling terminal omitted close_at_ms"));
                    }
                };
                let close_reason = match event.get("close_reason") {
                    Some(value) if value.is_null() => None,
                    Some(value) => Some(
                        value
                            .as_str()
                            .filter(|reason| {
                                matches!(*reason, "direct-all-ready" | "reconstruct-threshold")
                            })
                            .ok_or_else(|| anyhow!("rolling terminal has invalid close_reason"))?,
                    ),
                    None => {
                        return Err(anyhow!("rolling terminal omitted close_reason"));
                    }
                };
                let close_shape_valid = match (close_at, close_reason) {
                    (None, None) => true,
                    (Some(_), Some("direct-all-ready" | "reconstruct-threshold")) => true,
                    _ => false,
                };
                let completion_shape_valid = match reason {
                    "direct-all-ready" => direct && !reconstructed && close_reason == Some(reason),
                    "reconstruct-threshold" => {
                        !direct && reconstructed && close_reason == Some(reason)
                    }
                    _ => !direct && !reconstructed,
                };
                if successes < group.last_successes
                    || completed < group.last_completed
                    || successes > group.data_count.saturating_add(group.parity_count)
                    || active > group.data_count
                    || active > group.last_active
                    || completed
                        > group
                            .miss_count
                            .saturating_add(group.admissions.len() as u64)
                    || admitted != group.admissions.len() as u64
                    || admitted != cached.saturating_add(joined).saturating_add(led)
                    || cached != group.parity_cached
                    || joined != group.parity_joined
                    || led != group.parity_led
                    || valid != group.parity_valid
                    || invalid != group.parity_invalid
                    || valid.saturating_add(invalid) > admitted
                    || !close_shape_valid
                    || !completion_shape_valid
                {
                    return Err(anyhow!(
                        "rolling terminal reason or lifecycle counts are inconsistent: {event}"
                    ));
                }
                group.terminal = true;
                *derived_terminals.entry(reason.to_owned()).or_default() += 1;
            }
            _ => {
                return Err(anyhow!(
                    "rolling-group event {event_index} has unknown type {event_name:?}"
                ));
            }
        }
    }

    let derived_started =
        u64::try_from(groups.len()).map_err(|_| anyhow!("rolling-group count did not fit u64"))?;
    let derived_active = u64::try_from(groups.values().filter(|group| !group.terminal).count())
        .map_err(|_| anyhow!("rolling active-group count did not fit u64"))?;
    let derived_terminal = derived_started.saturating_sub(derived_active);
    let terminal_counter = |reason: &str| derived_terminals.get(reason).copied().unwrap_or(0);
    if derived_started != groups_started
        || derived_dynamic_eligible != groups_dynamic_eligible
        || derived_dynamic_ineligible != groups_dynamic_ineligible
        || derived_active != groups_active
        || derived_terminal != groups_terminal
        || terminal_counter("direct-all-ready")
            != required_u64(trace, "terminal_direct_all_ready", "rolling-group trace")?
        || terminal_counter("reconstruct-threshold")
            != required_u64(
                trace,
                "terminal_reconstruct_threshold",
                "rolling-group trace",
            )?
        || terminal_counter("stale")
            != required_u64(trace, "terminal_stale", "rolling-group trace")?
        || terminal_counter("channel-closed")
            != required_u64(trace, "terminal_channel_closed", "rolling-group trace")?
        || terminal_counter("error")
            != required_u64(trace, "terminal_error", "rolling-group trace")?
        || derived_parity_cached != required_u64(trace, "parity_cached", "rolling-group trace")?
        || derived_parity_joined != required_u64(trace, "parity_joined", "rolling-group trace")?
        || derived_parity_led != required_u64(trace, "parity_led", "rolling-group trace")?
        || derived_parity_valid != required_u64(trace, "parity_valid", "rolling-group trace")?
        || derived_parity_invalid != required_u64(trace, "parity_invalid", "rolling-group trace")?
    {
        return Err(anyhow!(
            "rolling-group aggregate counters disagree with event identities"
        ));
    }
    let captured_prefix_windows = captured_critical_prefix_windows(metrics)?;
    if events.is_empty() || groups_started == 0 || groups_dynamic_eligible == 0 {
        return Err(anyhow!(
            "Beginning startup emitted no rolling-group trace (captured prefix windows: {captured_prefix_windows:?})"
        ));
    }
    Ok(())
}

fn validate_first_reference_terminal_windows(metrics: &Value) -> Result<()> {
    const WINDOW_BYTES: u64 = 512 * 1_024;
    let profile = metrics
        .get("profile")
        .ok_or_else(|| anyhow!("HLS profile did not expose its measurement profile"))?;
    let trace = profile
        .pointer("/service_worker_trace/range_progress")
        .ok_or_else(|| anyhow!("HLS profile did not expose first-reference range progress"))?;
    if trace.get("cap").and_then(Value::as_u64) != Some(256) {
        return Err(anyhow!("first-reference range progress has an invalid cap"));
    }
    let first_reference = trace
        .get("first_reference")
        .and_then(Value::as_str)
        .filter(|reference| {
            (64..=128).contains(&reference.len())
                && reference.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| anyhow!("first-reference range progress has no valid reference"))?;
    let first_playing = profile
        .pointer("/marks/first_playing")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| anyhow!("range progress did not observe first playing"))?;
    if trace.get("first_playing_at_ms").and_then(Value::as_f64) != Some(first_playing) {
        return Err(anyhow!(
            "range progress first-playing timestamp disagrees with the playback mark"
        ));
    }
    let transitions = trace
        .get("transitions")
        .and_then(Value::as_array)
        .filter(|transitions| transitions.len() <= 256)
        .ok_or_else(|| anyhow!("first-reference transitions exceeded their cap"))?;
    let windows = trace
        .get("terminal_windows")
        .and_then(Value::as_array)
        .filter(|windows| windows.len() == 5)
        .ok_or_else(|| anyhow!("range progress did not expose exact W0-W4 terminals"))?;
    let mut max_terminal_at = 0.0_f64;
    for (index, window) in windows.iter().enumerate() {
        let start = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(WINDOW_BYTES))
            .ok_or_else(|| anyhow!("range window start overflowed"))?;
        let end = start
            .checked_add(WINDOW_BYTES - 1)
            .ok_or_else(|| anyhow!("range window end overflowed"))?;
        let expected_horizon = format!("W{index}");
        let expected_range = format!("bytes={start}-{end}");
        let state = window
            .get("state")
            .and_then(Value::as_str)
            .filter(|state| matches!(*state, "done" | "failed"))
            .ok_or_else(|| anyhow!("{expected_horizon} was not terminal"))?;
        let at = required_finite_nonnegative(
            window,
            "at_ms",
            &format!("range terminal {expected_horizon}"),
        )?;
        max_terminal_at = max_terminal_at.max(at);
        if window.get("horizon").and_then(Value::as_str) != Some(expected_horizon.as_str())
            || window.get("range").and_then(Value::as_str) != Some(expected_range.as_str())
            || window
                .get("reference")
                .and_then(Value::as_str)
                .is_none_or(|reference| !reference.eq_ignore_ascii_case(first_reference))
            || !transitions.iter().any(|transition| {
                transition.get("at_ms").and_then(Value::as_f64) == Some(at)
                    && transition.get("range").and_then(Value::as_str)
                        == Some(expected_range.as_str())
                    && transition.get("state").and_then(Value::as_str) == Some(state)
                    && transition
                        .get("reference")
                        .and_then(Value::as_str)
                        .is_some_and(|reference| reference.eq_ignore_ascii_case(first_reference))
            })
        {
            return Err(anyhow!(
                "range terminal {expected_horizon} is not an exact captured transition"
            ));
        }
    }
    let observer = trace
        .get("observer")
        .ok_or_else(|| anyhow!("range progress did not expose observer state"))?;
    let disconnected_at =
        required_finite_nonnegative(observer, "disconnected_at_ms", "range progress observer")?;
    if observer.get("state").and_then(Value::as_str) != Some("disconnected")
        || observer.get("disconnect_reason").and_then(Value::as_str)
            != Some("first-playing-and-w0-w4-terminal")
        || disconnected_at < first_playing.max(max_terminal_at)
    {
        return Err(anyhow!(
            "range observer did not stop after both first playing and exact W0-W4 terminals"
        ));
    }
    Ok(())
}

fn validate_served_identity(metrics: &Value) -> Result<()> {
    let measurement_frozen_at =
        required_finite_nonnegative(metrics, "measurement_frozen_at_ms", "HLS result")?;
    if metrics.get("measured_at_ms").and_then(Value::as_f64) != Some(measurement_frozen_at)
        || metrics
            .pointer("/profile/measurement_frozen_at_ms")
            .and_then(Value::as_f64)
            != Some(measurement_frozen_at)
    {
        return Err(anyhow!(
            "HLS result and frozen profile disagree on measurement time"
        ));
    }
    let identity = metrics
        .get("served_identity")
        .ok_or_else(|| anyhow!("HLS result did not expose post-measurement served identity"))?;
    if identity.get("cache_mode").and_then(Value::as_str) != Some("no-store") {
        return Err(anyhow!("served identity was not fetched with no-store"));
    }
    let identity_started =
        required_finite_nonnegative(identity, "identity_fetch_started_at_ms", "served identity")?;
    let identity_completed = required_finite_nonnegative(
        identity,
        "identity_fetch_completed_at_ms",
        "served identity",
    )?;
    if measurement_frozen_at > identity_started || identity_started > identity_completed {
        return Err(anyhow!(
            "served identity overlapped or predated the frozen measurement"
        ));
    }
    let source_build_id = identity
        .get("source_wasm_build_id")
        .and_then(Value::as_str)
        .filter(|value| is_lower_hex(value, 16))
        .ok_or_else(|| anyhow!("served identity has no valid source/Wasm build id"))?;
    if metrics
        .pointer("/profile/source_wasm_build_id")
        .and_then(Value::as_str)
        != Some(source_build_id)
    {
        return Err(anyhow!(
            "served source/Wasm build id disagrees with the early mounted-interface latch"
        ));
    }

    let scope = metrics
        .pointer("/service_worker/scope")
        .and_then(Value::as_str)
        .filter(|scope| scope.ends_with('/'))
        .ok_or_else(|| anyhow!("served identity has no absolute Service Worker scope"))?;
    let assets = identity
        .get("assets")
        .and_then(Value::as_object)
        .filter(|assets| assets.len() == 4)
        .ok_or_else(|| anyhow!("served identity did not expose exactly four assets"))?;
    let expected_paths = [
        ("index", ""),
        ("service_js", "service.js"),
        ("javascript", "weeb_3.js"),
        ("wasm", "weeb_3_bg.wasm"),
    ];
    let mut common_build_version = None::<&str>;
    for (name, suffix) in expected_paths {
        let asset = assets
            .get(name)
            .and_then(Value::as_object)
            .filter(|asset| asset.len() == 6)
            .ok_or_else(|| {
                anyhow!("served identity asset {name} has fields beyond identity metadata")
            })?;
        for field in [
            "source_url",
            "status",
            "bytes",
            "sha256",
            "build_version",
            "etag",
        ] {
            if !asset.contains_key(field) {
                return Err(anyhow!("served identity asset {name} omitted {field}"));
            }
        }
        let expected_url = format!("{scope}{suffix}");
        let build_version = asset
            .get("build_version")
            .and_then(Value::as_str)
            .filter(|value| is_lower_hex(value, 16))
            .ok_or_else(|| anyhow!("served identity asset {name} has invalid build header"))?;
        match common_build_version {
            Some(expected) if expected != build_version => {
                return Err(anyhow!(
                    "served identity assets came from different embedded builds"
                ));
            }
            None => common_build_version = Some(build_version),
            _ => {}
        }
        if asset.get("source_url").and_then(Value::as_str) != Some(expected_url.as_str())
            || asset.get("status").and_then(Value::as_u64) != Some(200)
            || asset
                .get("bytes")
                .and_then(Value::as_u64)
                .is_none_or(|bytes| bytes == 0)
            || asset
                .get("sha256")
                .and_then(Value::as_str)
                .is_none_or(|hash| !is_lower_hex(hash, 64))
        {
            return Err(anyhow!("served identity asset {name} is incomplete"));
        }
    }
    let build_version = common_build_version.expect("four identity assets have build versions");
    for name in ["index", "service_js"] {
        if !assets
            .get(name)
            .and_then(|asset| asset.get("etag"))
            .is_some_and(Value::is_null)
        {
            return Err(anyhow!(
                "no-store served identity asset {name} unexpectedly had an ETag"
            ));
        }
    }
    let expected_etag = format!("\"{build_version}\"");
    for name in ["javascript", "wasm"] {
        if assets
            .get(name)
            .and_then(|asset| asset.get("etag"))
            .and_then(Value::as_str)
            != Some(expected_etag.as_str())
        {
            return Err(anyhow!(
                "served identity asset {name} had an incoherent ETag"
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawStartupGroupAttribution {
    group_id: u64,
    horizon: u64,
    depth: u64,
    parent_start: u64,
    parent_span: u64,
    requested_first_index: u64,
    requested_last_index: u64,
    requested_count: u64,
    data_count: u64,
    parity_count: u64,
    decoded_raw_count: u64,
    decoded_only_count: u64,
    cache_miss_count: u64,
    full_data_group_candidate: bool,
    full_data_group_eligible: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawStartupChildAttribution {
    group: RawStartupGroupAttribution,
    child_index: u64,
    child_start: u64,
    child_span: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RawStartupFlightSignature {
    horizon: u64,
    scout_child: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug)]
struct RawStartupLedFlight {
    registration_index: usize,
    completion_index: Option<usize>,
    signature: RawStartupFlightSignature,
}

fn raw_startup_child_attribution(
    event: &Value,
    event_index: usize,
) -> Result<Option<RawStartupChildAttribution>> {
    const FIELDS: [&str; 18] = [
        "group_id",
        "group_horizon_index",
        "group_depth",
        "group_parent_start",
        "group_parent_span",
        "requested_first_index",
        "requested_last_index",
        "requested_count",
        "data_count",
        "parity_count",
        "decoded_raw_count",
        "decoded_only_count",
        "cache_miss_count",
        "child_index",
        "child_start",
        "child_span",
        "full_data_group_candidate",
        "full_data_group_eligible",
    ];
    let values = FIELDS
        .iter()
        .map(|field| {
            event
                .get(*field)
                .ok_or_else(|| anyhow!("raw-startup event {event_index} did not expose {field}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if values.iter().all(|value| value.is_null()) {
        return Ok(None);
    }
    if values.iter().any(|value| value.is_null()) {
        return Err(anyhow!(
            "raw-startup event {event_index} has partial group attribution: {event}"
        ));
    }
    let number = |field: &str| -> Result<u64> {
        event
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("raw-startup event {event_index} has invalid {field}"))
    };
    let decimal = |field: &str| -> Result<u64> {
        let text = event
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("raw-startup event {event_index} has invalid {field}"))?;
        let value = text
            .parse::<u64>()
            .map_err(|_| anyhow!("raw-startup event {event_index} has invalid {field}"))?;
        if value.to_string() != text {
            return Err(anyhow!(
                "raw-startup event {event_index} has noncanonical {field}"
            ));
        }
        Ok(value)
    };
    let group = RawStartupGroupAttribution {
        group_id: number("group_id")?,
        horizon: number("group_horizon_index")?,
        depth: number("group_depth")?,
        parent_start: decimal("group_parent_start")?,
        parent_span: decimal("group_parent_span")?,
        requested_first_index: number("requested_first_index")?,
        requested_last_index: number("requested_last_index")?,
        requested_count: number("requested_count")?,
        data_count: number("data_count")?,
        parity_count: number("parity_count")?,
        decoded_raw_count: number("decoded_raw_count")?,
        decoded_only_count: number("decoded_only_count")?,
        cache_miss_count: number("cache_miss_count")?,
        full_data_group_candidate: event
            .get("full_data_group_candidate")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("raw-startup event {event_index} has invalid full_data_group_candidate")
            })?,
        full_data_group_eligible: event
            .get("full_data_group_eligible")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("raw-startup event {event_index} has invalid full_data_group_eligible")
            })?,
    };
    let child_index = number("child_index")?;
    let child_start = decimal("child_start")?;
    let child_span = decimal("child_span")?;
    let expected_candidate = group.requested_first_index == 0
        && group.requested_count == group.data_count
        && group.parity_count > 0;
    let expected_eligible = expected_candidate
        && group.cache_miss_count > 0
        && group.decoded_only_count < group.parity_count;
    let parent_end = group.parent_start.checked_add(group.parent_span);
    let child_end = child_start.checked_add(child_span);
    if group.group_id == 0
        || group.horizon == 0
        || group.parent_span == 0
        || child_span == 0
        || group.requested_count == 0
        || group.data_count == 0
        || group
            .data_count
            .checked_add(group.parity_count)
            .is_none_or(|total| total > 256)
        || group.requested_first_index > group.requested_last_index
        || group.requested_last_index >= group.data_count
        || group.requested_last_index - group.requested_first_index + 1 != group.requested_count
        || group
            .decoded_raw_count
            .checked_add(group.decoded_only_count)
            .and_then(|count| count.checked_add(group.cache_miss_count))
            != Some(group.requested_count)
        || child_index < group.requested_first_index
        || child_index > group.requested_last_index
        || child_start < group.parent_start
        || parent_end.is_none()
        || child_end.is_none()
        || child_end > parent_end
        || group.full_data_group_candidate != expected_candidate
        || group.full_data_group_eligible != expected_eligible
    {
        return Err(anyhow!(
            "raw-startup event {event_index} has inconsistent group attribution: {event}"
        ));
    }
    Ok(Some(RawStartupChildAttribution {
        group,
        child_index,
        child_start,
        child_span,
    }))
}

fn validate_raw_startup_trace(metrics: &Value) -> Result<()> {
    const EXPECTED_CAP: usize = 2_048;
    const EXPECTED_DATA_CAP: usize = EXPECTED_CAP - 1;
    let trace = metrics
        .pointer("/profile/raw_startup_trace")
        .ok_or_else(|| anyhow!("HLS profile did not install raw-startup attribution"))?;
    if trace.get("schema_version").and_then(Value::as_u64) != Some(3)
        || trace.get("cap").and_then(Value::as_u64) != Some(EXPECTED_CAP as u64)
        || trace.get("data_cap").and_then(Value::as_u64) != Some(EXPECTED_DATA_CAP as u64)
    {
        return Err(anyhow!("invalid raw-startup attribution schema: {trace}"));
    }
    let dropped = trace
        .get("dropped")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("raw-startup attribution did not expose its dropped count"))?;
    let optional_reason = |field: &str| -> Result<Option<&str>> {
        match trace.get(field) {
            Some(value) if value.is_null() => Ok(None),
            Some(value) => value
                .as_str()
                .map(Some)
                .ok_or_else(|| anyhow!("raw-startup attribution has invalid {field}: {value}")),
            None => Err(anyhow!("raw-startup attribution did not expose {field}")),
        }
    };
    let collector_terminal_reason = optional_reason("collector_terminal_reason")?;
    let emitter_terminal_reason = optional_reason("emitter_terminal_reason")?;
    let events = trace
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("raw-startup attribution did not expose events"))?;
    if events.len() > EXPECTED_CAP {
        return Err(anyhow!(
            "raw-startup attribution exceeded its cap: {} > {EXPECTED_CAP}",
            events.len()
        ));
    }

    let mut captured_critical_prefix_windows = None::<u64>;
    for value in metrics
        .pointer("/profile/service_worker_trace/hls_requests")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|request| request.get("response"))
        .filter_map(|response| response.get("header_fields"))
        .filter_map(|headers| headers.get("X-Weeb3-HLS-Critical-Prefix-Windows"))
        .filter_map(Value::as_str)
    {
        let windows = value.parse::<u64>().map_err(|_| {
            anyhow!("invalid captured X-Weeb3-HLS-Critical-Prefix-Windows value {value:?}")
        })?;
        captured_critical_prefix_windows =
            Some(captured_critical_prefix_windows.map_or(windows, |current| current.max(windows)));
    }
    if captured_critical_prefix_windows.is_some_and(|windows| windows > 1) && events.is_empty() {
        return Err(anyhow!(
            "HLS advertised a multi-window critical prefix but emitted no raw-startup attribution"
        ));
    }

    let cumulative = [
        "raw_leaders_led",
        "raw_leader_dispatches",
        "raw_leader_completions",
        "logical_retrieve_dispatches",
        "credits_minted",
        "credits_discarded",
    ];
    let mut previous_at = 0.0_f64;
    let mut previous = [0_u64; 6];
    let mut admission_close_index = None;
    let mut terminal = None::<(usize, &str)>;
    let mut scout_groups = BTreeMap::<u64, RawStartupGroupAttribution>::new();
    let mut scout_registrations =
        BTreeMap::<(u64, u64), (RawStartupChildAttribution, String, bool)>::new();
    let mut captured_led_registrations = 0_u64;
    let mut captured_accepted_led_registrations = 0_u64;
    let mut captured_led_completions = 0_u64;
    let mut captured_scout_registrations = 0_u64;
    let mut led_flights = BTreeMap::<u64, RawStartupLedFlight>::new();
    let mut joined_flights = Vec::<(u64, usize)>::new();
    for (index, event) in events.iter().enumerate() {
        let at = event
            .get("at_ms")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("raw-startup event {index} has no timestamp"))?;
        if !at.is_finite() || at < previous_at {
            return Err(anyhow!(
                "raw-startup event {index} has a non-monotonic timestamp"
            ));
        }
        previous_at = at;
        let event_name = event
            .get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("raw-startup event {index} has no event name"))?;
        let registration = event.get("registration").and_then(Value::as_str);
        let terminal_reason = event.get("terminal_reason").and_then(Value::as_str);
        let dispatch_accepted = event.get("dispatch_accepted").and_then(Value::as_bool);
        let canonical_cac = event.get("canonical_cac").and_then(Value::as_bool);
        let raw_flight_id = match event.get("raw_flight_id") {
            Some(value) if value.is_null() => None,
            Some(value) => {
                let text = value.as_str().ok_or_else(|| {
                    anyhow!("raw-startup event {index} has invalid raw_flight_id")
                })?;
                let value = text
                    .parse::<u64>()
                    .map_err(|_| anyhow!("raw-startup event {index} has invalid raw_flight_id"))?;
                if value == 0 || value.to_string() != text {
                    return Err(anyhow!(
                        "raw-startup event {index} has noncanonical raw_flight_id"
                    ));
                }
                Some(value)
            }
            None => {
                return Err(anyhow!(
                    "raw-startup event {index} did not expose raw_flight_id"
                ));
            }
        };
        let valid_shape = match event_name {
            "registration" => match registration {
                Some("Led") => {
                    dispatch_accepted.is_some()
                        && canonical_cac.is_none()
                        && terminal_reason.is_none()
                        && raw_flight_id.is_some()
                }
                Some("Cached") => {
                    dispatch_accepted.is_none()
                        && canonical_cac.is_none()
                        && terminal_reason.is_none()
                        && raw_flight_id.is_none()
                }
                Some("Joined") => {
                    dispatch_accepted.is_none()
                        && canonical_cac.is_none()
                        && terminal_reason.is_none()
                        && raw_flight_id.is_some()
                }
                _ => false,
            },
            "completion" => {
                registration == Some("Led")
                    && dispatch_accepted.is_none()
                    && canonical_cac.is_some()
                    && terminal_reason.is_none()
                    && raw_flight_id.is_some()
            }
            "admission-close" => {
                registration.is_none()
                    && dispatch_accepted.is_none()
                    && canonical_cac.is_none()
                    && terminal_reason.is_none()
                    && raw_flight_id.is_none()
            }
            "trace-terminal" => {
                registration.is_none()
                    && dispatch_accepted.is_none()
                    && canonical_cac.is_none()
                    && raw_flight_id.is_none()
                    && matches!(
                        terminal_reason,
                        Some("admission-closed" | "cap-reached" | "dispatch-failed")
                    )
            }
            _ => false,
        };
        if event.get("schema_version").and_then(Value::as_u64) != Some(3)
            || event.get("layer").and_then(Value::as_str) != Some("raw-singleflight")
            || !valid_shape
            || event
                .get("bee_peer_attempts")
                .is_none_or(|value| !value.is_null())
            || event
                .get("retrieval_permits")
                .is_none_or(|value| !value.is_null())
        {
            return Err(anyhow!("invalid raw-startup event {index}: {event}"));
        }
        match event_name {
            "admission-close" => {
                if admission_close_index.replace(index).is_some()
                    || event.get("admission_open").and_then(Value::as_bool) != Some(false)
                {
                    return Err(anyhow!(
                        "raw-startup event {index} has an invalid admission-close snapshot"
                    ));
                }
            }
            "trace-terminal" => {
                if terminal
                    .replace((index, terminal_reason.unwrap()))
                    .is_some()
                    || index + 1 != events.len()
                {
                    return Err(anyhow!(
                        "raw-startup event {index} is not the unique final terminal snapshot"
                    ));
                }
            }
            _ => {}
        }
        let horizon = event
            .get("horizon_index")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("raw-startup event {index} has no horizon"))?;
        if event.get("horizon").and_then(Value::as_str) != Some(format!("W{horizon}").as_str()) {
            return Err(anyhow!("raw-startup event {index} mislabeled its horizon"));
        }
        let attribution = raw_startup_child_attribution(event, index)?;
        let should_have_attribution =
            matches!(event_name, "registration" | "completion") && horizon > 0;
        if should_have_attribution != attribution.is_some()
            || !matches!(event_name, "registration" | "completion") && horizon != 0
        {
            return Err(anyhow!(
                "raw-startup event {index} has attribution inconsistent with its horizon/type: {event}"
            ));
        }
        let flight_signature = RawStartupFlightSignature {
            horizon,
            scout_child: attribution
                .as_ref()
                .map(|attribution| (attribution.group.group_id, attribution.child_index)),
        };
        if let Some(attribution) = attribution {
            if attribution.group.horizon != horizon {
                return Err(anyhow!(
                    "raw-startup event {index} has mismatched group/event horizons"
                ));
            }
            match scout_groups.entry(attribution.group.group_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(attribution.group.clone());
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if entry.get() != &attribution.group =>
                {
                    return Err(anyhow!(
                        "raw-startup group {} changed immutable metadata",
                        attribution.group.group_id
                    ));
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
            let key = (attribution.group.group_id, attribution.child_index);
            match event_name {
                "registration" => {
                    captured_scout_registrations = captured_scout_registrations.saturating_add(1);
                    let mut observed_group_children = 1_u64;
                    let child_end = attribution
                        .child_start
                        .checked_add(attribution.child_span)
                        .expect("validated raw-startup child end");
                    for ((other_group_id, other_child_index), (other, _, _)) in &scout_registrations
                    {
                        if *other_group_id != attribution.group.group_id {
                            continue;
                        }
                        observed_group_children = observed_group_children.saturating_add(1);
                        let other_end = other
                            .child_start
                            .checked_add(other.child_span)
                            .expect("validated raw-startup child end");
                        let geometry_invalid = (*other_child_index < attribution.child_index
                            && other_end > attribution.child_start)
                            || (*other_child_index > attribution.child_index
                                && child_end > other.child_start);
                        if geometry_invalid {
                            return Err(anyhow!(
                                "raw-startup event {index} has overlapping/reversed group geometry"
                            ));
                        }
                    }
                    if observed_group_children > attribution.group.cache_miss_count {
                        return Err(anyhow!(
                            "raw-startup event {index} registered more group children than cache misses"
                        ));
                    }
                    if scout_registrations
                        .insert(key, (attribution, registration.unwrap().to_owned(), false))
                        .is_some()
                    {
                        return Err(anyhow!(
                            "raw-startup event {index} duplicated scout child registration"
                        ));
                    }
                }
                "completion" => {
                    let Some((registered, kind, completed)) = scout_registrations.get_mut(&key)
                    else {
                        return Err(anyhow!(
                            "raw-startup event {index} completed an unregistered scout child"
                        ));
                    };
                    if kind != "Led" || *completed || registered != &attribution {
                        return Err(anyhow!(
                            "raw-startup event {index} did not match one Led registration"
                        ));
                    }
                    *completed = true;
                }
                _ => unreachable!("only raw child rows carry attribution"),
            }
        }
        if event_name == "registration" && registration == Some("Led") {
            captured_led_registrations = captured_led_registrations.saturating_add(1);
            let flight_id = raw_flight_id.expect("validated Led raw flight ID");
            if led_flights
                .insert(
                    flight_id,
                    RawStartupLedFlight {
                        registration_index: index,
                        completion_index: None,
                        signature: flight_signature,
                    },
                )
                .is_some()
            {
                return Err(anyhow!(
                    "raw-startup event {index} duplicated Led raw flight {flight_id}"
                ));
            }
            if dispatch_accepted == Some(true) {
                captured_accepted_led_registrations =
                    captured_accepted_led_registrations.saturating_add(1);
            }
        } else if event_name == "registration" && registration == Some("Joined") {
            joined_flights.push((
                raw_flight_id.expect("validated Joined raw flight ID"),
                index,
            ));
        } else if event_name == "completion" {
            captured_led_completions = captured_led_completions.saturating_add(1);
            let flight_id = raw_flight_id.expect("validated completion raw flight ID");
            let Some(flight) = led_flights.get_mut(&flight_id) else {
                return Err(anyhow!(
                    "raw-startup event {index} completed unknown raw flight {flight_id}"
                ));
            };
            if flight.registration_index >= index
                || flight.completion_index.replace(index).is_some()
                || flight.signature != flight_signature
            {
                return Err(anyhow!(
                    "raw-startup event {index} mismatched, duplicated, or preceded Led raw flight {flight_id}"
                ));
            }
        }

        for (field_index, field) in cumulative.iter().enumerate() {
            let value = event
                .get(*field)
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("raw-startup event {index} has no {field}"))?;
            if value < previous[field_index] {
                return Err(anyhow!(
                    "raw-startup event {index} regressed cumulative {field}"
                ));
            }
            previous[field_index] = value;
        }

        let counter = |field: &str| {
            event
                .get(field)
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("raw-startup event {index} has no {field}"))
        };
        let led = counter("raw_leaders_led")?;
        let dispatched = counter("raw_leader_dispatches")?;
        let completed = counter("raw_leader_completions")?;
        let active = counter("raw_leaders_active")?;
        let logical = counter("logical_retrieve_dispatches")?;
        let minted = counter("credits_minted")?;
        let available = counter("credits_available")?;
        let held = counter("credits_held")?;
        let discarded = counter("credits_discarded")?;
        let scout_active = counter("scout_active")?;
        if dispatched > led
            || completed > led
            || active != led.saturating_sub(completed)
            || logical != dispatched
            || minted != available.saturating_add(held).saturating_add(discarded)
            || scout_active > active
            || scout_active > held
        {
            return Err(anyhow!(
                "raw-startup event {index} violates lifecycle conservation: {event}"
            ));
        }
        if event_name == "admission-close" && available != 0 {
            return Err(anyhow!(
                "raw-startup admission closed before queued credits were drained: {event}"
            ));
        }
    }

    for (flight_id, joined_index) in joined_flights {
        if let Some(RawStartupLedFlight {
            registration_index,
            completion_index: Some(completion_index),
            ..
        }) = led_flights.get(&flight_id)
            && !(*registration_index < joined_index && joined_index < *completion_index)
        {
            return Err(anyhow!(
                "raw-startup Joined event {joined_index} was not between traced Led registration/completion for flight {flight_id}"
            ));
        }
    }

    if dropped != 0 || collector_terminal_reason.is_some() {
        return Err(anyhow!(
            "raw-startup collector truncated its trace: dropped={dropped}, reason={collector_terminal_reason:?}"
        ));
    }
    if matches!(
        emitter_terminal_reason,
        Some("cap-reached" | "dispatch-failed")
    ) {
        return Err(anyhow!(
            "raw-startup emitter terminated unsuccessfully: {emitter_terminal_reason:?}"
        ));
    }
    if let Some((_, reason @ ("cap-reached" | "dispatch-failed"))) = terminal {
        return Err(anyhow!(
            "raw-startup trace terminated unsuccessfully: {reason}"
        ));
    }
    if !events.is_empty() {
        let Some((terminal_index, "admission-closed")) = terminal else {
            return Err(anyhow!(
                "raw-startup attribution has no successful terminal snapshot"
            ));
        };
        if emitter_terminal_reason != Some("admission-closed") {
            return Err(anyhow!(
                "raw-startup terminal event and emitter status disagree"
            ));
        }
        let Some(close_index) = admission_close_index else {
            return Err(anyhow!(
                "raw-startup attribution has no admission-close snapshot"
            ));
        };
        if close_index >= terminal_index {
            return Err(anyhow!(
                "raw-startup terminal snapshot did not follow admission close"
            ));
        }
        let final_event = &events[terminal_index];
        let final_counter = |field: &str| final_event.get(field).and_then(Value::as_u64);
        if final_counter("raw_leaders_led") != final_counter("raw_leader_completions")
            || final_counter("raw_leaders_active") != Some(0)
            || final_counter("credits_minted") != final_counter("credits_discarded")
            || final_counter("credits_available") != Some(0)
            || final_counter("credits_held") != Some(0)
            || final_counter("scout_active") != Some(0)
        {
            return Err(anyhow!(
                "raw-startup terminal snapshot retained unsettled work: {final_event}"
            ));
        }
        if final_counter("raw_leaders_led") != Some(captured_led_registrations)
            || final_counter("raw_leader_dispatches") != Some(captured_accepted_led_registrations)
            || final_counter("raw_leader_completions") != Some(captured_led_completions)
        {
            return Err(anyhow!(
                "raw-startup captured rows do not match final producer aggregates"
            ));
        }
        if captured_critical_prefix_windows.is_some_and(|windows| windows > 1)
            && captured_scout_registrations == 0
        {
            return Err(anyhow!(
                "HLS advertised a multi-window critical prefix without attributed W1+ scout registrations"
            ));
        }
        if scout_registrations
            .values()
            .any(|(_, registration, completed)| registration == "Led" && !completed)
        {
            return Err(anyhow!(
                "raw-startup terminal snapshot left a Led scout registration unmatched"
            ));
        }
    } else if emitter_terminal_reason.is_some() {
        return Err(anyhow!(
            "raw-startup emitter reported termination without an observable terminal event"
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct HlsMetricSample {
    at_ms: f64,
    current_time_s: f64,
    duration_s: Option<f64>,
    forward_buffer_s: f64,
    paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
struct HlsBufferSummary {
    first_minute: Option<HlsMetricSample>,
    final_sample: Option<HlsMetricSample>,
    early_low_water_s: Option<f64>,
    late_low_water_s: Option<f64>,
    low_water_trend_s: Option<f64>,
    matched_window_ms: Option<f64>,
    timeline_jump_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct HlsPlaybackCheckpoint {
    at_ms: f64,
    current_time_s: Option<f64>,
    media_time_s: Option<f64>,
    duration_s: Option<f64>,
    forward_buffer_s: Option<f64>,
    timeline: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsPlaybackCheckpoints {
    first_playing: Option<HlsPlaybackCheckpoint>,
    first_presented_frame: Option<HlsPlaybackCheckpoint>,
    after_10_seconds: Option<HlsPlaybackCheckpoint>,
    after_30_seconds: Option<HlsPlaybackCheckpoint>,
    after_60_seconds: Option<HlsPlaybackCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct HlsWaitingEpisode {
    waiting_at_ms: f64,
    playing_at_ms: Option<f64>,
    observed_duration_ms: f64,
    waiting_timeline: Option<String>,
    playing_timeline: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsWaitingSummary {
    episodes: Vec<HlsWaitingEpisode>,
    completed_count: usize,
    open_count: usize,
    total_observed_ms: f64,
    max_observed_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsDurationSummary {
    anchor: Option<String>,
    anchor_at_ms: Option<f64>,
    initial_duration_s: Option<f64>,
    final_duration_s: Option<f64>,
    max_duration_s: Option<f64>,
    largest_positive_jump_s: Option<f64>,
    largest_negative_jump_s: Option<f64>,
    durationchange_event_count: usize,
    observation_count: usize,
    observed_change_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct HlsColdStartAttributionSummary {
    first_controller_change_ms: Option<f64>,
    hls_request_count: usize,
    hls_range_request_count: usize,
    hls_response_count: usize,
    first_request_at_ms: Option<f64>,
    first_request_url: Option<String>,
    first_request_range: Option<String>,
    first_request_stream_token: Option<String>,
    first_response_at_ms: Option<f64>,
    first_response_status: Option<u64>,
    first_response_critical_prefix_windows: Option<String>,
    first_range_request_at_ms: Option<f64>,
    first_range: Option<String>,
    first_range_stream_token: Option<String>,
    raw_trace_event_count: usize,
    raw_leader_dispatch_count: Option<u64>,
    raw_leader_completion_count: Option<u64>,
    raw_credit_minted_count: Option<u64>,
    raw_credit_available_final: Option<u64>,
    raw_credit_held_final: Option<u64>,
    raw_credit_discarded_final: Option<u64>,
    raw_scout_active_final: Option<u64>,
}

fn summarize_hls_cold_start_attribution(metrics: &Value) -> HlsColdStartAttributionSummary {
    let requests = metrics
        .pointer("/profile/service_worker_trace/hls_requests")
        .and_then(Value::as_array);
    let first_request = requests.and_then(|requests| requests.first());
    let first_range_request = requests
        .into_iter()
        .flatten()
        .find(|request| request.get("request_kind").and_then(Value::as_str) == Some("range"));
    let first_response = requests
        .into_iter()
        .flatten()
        .find_map(|request| request.get("response"));
    let raw_events = metrics
        .pointer("/profile/raw_startup_trace/events")
        .and_then(Value::as_array);
    let last_raw_event = raw_events.and_then(|events| events.last());
    HlsColdStartAttributionSummary {
        first_controller_change_ms: metrics
            .pointer("/profile/service_worker_trace/controller_changes/0/at_ms")
            .and_then(Value::as_f64),
        hls_request_count: requests.map_or(0, Vec::len),
        hls_range_request_count: requests.map_or(0, |requests| {
            requests
                .iter()
                .filter(|request| {
                    request.get("request_kind").and_then(Value::as_str) == Some("range")
                })
                .count()
        }),
        hls_response_count: requests.map_or(0, |requests| {
            requests
                .iter()
                .filter(|request| request.get("response").is_some_and(Value::is_object))
                .count()
        }),
        first_request_at_ms: first_request
            .and_then(|request| request.get("request_at_ms"))
            .and_then(Value::as_f64),
        first_request_url: first_request
            .and_then(|request| request.get("url"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        first_request_range: first_request
            .and_then(|request| request.get("range"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        first_request_stream_token: first_request
            .and_then(|request| request.get("stream_token"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        first_response_at_ms: first_response
            .and_then(|response| response.get("at_ms"))
            .and_then(Value::as_f64),
        first_response_status: first_response
            .and_then(|response| response.get("status"))
            .and_then(Value::as_u64),
        first_response_critical_prefix_windows: first_response
            .and_then(|response| response.get("header_fields"))
            .and_then(|headers| headers.get("X-Weeb3-HLS-Critical-Prefix-Windows"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        first_range_request_at_ms: first_range_request
            .and_then(|request| request.get("request_at_ms"))
            .and_then(Value::as_f64),
        first_range: first_range_request
            .and_then(|request| request.get("range"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        first_range_stream_token: first_range_request
            .and_then(|request| request.get("stream_token"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        raw_trace_event_count: raw_events.map_or(0, Vec::len),
        raw_leader_dispatch_count: last_raw_event
            .and_then(|event| event.get("raw_leader_dispatches"))
            .and_then(Value::as_u64),
        raw_leader_completion_count: last_raw_event
            .and_then(|event| event.get("raw_leader_completions"))
            .and_then(Value::as_u64),
        raw_credit_minted_count: last_raw_event
            .and_then(|event| event.get("credits_minted"))
            .and_then(Value::as_u64),
        raw_credit_available_final: last_raw_event
            .and_then(|event| event.get("credits_available"))
            .and_then(Value::as_u64),
        raw_credit_held_final: last_raw_event
            .and_then(|event| event.get("credits_held"))
            .and_then(Value::as_u64),
        raw_credit_discarded_final: last_raw_event
            .and_then(|event| event.get("credits_discarded"))
            .and_then(Value::as_u64),
        raw_scout_active_final: last_raw_event
            .and_then(|event| event.get("scout_active"))
            .and_then(Value::as_u64),
    }
}

fn hls_metric_samples(metrics: &Value) -> Vec<HlsMetricSample> {
    metrics
        .pointer("/profile/samples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|sample| {
            Some(HlsMetricSample {
                at_ms: sample.get("at_ms")?.as_f64()?,
                current_time_s: sample.get("current_time_s")?.as_f64()?,
                duration_s: sample.get("duration_s").and_then(Value::as_f64),
                forward_buffer_s: sample.get("forward_buffer_s")?.as_f64()?,
                paused: sample.get("paused")?.as_bool()?,
            })
        })
        .collect()
}

fn hls_playback_checkpoint(entry: &Value) -> Option<HlsPlaybackCheckpoint> {
    Some(HlsPlaybackCheckpoint {
        at_ms: entry.get("at_ms")?.as_f64()?,
        current_time_s: entry.get("current_time_s").and_then(Value::as_f64),
        media_time_s: entry.get("media_time_s").and_then(Value::as_f64),
        duration_s: entry.get("duration_s").and_then(Value::as_f64),
        forward_buffer_s: entry.get("forward_buffer_s").and_then(Value::as_f64),
        timeline: entry
            .get("timeline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn hls_first_event_checkpoint(metrics: &Value, event: &str) -> Option<HlsPlaybackCheckpoint> {
    metrics
        .pointer("/profile/events")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("event").and_then(Value::as_str) == Some(event))
        .and_then(hls_playback_checkpoint)
}

fn hls_nearest_sample_checkpoint(
    metrics: &Value,
    first_playing_ms: f64,
    offset_ms: f64,
) -> Option<HlsPlaybackCheckpoint> {
    let samples = metrics.pointer("/profile/samples")?.as_array()?;
    let target_ms = first_playing_ms + offset_ms;
    let last_sample_ms = samples
        .iter()
        .filter_map(|sample| sample.get("at_ms").and_then(Value::as_f64))
        .max_by(f64::total_cmp)?;
    if target_ms > last_sample_ms {
        return None;
    }
    samples
        .iter()
        .filter(|sample| {
            sample
                .get("at_ms")
                .and_then(Value::as_f64)
                .is_some_and(|at_ms| at_ms >= first_playing_ms)
        })
        .min_by(|left, right| {
            let left_distance = left
                .get("at_ms")
                .and_then(Value::as_f64)
                .map(|at_ms| (at_ms - target_ms).abs())
                .unwrap_or(f64::INFINITY);
            let right_distance = right
                .get("at_ms")
                .and_then(Value::as_f64)
                .map(|at_ms| (at_ms - target_ms).abs())
                .unwrap_or(f64::INFINITY);
            left_distance.total_cmp(&right_distance)
        })
        .and_then(hls_playback_checkpoint)
}

const RETRIEVAL_PROFILE_COUNTER_FIELDS: &[&str] = &[
    "tickets_created",
    "enqueue_accepted",
    "enqueue_rejected",
    "relay_forward_succeeded",
    "relay_forward_failed",
    "queue_dequeued",
    "logical_completed",
    "logical_nonempty",
    "logical_empty",
    "pre_permit_rejected",
    "delivery_succeeded",
    "delivery_failed",
    "permit_wait_started",
    "permit_wait_acquired",
    "permit_wait_aborted",
    "permits_released",
    "physical_dispatched",
    "physical_immediate_completed",
    "physical_timed_out",
    "detached_completed",
    "immediate_result_send_succeeded",
    "immediate_result_send_failed",
    "timeout_result_send_succeeded",
    "timeout_result_send_failed",
];

const RETRIEVAL_PROFILE_SCOPE_COUNTER_FIELDS: &[&str] = &[
    "tickets_created",
    "enqueue_accepted",
    "enqueue_rejected",
    "relay_forward_succeeded",
    "relay_forward_failed",
    "queue_dequeued",
    "logical_completed",
    "permit_acquired",
    "physical_dispatched",
    "physical_immediate_completed",
    "physical_timed_out",
    "detached_completed",
    "immediate_result_send_succeeded",
    "immediate_result_send_failed",
    "timeout_result_send_succeeded",
    "timeout_result_send_failed",
];

const RETRIEVAL_PROFILE_OUTCOME_FIELDS: &[&str] = &[
    "valid_cac",
    "valid_soc",
    "confirmed_empty",
    "invalid_nonempty",
    "channel_closed",
];

const RETRIEVAL_PROFILE_HISTOGRAM_FIELDS: &[&str] = &[
    "queue_to_permit_acquired_ms",
    "queue_to_permit_aborted_ms",
    "queue_to_pre_permit_rejection_ms",
    "permit_wait_acquired_ms",
    "permit_wait_aborted_ms",
    "immediate_attempt_ms",
    "detached_after_timeout_ms",
    "detached_total_attempt_ms",
];

const RETRIEVAL_PROFILE_CONSERVATION_FIELDS: &[&str] = &[
    "accepted_minus_queue_dequeued_forward_failed",
    "logical_dequeued_minus_active_completed",
    "permit_wait_started_minus_current_acquired_aborted",
    "permits_acquired_minus_current_released",
    "physical_dispatched_minus_active_immediate_timed_out",
    "immediate_completed_minus_outcomes",
    "immediate_completed_minus_result_sends",
    "timed_out_minus_detached_outstanding_completed",
    "timed_out_minus_result_sends",
    "detached_completed_minus_outcomes",
    "logical_completed_minus_deliveries",
];

fn retrieval_profile_counter_delta(before: &Value, after: &Value, field: &str) -> Value {
    let before = before.get(field).and_then(Value::as_u64).unwrap_or(0);
    let after = after.get(field).and_then(Value::as_u64).unwrap_or(0);
    Value::from(
        i128::from(after)
            .saturating_sub(i128::from(before))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
    )
}

fn retrieval_profile_histogram_delta(before: &Value, after: &Value, field: &str) -> Value {
    let before = before.get(field).unwrap_or(&Value::Null);
    let after = after.get(field).unwrap_or(&Value::Null);
    let before_buckets = before
        .get("buckets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let after_buckets = after
        .get("buckets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let bucket_count = before_buckets.len().max(after_buckets.len());
    let buckets = (0..bucket_count)
        .map(|index| {
            let before = before_buckets
                .get(index)
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let after = after_buckets
                .get(index)
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            Value::from(
                i128::from(after)
                    .saturating_sub(i128::from(before))
                    .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            )
        })
        .collect::<Vec<_>>();
    json!({
        "count": retrieval_profile_counter_delta(before, after, "count"),
        "sum_ms": after.get("sum_ms").and_then(Value::as_f64).unwrap_or(0.0)
            - before.get("sum_ms").and_then(Value::as_f64).unwrap_or(0.0),
        "buckets": buckets
    })
}

fn retrieval_profile_delta(before: &Value, after: &Value) -> Value {
    let counters = RETRIEVAL_PROFILE_COUNTER_FIELDS
        .iter()
        .map(|field| {
            (
                (*field).to_owned(),
                retrieval_profile_counter_delta(before, after, field),
            )
        })
        .collect::<serde_json_crates_io::Map<String, Value>>();
    let outcomes = ["immediate_outcomes", "detached_outcomes"]
        .into_iter()
        .map(|group| {
            let before_group = before.get(group).unwrap_or(&Value::Null);
            let after_group = after.get(group).unwrap_or(&Value::Null);
            let delta = RETRIEVAL_PROFILE_OUTCOME_FIELDS
                .iter()
                .map(|field| {
                    (
                        (*field).to_owned(),
                        retrieval_profile_counter_delta(before_group, after_group, field),
                    )
                })
                .collect::<serde_json_crates_io::Map<String, Value>>();
            (group.to_owned(), Value::Object(delta))
        })
        .collect::<serde_json_crates_io::Map<String, Value>>();
    let by_scope = ["stream_scoped", "unscoped"]
        .into_iter()
        .map(|scope| {
            let before_scope = before
                .get("by_scope")
                .and_then(|groups| groups.get(scope))
                .unwrap_or(&Value::Null);
            let after_scope = after
                .get("by_scope")
                .and_then(|groups| groups.get(scope))
                .unwrap_or(&Value::Null);
            let delta = RETRIEVAL_PROFILE_SCOPE_COUNTER_FIELDS
                .iter()
                .map(|field| {
                    (
                        (*field).to_owned(),
                        retrieval_profile_counter_delta(before_scope, after_scope, field),
                    )
                })
                .collect::<serde_json_crates_io::Map<String, Value>>();
            (scope.to_owned(), Value::Object(delta))
        })
        .collect::<serde_json_crates_io::Map<String, Value>>();
    let histograms = RETRIEVAL_PROFILE_HISTOGRAM_FIELDS
        .iter()
        .map(|field| {
            (
                (*field).to_owned(),
                retrieval_profile_histogram_delta(before, after, field),
            )
        })
        .collect::<serde_json_crates_io::Map<String, Value>>();
    json!({
        "counters": counters,
        "by_scope": by_scope,
        "outcomes": outcomes,
        "histograms": histograms
    })
}

fn hls_nearest_retrieval_snapshot(
    metrics: &Value,
    first_playing_ms: f64,
    offset_ms: f64,
) -> Option<Value> {
    let samples = metrics.pointer("/profile/samples")?.as_array()?;
    let target_ms = first_playing_ms + offset_ms;
    let last_sample_ms = samples
        .iter()
        .filter_map(|sample| sample.get("at_ms").and_then(Value::as_f64))
        .max_by(f64::total_cmp)?;
    if target_ms > last_sample_ms {
        return None;
    }
    samples
        .iter()
        .filter(|sample| {
            sample
                .get("at_ms")
                .and_then(Value::as_f64)
                .is_some_and(|at_ms| at_ms >= first_playing_ms)
        })
        .min_by(|left, right| {
            let distance = |entry: &Value| {
                entry
                    .get("at_ms")
                    .and_then(Value::as_f64)
                    .map(|at_ms| (at_ms - target_ms).abs())
                    .unwrap_or(f64::INFINITY)
            };
            distance(left).total_cmp(&distance(right))
        })?
        .get("retrieval")
        .filter(|snapshot| !snapshot.is_null())
        .cloned()
}

fn summarize_hls_retrieval_profile(metrics: &Value, first_playing_ms: Option<f64>) -> Value {
    let first_playing = metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .and_then(|events| {
            events
                .iter()
                .find(|entry| entry.get("event").and_then(Value::as_str) == Some("playing"))
        })
        .and_then(|entry| entry.get("retrieval"))
        .filter(|snapshot| !snapshot.is_null())
        .cloned();
    let first_minute = first_playing_ms
        .filter(|value| value.is_finite())
        .and_then(|at_ms| hls_nearest_retrieval_snapshot(metrics, at_ms, 60_000.0));
    let final_snapshot = metrics
        .pointer("/profile/retrieval_profile_final")
        .filter(|snapshot| !snapshot.is_null())
        .cloned()
        .or_else(|| {
            metrics
                .pointer("/profile/samples")
                .and_then(Value::as_array)
                .and_then(|samples| samples.last())
                .and_then(|sample| sample.get("retrieval"))
                .filter(|snapshot| !snapshot.is_null())
                .cloned()
        });
    let post_play_delta = first_playing
        .as_ref()
        .zip(final_snapshot.as_ref())
        .map(|(before, after)| retrieval_profile_delta(before, after));

    json!({
        "first_playing": first_playing,
        "first_minute": first_minute,
        "final": final_snapshot,
        "post_play_delta": post_play_delta
    })
}

fn summarize_hls_playback_checkpoints(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsPlaybackCheckpoints {
    let first_presented_frame = metrics
        .pointer("/profile/first_presented_frame")
        .and_then(hls_playback_checkpoint);
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsPlaybackCheckpoints {
            first_presented_frame,
            ..HlsPlaybackCheckpoints::default()
        };
    };
    HlsPlaybackCheckpoints {
        first_playing: hls_first_event_checkpoint(metrics, "playing"),
        first_presented_frame,
        after_10_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 10_000.0),
        after_30_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 30_000.0),
        after_60_seconds: hls_nearest_sample_checkpoint(metrics, first_playing_ms, 60_000.0),
    }
}

fn summarize_hls_waiting_episodes(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsWaitingSummary {
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsWaitingSummary::default();
    };
    let measured_at_ms = metrics
        .get("measured_at_ms")
        .and_then(Value::as_f64)
        .unwrap_or(first_playing_ms)
        .max(first_playing_ms);
    let mut summary = HlsWaitingSummary::default();
    let mut open: Option<(f64, Option<String>)> = None;
    for event in metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(at_ms) = event.get("at_ms").and_then(Value::as_f64) else {
            continue;
        };
        if at_ms < first_playing_ms {
            continue;
        }
        let timeline = event
            .get("timeline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        match event.get("event").and_then(Value::as_str) {
            Some("waiting") if open.is_none() => open = Some((at_ms, timeline)),
            Some("playing") => {
                if let Some((waiting_at_ms, waiting_timeline)) = open.take() {
                    let observed_duration_ms = (at_ms - waiting_at_ms).max(0.0);
                    summary.episodes.push(HlsWaitingEpisode {
                        waiting_at_ms,
                        playing_at_ms: Some(at_ms),
                        observed_duration_ms,
                        waiting_timeline,
                        playing_timeline: timeline,
                    });
                    summary.completed_count += 1;
                }
            }
            _ => {}
        }
    }
    if let Some((waiting_at_ms, waiting_timeline)) = open {
        summary.episodes.push(HlsWaitingEpisode {
            waiting_at_ms,
            playing_at_ms: None,
            observed_duration_ms: (measured_at_ms - waiting_at_ms).max(0.0),
            waiting_timeline,
            playing_timeline: None,
        });
        summary.open_count = 1;
    }
    summary.total_observed_ms = summary
        .episodes
        .iter()
        .map(|episode| episode.observed_duration_ms)
        .sum();
    summary.max_observed_ms = summary
        .episodes
        .iter()
        .map(|episode| episode.observed_duration_ms)
        .max_by(f64::total_cmp);
    summary
}

fn summarize_hls_duration_changes(
    metrics: &Value,
    first_playing_ms: Option<f64>,
) -> HlsDurationSummary {
    let first_presented_frame = metrics
        .pointer("/profile/first_presented_frame")
        .and_then(hls_playback_checkpoint)
        .filter(|checkpoint| checkpoint.at_ms.is_finite());
    let first_playing = hls_first_event_checkpoint(metrics, "playing")
        .filter(|checkpoint| checkpoint.at_ms.is_finite());
    let fallback_playing = first_playing_ms
        .filter(|value| value.is_finite())
        .map(|at_ms| ("first_playing", at_ms, None));
    let frame_anchor = first_presented_frame.as_ref().map(|checkpoint| {
        (
            "first_presented_frame",
            checkpoint.at_ms,
            checkpoint.duration_s,
        )
    });
    let playing_anchor = first_playing
        .as_ref()
        .map(|checkpoint| ("first_playing", checkpoint.at_ms, checkpoint.duration_s))
        .or(fallback_playing);
    let anchor = match (frame_anchor, playing_anchor) {
        (Some(frame), Some(playing)) if frame.1 <= playing.1 => Some(frame),
        (Some(_), Some(playing)) => Some(playing),
        (Some(frame), None) => Some(frame),
        (None, Some(playing)) => Some(playing),
        (None, None) => None,
    };
    let Some((anchor_name, anchor_at_ms, anchor_duration_s)) = anchor else {
        return HlsDurationSummary::default();
    };

    let mut observations = Vec::new();
    observations.push((anchor_at_ms, anchor_duration_s.unwrap_or(f64::NAN)));
    for entry in metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            metrics
                .pointer("/profile/samples")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
    {
        let Some(at_ms) = entry.get("at_ms").and_then(Value::as_f64) else {
            continue;
        };
        if at_ms < anchor_at_ms {
            continue;
        }
        let duration_s = entry
            .get("duration_s")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        observations.push((at_ms, duration_s));
    }
    if let (Some(at_ms), Some(duration_s)) = (
        metrics.get("measured_at_ms").and_then(Value::as_f64),
        metrics.pointer("/media/duration_s").and_then(Value::as_f64),
    ) {
        if at_ms >= anchor_at_ms {
            observations.push((at_ms, duration_s));
        }
    }

    summarize_hls_duration_observations(
        anchor_name,
        anchor_at_ms,
        hls_event_count(metrics, "durationchange", Some(anchor_at_ms)),
        observations,
    )
}

fn summarize_hls_duration_observations(
    anchor: &str,
    anchor_at_ms: f64,
    durationchange_event_count: usize,
    observations: impl IntoIterator<Item = (f64, f64)>,
) -> HlsDurationSummary {
    let mut observations = observations
        .into_iter()
        .filter(|(at_ms, duration_s)| {
            at_ms.is_finite() && duration_s.is_finite() && *duration_s >= 0.0
        })
        .collect::<Vec<_>>();
    observations.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut summary = HlsDurationSummary {
        anchor: Some(anchor.to_string()),
        anchor_at_ms: Some(anchor_at_ms),
        durationchange_event_count,
        observation_count: observations.len(),
        ..HlsDurationSummary::default()
    };
    let Some((_, initial_duration_s)) = observations.first().copied() else {
        return summary;
    };
    summary.initial_duration_s = Some(initial_duration_s);
    summary.final_duration_s = observations.last().map(|(_, duration_s)| *duration_s);
    summary.max_duration_s = observations
        .iter()
        .map(|(_, duration_s)| *duration_s)
        .max_by(f64::total_cmp);
    for pair in observations.windows(2) {
        let jump_s = pair[1].1 - pair[0].1;
        if jump_s > 0.0 {
            summary.largest_positive_jump_s = Some(
                summary
                    .largest_positive_jump_s
                    .map_or(jump_s, |largest| largest.max(jump_s)),
            );
            summary.observed_change_count += 1;
        } else if jump_s < 0.0 {
            summary.largest_negative_jump_s = Some(
                summary
                    .largest_negative_jump_s
                    .map_or(jump_s, |largest| largest.min(jump_s)),
            );
            summary.observed_change_count += 1;
        }
    }
    summary
}

fn validate_hls_duration_jump(summary: &HlsDurationSummary, limit_s: f64) -> Result<()> {
    if !limit_s.is_finite() || limit_s < 0.0 {
        return Err(anyhow!(
            "WEEB3_HLS_MAX_DURATION_JUMP_SECONDS must be finite and non-negative"
        ));
    }
    if summary.initial_duration_s.is_none() {
        return Err(anyhow!(
            "HLS profile did not report a finite media duration after playback started"
        ));
    }
    let largest_jump_s = summary.largest_positive_jump_s.unwrap_or(0.0).max(
        summary
            .largest_negative_jump_s
            .map(|jump| jump.abs())
            .unwrap_or(0.0),
    );
    if largest_jump_s > limit_s {
        return Err(anyhow!(
            "HLS duration jumped by as much as {largest_jump_s:.6}s, exceeding {limit_s:.6}s"
        ));
    }
    Ok(())
}

fn hls_event_count(metrics: &Value, event: &str, after_ms: Option<f64>) -> usize {
    metrics
        .pointer("/profile/events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("event").and_then(Value::as_str) == Some(event))
        .filter(|entry| {
            after_ms.is_none_or(|after| {
                entry
                    .get("at_ms")
                    .and_then(Value::as_f64)
                    .is_some_and(|at| at >= after)
            })
        })
        .count()
}

fn hls_first_event_value(metrics: &Value, event: &str, field: &str) -> Option<f64> {
    metrics
        .pointer("/profile/events")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("event").and_then(Value::as_str) == Some(event))?
        .get(field)?
        .as_f64()
}

fn summarize_hls_buffer(
    samples: &[HlsMetricSample],
    first_playing_ms: Option<f64>,
    requested_window_ms: f64,
) -> HlsBufferSummary {
    let Some(first_playing_ms) = first_playing_ms.filter(|value| value.is_finite()) else {
        return HlsBufferSummary::default();
    };
    let active = &samples[samples.partition_point(|sample| sample.at_ms < first_playing_ms)..];
    let Some(final_sample) = active.last().copied() else {
        return HlsBufferSummary::default();
    };
    let first_minute_at = first_playing_ms + 60_000.0;
    let first_minute = active
        .iter()
        .find(|sample| sample.at_ms >= first_minute_at)
        .copied();
    let available_ms = (final_sample.at_ms - first_minute_at).max(0.0);
    let window_ms = requested_window_ms.max(0.0).min(available_ms / 2.0);
    let low_water = |start: f64, end: f64| {
        active
            .iter()
            .filter(|sample| sample.at_ms >= start && sample.at_ms <= end)
            .map(|sample| sample.forward_buffer_s)
            .min_by(f64::total_cmp)
    };
    let (early, late, trend, matched_window_ms) = if window_ms >= 500.0 {
        let early = low_water(first_minute_at, first_minute_at + window_ms);
        let late = low_water(final_sample.at_ms - window_ms, final_sample.at_ms);
        (
            early,
            late,
            early.zip(late).map(|(early, late)| late - early),
            Some(window_ms),
        )
    } else {
        (None, None, None, None)
    };
    let timeline_jump_count = active
        .windows(2)
        .filter(|pair| {
            if pair[0].paused || pair[1].paused {
                return false;
            }
            let elapsed_s = (pair[1].at_ms - pair[0].at_ms) / 1000.0;
            let media_s = pair[1].current_time_s - pair[0].current_time_s;
            elapsed_s > 0.0 && (media_s < -0.25 || media_s > elapsed_s + 1.0)
        })
        .count();

    HlsBufferSummary {
        first_minute,
        final_sample: Some(final_sample),
        early_low_water_s: early,
        late_low_water_s: late,
        low_water_trend_s: trend,
        matched_window_ms,
        timeline_jump_count,
    }
}

fn contiguous_forward_buffer(current_time_s: f64, ranges: &[(f64, f64)]) -> f64 {
    ranges
        .iter()
        .find(|(start, end)| current_time_s >= start - 0.05 && current_time_s <= end + 0.05)
        .map(|(_, end)| (end - current_time_s).max(0.0))
        .unwrap_or(0.0)
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .parse::<u64>()
            .with_context(|| format!("{name} must be an unsigned integer")),
        _ => Ok(default),
    }
}

fn env_optional_u64(name: &str) -> Result<Option<u64>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            let parsed = value
                .parse::<u64>()
                .with_context(|| format!("{name} must be an unsigned integer"))?;
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn env_optional_f64(name: &str) -> Result<Option<f64>> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            let parsed = value
                .parse::<f64>()
                .with_context(|| format!("{name} must be a number"))?;
            if !parsed.is_finite() {
                return Err(anyhow!("{name} must be finite"));
            }
            Ok(Some(parsed))
        }
        _ => Ok(None),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        ),
        Err(_) => default,
    }
}

#[cfg(test)]
mod metric_tests {
    use super::*;

    fn script_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source
            .split_once(start)
            .and_then(|(_, tail)| tail.split_once(end))
            .map(|(section, _)| section)
            .unwrap_or_else(|| panic!("missing script section between {start:?} and {end:?}"))
    }

    fn sample(at_ms: f64, current_time_s: f64, forward_buffer_s: f64) -> HlsMetricSample {
        HlsMetricSample {
            at_ms,
            current_time_s,
            duration_s: Some(600.0),
            forward_buffer_s,
            paused: false,
        }
    }

    fn valid_retrieval_profile_snapshot() -> Value {
        let scoped = RETRIEVAL_PROFILE_SCOPE_COUNTER_FIELDS
            .iter()
            .map(|field| ((*field).to_owned(), Value::from(0)))
            .collect::<serde_json_crates_io::Map<String, Value>>();
        let conservation = RETRIEVAL_PROFILE_CONSERVATION_FIELDS
            .iter()
            .map(|field| ((*field).to_owned(), Value::from(0)))
            .collect::<serde_json_crates_io::Map<String, Value>>();
        let mut snapshot = json!({
            "schema_version": 1,
            "enabled": true,
            "activation_at_ms": 1.0,
            "snapshot_at_ms": 2.0,
            "permit_capacity": 4,
            "log2_bucket_upper_bounds_ms":
                (0..32).map(|index| 1_u64 << index).collect::<Vec<_>>(),
            "tickets_created": 0,
            "enqueue_accepted": 0,
            "enqueue_rejected": 0,
            "relay_forward_succeeded": 0,
            "relay_forward_failed": 0,
            "queue_dequeued": 0,
            "logical_completed": 0,
            "permit_wait_acquired": 0,
            "permits_current": 0,
            "permits_high_water": 0,
            "physical_dispatched": 0,
            "physical_immediate_completed": 0,
            "physical_timed_out": 0,
            "detached_completed": 0,
            "immediate_result_send_succeeded": 0,
            "immediate_result_send_failed": 0,
            "timeout_result_send_succeeded": 0,
            "timeout_result_send_failed": 0,
            "by_scope": {
                "stream_scoped": scoped.clone(),
                "unscoped": scoped
            },
            "conservation": conservation
        });
        let snapshot = snapshot
            .as_object_mut()
            .expect("retrieval snapshot fixture object");
        for field in RETRIEVAL_PROFILE_HISTOGRAM_FIELDS {
            snapshot.insert(
                (*field).to_owned(),
                json!({
                    "count": 0,
                    "sum_ms": 0.0,
                    "max_ms": 0.0,
                    "buckets": vec![0_u64; 32]
                }),
            );
        }
        Value::Object(snapshot.clone())
    }

    fn valid_rolling_group_metrics() -> Value {
        const ORIGIN: f64 = 1_700_000_000_000.0;
        json!({
            "profile": {
                "measurement_frozen_at_ms": 1_600.0,
                "performance_time_origin_ms": ORIGIN,
                "marks": { "first_playing": 1_500.0 },
                "retrieval_rolling_group_trace_capture": {
                    "attempted": true,
                    "call_count": 1,
                    "getter_present": true,
                    "reason": "first-playing",
                    "at_ms": 1_500.0,
                    "performance_time_origin_ms": ORIGIN
                },
                "retrieval_rolling_group_trace": {
                    "schema_version": 1,
                    "activation_at_ms": ORIGIN + 100.0,
                    "snapshot_at_ms": ORIGIN + 1_500.0,
                    "cap": 2_048,
                    "events_attempted": 4,
                    "dropped": 0,
                    "truncated": false,
                    "groups_started": 1,
                    "groups_dynamic_eligible": 1,
                    "groups_dynamic_ineligible": 0,
                    "groups_active": 0,
                    "groups_terminal": 1,
                    "terminal_direct_all_ready": 0,
                    "terminal_reconstruct_threshold": 1,
                    "terminal_stale": 0,
                    "terminal_channel_closed": 0,
                    "terminal_error": 0,
                    "parity_admitted": 1,
                    "parity_cached": 0,
                    "parity_joined": 0,
                    "parity_led": 1,
                    "parity_valid": 1,
                    "parity_invalid": 0,
                    "managed_to_ordinary_promotions": 0,
                    "first_managed_to_ordinary_promotion_at_ms": null,
                    "last_managed_to_ordinary_promotion_at_ms": null,
                    "events": [{
                        "event": "init",
                        "group_id": 1,
                        "at_ms": ORIGIN + 200.0,
                        "requested_count": 2,
                        "data_count": 2,
                        "parity_count": 1,
                        "decoded_raw_count": 0,
                        "decoded_only_count": 0,
                        "miss_count": 2,
                        "static_candidate": true,
                        "dynamic_eligible": true,
                        "initial_cached": 1,
                        "initial_joined": 0,
                        "initial_led": 1,
                        "initial_active": 1,
                        "initial_successes": 0
                    }, {
                        "event": "parity-admission",
                        "group_id": 1,
                        "decision_at_ms": ORIGIN + 1_200.0,
                        "gate_elapsed_ms": 1_000,
                        "shard_index": 2,
                        "parity_offset": 0,
                        "registration": "Led",
                        "active_before": 0,
                        "active_after": 1,
                        "successes": 1,
                        "completed": 2
                    }, {
                        "event": "parity-result",
                        "group_id": 1,
                        "at_ms": ORIGIN + 1_250.0,
                        "shard_index": 2,
                        "parity_offset": 0,
                        "valid": true,
                        "active_before": 1,
                        "active_after": 0,
                        "successes_before": 1,
                        "successes_after": 2,
                        "completed": 3
                    }, {
                        "event": "terminal",
                        "group_id": 1,
                        "at_ms": ORIGIN + 1_350.0,
                        "close_at_ms": ORIGIN + 1_300.0,
                        "close_reason": "reconstruct-threshold",
                        "reason": "reconstruct-threshold",
                        "successes": 2,
                        "completed": 3,
                        "active": 0,
                        "parity_admitted": 1,
                        "parity_cached": 0,
                        "parity_joined": 0,
                        "parity_led": 1,
                        "parity_valid": 1,
                        "parity_invalid": 0,
                        "direct_completion": false,
                        "reconstructed": true
                    }]
                },
                "service_worker_trace": { "hls_requests": [{
                    "response": { "header_fields": {
                        "X-Weeb3-HLS-Critical-Prefix-Windows": "5"
                    }}
                }]}
            }
        })
    }

    fn empty_rolling_group_metrics(prefix_windows: &str) -> Value {
        const ORIGIN: f64 = 1_700_000_000_000.0;
        json!({
            "profile": {
                "measurement_frozen_at_ms": 1_600.0,
                "performance_time_origin_ms": ORIGIN,
                "marks": { "first_playing": 1_500.0 },
                "retrieval_rolling_group_trace_capture": {
                    "attempted": true,
                    "call_count": 1,
                    "getter_present": true,
                    "reason": "first-playing",
                    "at_ms": 1_500.0,
                    "performance_time_origin_ms": ORIGIN
                },
                "retrieval_rolling_group_trace": {
                    "schema_version": 1,
                    "activation_at_ms": ORIGIN + 100.0,
                    "snapshot_at_ms": ORIGIN + 1_500.0,
                    "cap": 2_048,
                    "events_attempted": 0,
                    "dropped": 0,
                    "truncated": false,
                    "groups_started": 0,
                    "groups_dynamic_eligible": 0,
                    "groups_dynamic_ineligible": 0,
                    "groups_active": 0,
                    "groups_terminal": 0,
                    "terminal_direct_all_ready": 0,
                    "terminal_reconstruct_threshold": 0,
                    "terminal_stale": 0,
                    "terminal_channel_closed": 0,
                    "terminal_error": 0,
                    "parity_admitted": 0,
                    "parity_cached": 0,
                    "parity_joined": 0,
                    "parity_led": 0,
                    "parity_valid": 0,
                    "parity_invalid": 0,
                    "managed_to_ordinary_promotions": 0,
                    "first_managed_to_ordinary_promotion_at_ms": null,
                    "last_managed_to_ordinary_promotion_at_ms": null,
                    "events": []
                },
                "service_worker_trace": { "hls_requests": [{
                    "response": { "header_fields": {
                        "X-Weeb3-HLS-Critical-Prefix-Windows": prefix_windows
                    }}
                }]}
            }
        })
    }

    fn valid_first_reference_metrics() -> Value {
        let reference = "ab".repeat(32);
        let transitions = (0..5)
            .map(|index| {
                let start = index * 512 * 1_024;
                let range = format!("bytes={start}-{}", start + 512 * 1_024 - 1);
                json!({
                    "at_ms": 100.0 + index as f64,
                    "reference": reference,
                    "range": range,
                    "state": "done"
                })
            })
            .collect::<Vec<_>>();
        let terminal_windows = (0..5)
            .map(|index| {
                let start = index * 512 * 1_024;
                let range = format!("bytes={start}-{}", start + 512 * 1_024 - 1);
                json!({
                    "horizon": format!("W{index}"),
                    "range": range,
                    "reference": reference,
                    "state": "done",
                    "at_ms": 100.0 + index as f64,
                    "text": "terminal"
                })
            })
            .collect::<Vec<_>>();
        json!({
            "profile": {
                "marks": { "first_playing": 102.0 },
                "service_worker_trace": { "range_progress": {
                    "cap": 256,
                    "first_reference": reference,
                    "first_playing_at_ms": 102.0,
                    "transitions": transitions,
                    "terminal_windows": terminal_windows,
                    "observer": {
                        "state": "disconnected",
                        "disconnected_at_ms": 104.0,
                        "disconnect_reason": "first-playing-and-w0-w4-terminal"
                    }
                }}
            }
        })
    }

    fn valid_served_identity_metrics() -> Value {
        let asset_version = "1234567890abcdef";
        let hash = "ab".repeat(32);
        let asset = |source_url: &str, etag: Option<String>| {
            json!({
                "source_url": source_url,
                "status": 200,
                "bytes": 128,
                "sha256": hash,
                "build_version": asset_version,
                "etag": etag
            })
        };
        json!({
            "measured_at_ms": 120_000.0,
            "measurement_frozen_at_ms": 120_000.0,
            "profile": {
                "measurement_frozen_at_ms": 120_000.0,
                "source_wasm_build_id": "fedcba0987654321"
            },
            "service_worker": { "scope": "https://host/weeb-3/" },
            "served_identity": {
                "cache_mode": "no-store",
                "identity_fetch_started_at_ms": 120_001.0,
                "identity_fetch_completed_at_ms": 120_010.0,
                "source_wasm_build_id": "fedcba0987654321",
                "assets": {
                    "index": asset("https://host/weeb-3/", None),
                    "service_js": asset("https://host/weeb-3/service.js", None),
                    "javascript": asset(
                        "https://host/weeb-3/weeb_3.js",
                        Some(format!("\"{asset_version}\""))
                    ),
                    "wasm": asset(
                        "https://host/weeb-3/weeb_3_bg.wasm",
                        Some(format!("\"{asset_version}\""))
                    )
                }
            }
        })
    }

    #[test]
    fn document_start_trace_observes_hls_ranges_without_consuming_app_messages() {
        assert!(
            HLS_PROFILE_SCRIPT
                .contains("navigator.serviceWorker.addEventListener('controllerchange'")
        );
        assert!(HLS_PROFILE_SCRIPT.contains("navigator.serviceWorker.addEventListener('message'"));
        assert!(HLS_PROFILE_SCRIPT.contains("data?.type !== 'WEEB3_FETCH_REQUEST'"));
        assert!(HLS_PROFILE_SCRIPT.contains("/^bytes=[0-9]+-[0-9]+$/"));
        assert!(HLS_PROFILE_SCRIPT.contains("/\\/hls\\/bytes\\/"));
        assert!(HLS_PROFILE_SCRIPT.contains("request_kind: range ? 'range' : 'stream-open'"));
        assert!(HLS_PROFILE_SCRIPT.contains("stream_token: typeof data.streamToken"));
        assert!(HLS_PROFILE_SCRIPT.contains("'X-Weeb3-HLS-Critical-Prefix-Windows'"));

        let observation = script_section(
            HLS_PROFILE_SCRIPT,
            "const captureHlsRangeResponse =",
            "const bufferAhead =",
        );
        assert!(!observation.contains("preventDefault"));
        assert!(!observation.contains("stopPropagation"));
        assert!(!observation.contains("stopImmediatePropagation"));
        assert!(!observation.contains("port.start("));
        assert!(!observation.contains("port.close("));
        assert!(observation.contains("const originalPostMessage = port.postMessage;"));
        assert!(observation.contains("delete port.postMessage;"));
        let forwarding = script_section(
            observation,
            "const forwardingPostMessage = function(...args) {",
            "try {\n            Object.defineProperty(port, 'postMessage'",
        );
        let restore = forwarding
            .find("restore();")
            .expect("the one-shot wrapper must restore the port first");
        let observe = forwarding
            .find("entry.response = {")
            .expect("the response must be reduced to serializable attribution");
        let forward = forwarding
            .find("const forwarded = Reflect.apply(originalPostMessage, this, args);")
            .expect("the original receiver and argument list must be forwarded");
        let frozen = forwarding
            .find("if (profile.measurement_frozen_at_ms !== null) return forwarded;")
            .expect("an armed wrapper must become inert after measurement freeze");
        let timestamp = forwarding
            .find("const at = performance.now();")
            .expect("active response capture timestamp");
        assert!(restore < forward && forward < frozen && frozen < timestamp && timestamp < observe);
        assert!(forwarding.contains("return forwarded;"));
    }

    #[test]
    fn range_progress_observer_is_read_only_and_filters_to_the_first_hls_reference() {
        let observation = script_section(
            HLS_PROFILE_SCRIPT,
            "const rangeProgressTrace =",
            "const bufferAhead =",
        );
        assert!(observation.contains("rangeProgressContainer.textContent || ''"));
        assert!(
            observation
                .contains("/^hls-segment-range\\s+([a-fA-F0-9]{64,128})\\s+(bytes=[0-9]+-[0-9]+)")
        );
        assert!(observation.contains("rangeProgressTrace.first_reference = match[1];"));
        assert!(observation.contains("if (referenceKey !== firstRangeReferenceKey) continue;"));
        for field in [
            "at_ms: at",
            "reference: match[1]",
            "range: match[2]",
            "state: match[3]",
            "text",
        ] {
            assert!(
                observation.contains(field),
                "missing progress field {field}"
            );
        }
        assert!(observation.contains("rangeProgressObserver.observe(progressRows, {"));
        assert!(observation.contains("childList: true, subtree: true, characterData: true"));
        assert!(observation.contains("rangeProgressDiscoveryObserver.observe(document, {"));
        assert!(!observation.contains("preventDefault"));
        assert!(!observation.contains("stopPropagation"));
        assert!(!observation.contains("stopImmediatePropagation"));
        assert!(!observation.contains(".setAttribute("));
        assert!(!observation.contains(".remove("));
        assert!(!observation.contains(".append("));
        assert!(!observation.contains(".appendChild("));
        assert!(!observation.contains(".replaceChildren("));
        assert!(!observation.contains(".innerHTML ="));
        assert!(!observation.contains(".textContent ="));
        assert!(HLS_PROFILE_RESULT_SCRIPT.contains("profile: frozenProfile"));
    }

    #[test]
    fn range_progress_trace_is_strictly_capped() {
        assert!(HLS_PROFILE_SCRIPT.contains("const RANGE_PROGRESS_TRACE_CAP = 256;"));
        assert!(HLS_PROFILE_SCRIPT.contains("cap: RANGE_PROGRESS_TRACE_CAP"));
        let scan = script_section(
            HLS_PROFILE_SCRIPT,
            "const scanRangeProgressRows =",
            "const attachRangeProgressObserver =",
        );
        assert_eq!(
            scan.matches("rangeProgressTrace.transitions.length >= RANGE_PROGRESS_TRACE_CAP")
                .count(),
            2
        );
        let push = scan
            .find("rangeProgressTrace.transitions.push({")
            .expect("range progress transitions must be recorded");
        let checks = scan
            .match_indices("disconnectRangeProgressObservers('cap', at);")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), 2);
        assert!(checks[0] < push && push < checks[1]);
    }

    #[test]
    fn range_progress_observers_require_first_playing_and_exact_w0_w4_terminals() {
        let observation = script_section(
            HLS_PROFILE_SCRIPT,
            "const rangeProgressTrace =",
            "const bufferAhead =",
        );
        assert!(observation.contains("rangeProgressObserver?.disconnect();"));
        assert!(observation.contains("rangeProgressDiscoveryObserver?.disconnect();"));
        assert!(observation.contains("rangeProgressTrace.observer.state = 'disconnected';"));
        assert!(observation.contains("rangeProgressTrace.observer.disconnect_reason = reason;"));
        assert!(observation.contains("disconnectRangeProgressObservers('cap', at);"));

        assert!(observation.contains("window.state === 'done' || window.state === 'failed'"));
        assert!(observation.contains("'first-playing-and-w0-w4-terminal', at"));

        let frame = script_section(
            HLS_PROFILE_SCRIPT,
            "media.requestVideoFrameCallback((now, metadata) => {",
            "mark('first_presented_frame', now);",
        );
        assert!(frame.contains("scanRangeProgressRows();"));
        assert!(!frame.contains("disconnectRangeProgressObservers("));

        let playing = script_section(
            HLS_PROFILE_SCRIPT,
            "if (name === 'playing' && profile.marks.first_playing === undefined) {",
            "snapshot(name, at);",
        );
        assert!(playing.contains("rangeProgressTrace.first_playing_at_ms = at;"));
        assert!(playing.contains("scanRangeProgressRows();"));
        assert!(playing.contains("maybeDisconnectRangeProgressObservers(at);"));
        assert!(HLS_PROFILE_SCRIPT.contains("disconnectRangeProgressObservers('deadline');"));
    }

    #[test]
    fn raw_startup_trace_is_explicitly_enabled_typed_and_strictly_capped() {
        assert!(HLS_PROFILE_SCRIPT.contains("window.__weeb3HlsRawStartupProfileEnabled = true;"));
        assert!(HLS_PROFILE_SCRIPT.contains("const RAW_STARTUP_TRACE_CAP = 2048;"));
        assert!(
            HLS_PROFILE_SCRIPT
                .contains("const RAW_STARTUP_TRACE_DATA_CAP = RAW_STARTUP_TRACE_CAP - 1;")
        );
        assert!(
            HLS_PROFILE_SCRIPT
                .contains("const RAW_STARTUP_PROFILE_EVENT = 'weeb3-hls-raw-startup-profile';")
        );
        let listener = script_section(
            HLS_PROFILE_SCRIPT,
            "addEventListener(RAW_STARTUP_PROFILE_EVENT, event => {",
            "const coldStartProbes = []",
        );
        let listener_start = HLS_PROFILE_SCRIPT
            .find("addEventListener(RAW_STARTUP_PROFILE_EVENT, event => {")
            .expect("raw startup listener");
        let activation = HLS_PROFILE_SCRIPT
            .find("window.__weeb3HlsRawStartupProfileEnabled = true;")
            .expect("raw startup activation");
        assert!(
            listener_start < activation,
            "listener must precede activation"
        );
        assert!(listener.contains("rawStartupTrace.events.length >= RAW_STARTUP_TRACE_CAP"));
        assert!(listener.contains("rawStartupTrace.events.length >= RAW_STARTUP_TRACE_DATA_CAP"));
        assert!(listener.contains("'trace-terminal'"));
        assert!(listener.contains("'cap-reached'"));
        assert!(listener.contains("'dispatch-failed'"));
        assert!(listener.contains("rawStartupTrace.collector_terminal_reason = 'cap-reached';"));
        assert!(listener.contains("detail.layer !== 'raw-singleflight'"));
        assert!(listener.contains("detail.schema_version !== 3"));
        assert!(listener.contains("!['Cached', 'Joined', 'Led'].includes(registration)"));
        assert!(listener.contains("raw_flight_id: rawFlightId"));
        assert!(listener.contains("BigInt(detail.raw_flight_id) > 0n"));
        assert!(listener.contains("BigInt(detail.raw_flight_id) <= 18446744073709551615n"));
        for field in [
            "group_id",
            "group_horizon_index",
            "group_depth",
            "group_parent_start",
            "group_parent_span",
            "requested_first_index",
            "requested_last_index",
            "requested_count",
            "data_count",
            "parity_count",
            "decoded_raw_count",
            "decoded_only_count",
            "cache_miss_count",
            "child_index",
            "child_start",
            "child_span",
            "full_data_group_candidate",
            "full_data_group_eligible",
        ] {
            assert!(listener.contains(field), "collector omitted {field}");
        }
        assert!(listener.contains("/^(0|[1-9]\\d*)$/.test(value)"));
        assert!(listener.contains("fullDataGroupEligible !== expectedEligible"));
        assert!(listener.contains("bee_peer_attempts: null"));
        assert!(listener.contains("retrieval_permits: null"));
        assert!(listener.contains("priced_peers: Number(document.getElementById('connections')"));
        assert!(
            listener.contains("ongoing_connections: Number(document.getElementById('ongoing')")
        );
        assert!(listener.contains("rawStartupTrace.events.push(row);"));
        assert!(listener.contains("rawStartupTrace.latest = row;"));
        assert!(listener.contains("rawStartupTrace.admission_close = row;"));
        assert!(listener.contains("rawStartupTrace.terminal = row;"));
    }

    #[test]
    fn fifty_ms_pre_frame_samples_correlate_raw_counters_and_w0_through_w4() {
        assert!(HLS_PROFILE_SCRIPT.contains("const PRE_FRAME_SAMPLE_INTERVAL_MS = 50;"));
        assert!(HLS_PROFILE_SCRIPT.contains("const STEADY_SAMPLE_INTERVAL_MS = 500;"));
        assert!(HLS_PROFILE_SCRIPT.contains("pre_frame_interval_ms: 50"));
        assert!(HLS_PROFILE_SCRIPT.contains(
            "scheduleProfileSample(profile.first_presented_frame\n                ? STEADY_SAMPLE_INTERVAL_MS : PRE_FRAME_SAMPLE_INTERVAL_MS);"
        ));
        assert!(HLS_PROFILE_SCRIPT.contains(
            "profile.first_presented_frame = { ...row };\n                beginSteadySampling();"
        ));
        assert!(
            HLS_PROFILE_SCRIPT.contains("scheduleProfileSample(PRE_FRAME_SAMPLE_INTERVAL_MS);")
        );
        let windows = script_section(
            HLS_PROFILE_SCRIPT,
            "const firstSegmentWindowStates =",
            "const bufferAhead =",
        );
        assert!(windows.contains("Array.from({ length: 5 }"));
        assert!(windows.contains("horizon: `W${index}`"));
        assert!(windows.contains("start % HLS_STORAGE_WINDOW_BYTES !== 0"));
        let snapshot = script_section(
            HLS_PROFILE_SCRIPT,
            "const snapshot =",
            "const PRE_FRAME_SAMPLE_INTERVAL_MS =",
        );
        assert!(snapshot.contains("first_segment_windows: firstSegmentWindowStates(progress)"));
        assert!(snapshot.contains("raw_startup: rawStartupTrace.latest"));
        assert!(!HLS_PROFILE_SCRIPT.contains("setInterval(() => snapshot('sample'), 500)"));
    }

    #[test]
    fn retrieval_profile_is_enabled_before_app_and_sampled_without_dom_events() {
        let activation = HLS_PROFILE_SCRIPT
            .find("window.__weeb3HlsRetrieveProfileEnabled = true;")
            .expect("retrieval profile activation");
        let profile = HLS_PROFILE_SCRIPT
            .find("const profile = window.__weeb3HlsProfile")
            .expect("profile construction");
        assert!(activation < profile);
        assert!(
            HLS_PROFILE_SCRIPT
                .contains("const getter = window.__weeb3GetHlsRetrieveProfileSnapshot;")
        );
        let snapshot = script_section(
            HLS_PROFILE_SCRIPT,
            "const snapshot =",
            "const PRE_FRAME_SAMPLE_INTERVAL_MS =",
        );
        assert!(snapshot.contains("retrieval: retrievalProfileSnapshot()"));
        assert!(HLS_PROFILE_RESULT_SCRIPT.contains("profile.retrieval_profile_final ="));
        assert!(!HLS_PROFILE_SCRIPT.contains("weeb3-hls-retrieval-attempt"));
    }

    #[test]
    fn rolling_group_trace_is_one_shot_at_first_playing_with_deadline_only_fallback() {
        let activation = HLS_PROFILE_SCRIPT
            .find("window.__weeb3HlsRetrieveProfileEnabled = true;")
            .expect("retrieval profile activation");
        let profile = HLS_PROFILE_SCRIPT
            .find("const profile = window.__weeb3HlsProfile")
            .expect("profile construction");
        assert!(activation < profile);
        let finalizer = script_section(
            HLS_PROFILE_SCRIPT,
            "const finalizeRollingGroupTrace =",
            "const captureSourceWasmBuildId =",
        );
        assert!(finalizer.contains("window.__weeb3FinalizeHlsRetrieveRollingGroupTrace"));
        assert!(finalizer.contains("if (capture.attempted) return"));
        assert!(finalizer.contains("capture.call_count++;"));
        assert_eq!(finalizer.matches("finalizer();").count(), 1);

        let snapshot = script_section(
            HLS_PROFILE_SCRIPT,
            "const snapshot =",
            "const PRE_FRAME_SAMPLE_INTERVAL_MS =",
        );
        assert!(!snapshot.contains("retrieval_rolling_group_trace"));
        assert!(!snapshot.contains("FinalizeHlsRetrieveRollingGroupTrace"));

        let playing = script_section(
            HLS_PROFILE_SCRIPT,
            "if (name === 'playing' && profile.marks.first_playing === undefined) {",
            "snapshot(name, at);",
        );
        assert_eq!(
            playing
                .matches("finalizeRollingGroupTrace('first-playing', at);")
                .count(),
            1
        );
        let deadline = script_section(
            HLS_PROFILE_SCRIPT,
            "window.__weeb3FinalizeHlsProfileDeadline = () => {",
            "scheduleProfileSample(PRE_FRAME_SAMPLE_INTERVAL_MS);",
        );
        assert!(deadline.contains(
            "!profile.retrieval_rolling_group_trace_capture.attempted &&\n            profile.marks.first_playing === undefined"
        ));
        assert_eq!(
            deadline
                .matches("finalizeRollingGroupTrace('deadline');")
                .count(),
            1
        );
        assert!(HLS_PROFILE_SCRIPT.contains("performance_time_origin_ms: performance.timeOrigin"));
    }

    #[test]
    fn result_freezes_measurement_before_final_retrieval_and_no_store_identity_probes() {
        let deadline = HLS_PROFILE_RESULT_SCRIPT
            .find("window.__weeb3FinalizeHlsProfileDeadline();")
            .expect("deadline finalizer");
        let freeze = HLS_PROFILE_RESULT_SCRIPT
            .find("window.__weeb3FreezeHlsProfileMeasurement();")
            .expect("measurement freeze");
        let retrieval = HLS_PROFILE_RESULT_SCRIPT
            .find("profile.retrieval_profile_final =")
            .expect("final retrieval snapshot");
        let cold_ready = HLS_PROFILE_RESULT_SCRIPT
            .find("await window.__weeb3ColdStartReady;")
            .expect("cold-start probe join");
        let identity_start = HLS_PROFILE_RESULT_SCRIPT
            .find("const identityFetchStartedAtMs = performance.now();")
            .expect("identity start");
        let fetch = HLS_PROFILE_RESULT_SCRIPT
            .find("await fetch(requestedUrl, { cache: 'no-store' });")
            .expect("no-store identity fetch");
        assert!(deadline < freeze && freeze < retrieval && retrieval < cold_ready);
        assert!(cold_ready < identity_start && identity_start < fetch);
        assert!(HLS_PROFILE_RESULT_SCRIPT.contains(
            "const scopeBase = registration?.scope || new URL('./', location.href).href;"
        ));
        for asset in ["service.js", "weeb_3.js", "weeb_3_bg.wasm"] {
            assert!(HLS_PROFILE_RESULT_SCRIPT.contains(asset));
        }
        let probe = script_section(
            HLS_PROFILE_RESULT_SCRIPT,
            "const probeAsset =",
            "const scopeBase =",
        );
        for field in [
            "source_url:",
            "status:",
            "bytes:",
            "sha256:",
            "build_version:",
            "etag:",
        ] {
            assert!(probe.contains(field), "missing identity field {field}");
        }
        assert!(!probe.contains("body:"));
        assert!(!probe.contains("text:"));
        assert!(HLS_PROFILE_RESULT_SCRIPT.contains("profile: frozenProfile"));
        assert!(
            HLS_PROFILE_RESULT_SCRIPT.contains("measurement_frozen_at_ms: measurementFrozenAtMs")
        );
    }

    #[test]
    fn measurement_freeze_stops_sampling_and_early_latches_source_wasm_build_id() {
        let scheduler = script_section(
            HLS_PROFILE_SCRIPT,
            "const scheduleProfileSample =",
            "const beginSteadySampling =",
        );
        assert!(scheduler.contains("if (profile.measurement_frozen_at_ms !== null)"));
        assert!(scheduler.contains("sampleTimer = null;\n                return;"));
        let freeze = script_section(
            HLS_PROFILE_SCRIPT,
            "window.__weeb3FreezeHlsProfileMeasurement = () => {",
            "window.__weeb3FinalizeHlsProfileDeadline = () => {",
        );
        assert!(freeze.contains("profile.measurement_frozen_at_ms = performance.now();"));
        assert!(freeze.contains("clearTimeout(sampleTimer);"));
        assert!(HLS_PROFILE_SCRIPT.contains("/\\bInterface mounted, version ([0-9a-f]{16})\\b/"));
        let snapshot = script_section(
            HLS_PROFILE_SCRIPT,
            "const snapshot =",
            "const PRE_FRAME_SAMPLE_INTERVAL_MS =",
        );
        assert!(snapshot.contains("captureSourceWasmBuildId();"));
        assert!(
            HLS_PROFILE_RESULT_SCRIPT
                .contains("profile?.source_wasm_build_id || sourceBuildMatch?.[1] || null")
        );
        let discovery = script_section(
            HLS_PROFILE_SCRIPT,
            "const discoverMedia = () => {",
            "for (const name of [",
        );
        let frozen = discovery
            .find("if (profile.measurement_frozen_at_ms !== null) return;")
            .expect("media discovery freeze guard");
        let query = discovery
            .find("document.querySelector('video')")
            .expect("media discovery query");
        let arm = discovery
            .find("armFirstPresentedFrame(media);")
            .expect("media frame callback arming");
        assert!(frozen < query && query < arm);
    }

    #[test]
    fn retrieval_summary_keeps_phase_snapshots_and_post_play_scope_deltas() {
        let before = json!({
            "enqueue_accepted": 2,
            "physical_dispatched": 3,
            "by_scope": {
                "stream_scoped": { "physical_dispatched": 2 },
                "unscoped": { "physical_dispatched": 1 }
            },
            "immediate_outcomes": { "valid_cac": 1 },
            "queue_to_permit_acquired_ms": {
                "count": 2, "sum_ms": 5.0, "buckets": [1, 1]
            }
        });
        let after = json!({
            "enqueue_accepted": 7,
            "physical_dispatched": 11,
            "by_scope": {
                "stream_scoped": { "physical_dispatched": 9 },
                "unscoped": { "physical_dispatched": 2 }
            },
            "immediate_outcomes": { "valid_cac": 5 },
            "queue_to_permit_acquired_ms": {
                "count": 7, "sum_ms": 19.0, "buckets": [2, 5]
            }
        });
        let metrics = json!({
            "profile": {
                "events": [{
                    "event": "playing", "at_ms": 1_000.0, "retrieval": before
                }],
                "samples": [{
                    "event": "sample", "at_ms": 61_000.0, "retrieval": after
                }],
                "retrieval_profile_final": after
            }
        });
        let summary = summarize_hls_retrieval_profile(&metrics, Some(1_000.0));
        assert_eq!(
            summary.pointer("/first_minute/physical_dispatched"),
            Some(&Value::from(11))
        );
        assert_eq!(
            summary.pointer("/post_play_delta/counters/physical_dispatched"),
            Some(&Value::from(8))
        );
        assert_eq!(
            summary.pointer("/post_play_delta/by_scope/stream_scoped/physical_dispatched"),
            Some(&Value::from(7))
        );
        assert_eq!(
            summary.pointer("/post_play_delta/outcomes/immediate_outcomes/valid_cac"),
            Some(&Value::from(4))
        );
        assert_eq!(
            summary.pointer("/post_play_delta/histograms/queue_to_permit_acquired_ms/count"),
            Some(&Value::from(5))
        );
        assert_eq!(
            summary.pointer("/post_play_delta/histograms/queue_to_permit_acquired_ms/buckets/1"),
            Some(&Value::from(4))
        );
    }

    #[test]
    fn retrieval_validator_requires_final_schema_capacity_scope_and_conservation() {
        let valid_snapshot = valid_retrieval_profile_snapshot();
        let valid = json!({
            "profile": { "retrieval_profile_final": valid_snapshot }
        });
        validate_retrieval_profile(&valid).expect("valid retrieval profile");

        let missing = json!({ "profile": { "retrieval_profile_final": null } });
        assert!(validate_retrieval_profile(&missing).is_err());

        let mut zero_capacity = valid.clone();
        zero_capacity["profile"]["retrieval_profile_final"]["permit_capacity"] = Value::from(0);
        assert!(validate_retrieval_profile(&zero_capacity).is_err());

        let mut unbalanced = valid.clone();
        unbalanced["profile"]["retrieval_profile_final"]["conservation"]["logical_completed_minus_deliveries"] =
            Value::from(1);
        assert!(validate_retrieval_profile(&unbalanced).is_err());

        let mut scope_mismatch = valid.clone();
        scope_mismatch["profile"]["retrieval_profile_final"]["physical_dispatched"] =
            Value::from(1);
        assert!(validate_retrieval_profile(&scope_mismatch).is_err());

        let mut malformed_histogram = valid;
        malformed_histogram["profile"]["retrieval_profile_final"]["immediate_attempt_ms"]["count"] =
            Value::from(1);
        assert!(validate_retrieval_profile(&malformed_histogram).is_err());
    }

    #[test]
    fn rolling_group_validator_requires_one_shot_nontruncated_lifecycle_evidence() {
        let valid = valid_rolling_group_metrics();
        validate_rolling_group_trace(&valid).expect("valid rolling-group trace");

        let mut missing_getter = valid.clone();
        missing_getter["profile"]["retrieval_rolling_group_trace_capture"]["getter_present"] =
            json!(false);
        assert!(validate_rolling_group_trace(&missing_getter).is_err());

        let mut truncated = valid.clone();
        truncated["profile"]["retrieval_rolling_group_trace"]["events_attempted"] = json!(5);
        truncated["profile"]["retrieval_rolling_group_trace"]["dropped"] = json!(1);
        truncated["profile"]["retrieval_rolling_group_trace"]["truncated"] = json!(true);
        assert!(validate_rolling_group_trace(&truncated).is_err());

        let mut bad_group_id = valid.clone();
        bad_group_id["profile"]["retrieval_rolling_group_trace"]["events"][1]["group_id"] =
            json!(2);
        assert!(validate_rolling_group_trace(&bad_group_id).is_err());

        let mut bad_terminal_counts = valid.clone();
        bad_terminal_counts["profile"]["retrieval_rolling_group_trace"]["events"][3]["parity_valid"] =
            json!(0);
        assert!(validate_rolling_group_trace(&bad_terminal_counts).is_err());

        let mut bad_reason = valid;
        bad_reason["profile"]["retrieval_rolling_group_trace"]["events"][3]["reason"] =
            json!("stale");
        assert!(validate_rolling_group_trace(&bad_reason).is_err());

        assert!(validate_rolling_group_trace(&empty_rolling_group_metrics("1")).is_err());
        assert!(validate_rolling_group_trace(&empty_rolling_group_metrics("5")).is_err());
    }

    #[test]
    fn rolling_group_validator_checks_gate_clock_active_width_and_close_shape() {
        let valid = valid_rolling_group_metrics();

        let mut gate_clock_mismatch = valid.clone();
        gate_clock_mismatch["profile"]["retrieval_rolling_group_trace"]["events"][1]["gate_elapsed_ms"] =
            json!(1_001);
        assert!(validate_rolling_group_trace(&gate_clock_mismatch).is_err());

        let mut too_wide = valid.clone();
        too_wide["profile"]["retrieval_rolling_group_trace"]["events"][2]["active_before"] =
            json!(3);
        too_wide["profile"]["retrieval_rolling_group_trace"]["events"][2]["active_after"] =
            json!(2);
        assert!(validate_rolling_group_trace(&too_wide).is_err());

        let mut terminal_active_jump = valid.clone();
        terminal_active_jump["profile"]["retrieval_rolling_group_trace"]["events"][3]["active"] =
            json!(1);
        assert!(validate_rolling_group_trace(&terminal_active_jump).is_err());

        let mut invalid_close_number = valid.clone();
        invalid_close_number["profile"]["retrieval_rolling_group_trace"]["events"][3]["close_at_ms"] =
            json!(1.0);
        invalid_close_number["profile"]["retrieval_rolling_group_trace"]["events"][3]["close_reason"] =
            Value::Null;
        assert!(validate_rolling_group_trace(&invalid_close_number).is_err());

        let mut terminal_too_wide = valid;
        terminal_too_wide["profile"]["retrieval_rolling_group_trace"]["events"][3]["active"] =
            json!(3);
        assert!(validate_rolling_group_trace(&terminal_too_wide).is_err());
    }

    #[test]
    fn first_reference_validator_requires_exact_terminal_windows_and_conjunctive_stop() {
        let valid = valid_first_reference_metrics();
        validate_first_reference_terminal_windows(&valid)
            .expect("valid exact first-reference terminal windows");

        let mut missing_terminal = valid.clone();
        missing_terminal["profile"]["service_worker_trace"]["range_progress"]["terminal_windows"]
            [2]["state"] = Value::Null;
        assert!(validate_first_reference_terminal_windows(&missing_terminal).is_err());

        let mut inexact_range = valid.clone();
        inexact_range["profile"]["service_worker_trace"]["range_progress"]["terminal_windows"][4]
            ["range"] = json!("bytes=2097152-2621438");
        assert!(validate_first_reference_terminal_windows(&inexact_range).is_err());

        let mut deadline_stop = valid.clone();
        deadline_stop["profile"]["service_worker_trace"]["range_progress"]["observer"]["disconnect_reason"] =
            json!("deadline");
        assert!(validate_first_reference_terminal_windows(&deadline_stop).is_err());

        let mut stopped_before_last_terminal = valid;
        stopped_before_last_terminal["profile"]["service_worker_trace"]["range_progress"]["observer"]
            ["disconnected_at_ms"] = json!(103.5);
        assert!(validate_first_reference_terminal_windows(&stopped_before_last_terminal).is_err());
    }

    #[test]
    fn served_identity_validator_requires_post_freeze_coherent_metadata_only() {
        let valid = valid_served_identity_metrics();
        validate_served_identity(&valid).expect("valid post-freeze served identity");

        let mut pre_measurement = valid.clone();
        pre_measurement["served_identity"]["identity_fetch_started_at_ms"] = json!(119_999.0);
        assert!(validate_served_identity(&pre_measurement).is_err());

        let mut malformed_hash = valid.clone();
        malformed_hash["served_identity"]["assets"]["wasm"]["sha256"] = json!("ABC");
        assert!(validate_served_identity(&malformed_hash).is_err());

        let mut mixed_build = valid.clone();
        mixed_build["served_identity"]["assets"]["javascript"]["build_version"] =
            json!("0000000000000000");
        assert!(validate_served_identity(&mixed_build).is_err());

        let mut persisted_body = valid.clone();
        persisted_body["served_identity"]["assets"]["index"]["body"] = json!("forbidden");
        assert!(validate_served_identity(&persisted_body).is_err());

        let mut source_mismatch = valid;
        source_mismatch["served_identity"]["source_wasm_build_id"] = json!("0000000000000000");
        assert!(validate_served_identity(&source_mismatch).is_err());
    }

    #[test]
    fn cold_start_attribution_summary_preserves_correlated_request_and_response() {
        let metrics = json!({
            "profile": {
                "service_worker_trace": {
                    "controller_changes": [{ "at_ms": 415.25 }],
                    "hls_requests": [{
                        "request_kind": "stream-open",
                        "request_at_ms": 1_025.5,
                        "url": "https://host/weeb-3/hls/bytes/abc",
                        "range": null,
                        "stream_token": null,
                        "response": {
                            "at_ms": 1_175.75,
                            "status": 200,
                            "header_fields": {
                                "X-Weeb3-HLS-Critical-Prefix-Windows": "5"
                            }
                        }
                    }, {
                        "request_kind": "range",
                        "request_at_ms": 1_200.0,
                        "range": "bytes=0-524287",
                        "stream_token": "9:beginning",
                        "response_capture": "armed"
                    }]
                }
            }
        });
        let summary = summarize_hls_cold_start_attribution(&metrics);
        assert_eq!(summary.first_controller_change_ms, Some(415.25));
        assert_eq!(summary.hls_request_count, 2);
        assert_eq!(summary.hls_range_request_count, 1);
        assert_eq!(summary.hls_response_count, 1);
        assert_eq!(summary.first_request_at_ms, Some(1_025.5));
        assert_eq!(
            summary.first_request_url.as_deref(),
            Some("https://host/weeb-3/hls/bytes/abc")
        );
        assert_eq!(summary.first_request_range, None);
        assert_eq!(summary.first_request_stream_token, None);
        assert_eq!(summary.first_response_at_ms, Some(1_175.75));
        assert_eq!(summary.first_response_status, Some(200));
        assert_eq!(
            summary.first_response_critical_prefix_windows.as_deref(),
            Some("5")
        );
        assert_eq!(summary.first_range_request_at_ms, Some(1_200.0));
        assert_eq!(summary.first_range.as_deref(), Some("bytes=0-524287"));
        assert_eq!(
            summary.first_range_stream_token.as_deref(),
            Some("9:beginning")
        );
        assert_eq!(
            summarize_hls_cold_start_attribution(&json!({})),
            HlsColdStartAttributionSummary::default()
        );
    }

    #[test]
    fn raw_startup_validator_enforces_ordering_and_conserved_credit_lifecycle() {
        let event = |at_ms: f64,
                     event: &str,
                     horizon: u64,
                     led: u64,
                     dispatched: u64,
                     completed: u64,
                     active: u64,
                     minted: u64,
                     available: u64,
                     held: u64,
                     discarded: u64,
                     scout_active: u64| {
            let attributed = horizon > 0 && matches!(event, "registration" | "completion");
            json!({
                "at_ms": at_ms,
                "schema_version": 3,
                "layer": "raw-singleflight",
                "event": event,
                "horizon_index": horizon,
                "horizon": format!("W{horizon}"),
                "registration": if matches!(event, "registration" | "completion") {
                    Some("Led")
                } else {
                    None
                },
                "raw_flight_id": if matches!(event, "registration" | "completion") {
                    Some(if horizon == 0 { "1" } else { "2" })
                } else {
                    None
                },
                "dispatch_accepted": if event == "registration" { Some(true) } else { None },
                "canonical_cac": if event == "completion" { Some(true) } else { None },
                "terminal_reason": if event == "trace-terminal" {
                    Some("admission-closed")
                } else {
                    None
                },
                "admission_open": !matches!(event, "admission-close" | "trace-terminal"),
                "group_id": attributed.then_some(1),
                "group_horizon_index": attributed.then_some(horizon),
                "group_depth": attributed.then_some(1),
                "group_parent_start": attributed.then_some("0"),
                "group_parent_span": attributed.then_some("12288"),
                "requested_first_index": attributed.then_some(0),
                "requested_last_index": attributed.then_some(2),
                "requested_count": attributed.then_some(3),
                "data_count": attributed.then_some(3),
                "parity_count": attributed.then_some(1),
                "decoded_raw_count": attributed.then_some(0),
                "decoded_only_count": attributed.then_some(0),
                "cache_miss_count": attributed.then_some(3),
                "child_index": attributed.then_some(1),
                "child_start": attributed.then_some("4096"),
                "child_span": attributed.then_some("4096"),
                "full_data_group_candidate": attributed.then_some(true),
                "full_data_group_eligible": attributed.then_some(true),
                "raw_leaders_led": led,
                "raw_leader_dispatches": dispatched,
                "raw_leader_completions": completed,
                "raw_leaders_active": active,
                "logical_retrieve_dispatches": dispatched,
                "credits_minted": minted,
                "credits_available": available,
                "credits_held": held,
                "credits_discarded": discarded,
                "scout_active": scout_active,
                "bee_peer_attempts": null,
                "retrieval_permits": null
            })
        };
        let valid = json!({
            "profile": {
                "raw_startup_trace": {
                    "schema_version": 3,
                    "cap": 2_048,
                    "data_cap": 2_047,
                    "dropped": 0,
                    "collector_terminal_reason": null,
                    "emitter_terminal_reason": "admission-closed",
                    "events": [
                        event(100.0, "registration", 0, 1, 1, 0, 1, 0, 0, 0, 0, 0),
                        event(150.0, "completion", 0, 1, 1, 1, 0, 1, 1, 0, 0, 0),
                        event(151.0, "registration", 1, 2, 2, 1, 1, 1, 0, 1, 0, 1),
                        event(170.0, "completion", 1, 2, 2, 2, 0, 1, 1, 0, 0, 0),
                        event(175.0, "admission-close", 0, 2, 2, 2, 0, 1, 0, 0, 1, 0),
                        event(176.0, "trace-terminal", 0, 2, 2, 2, 0, 1, 0, 0, 1, 0)
                    ]
                }
            }
        });
        validate_raw_startup_trace(&valid).expect("valid raw-startup lifecycle");
        let summary = summarize_hls_cold_start_attribution(&valid);
        assert_eq!(summary.raw_trace_event_count, 6);
        assert_eq!(summary.raw_leader_dispatch_count, Some(2));
        assert_eq!(summary.raw_leader_completion_count, Some(2));
        assert_eq!(summary.raw_credit_minted_count, Some(1));
        assert_eq!(summary.raw_credit_available_final, Some(0));
        assert_eq!(summary.raw_credit_held_final, Some(0));
        assert_eq!(summary.raw_credit_discarded_final, Some(1));
        assert_eq!(summary.raw_scout_active_final, Some(0));

        let mut silent_completion_loss = valid.clone();
        silent_completion_loss["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .remove(1);
        assert!(validate_raw_startup_trace(&silent_completion_loss).is_err());

        let mut silent_led_loss = valid.clone();
        silent_led_loss["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .remove(0);
        assert!(validate_raw_startup_trace(&silent_led_loss).is_err());

        let mut accepted_led_aggregate_mismatch = valid.clone();
        accepted_led_aggregate_mismatch["profile"]["raw_startup_trace"]["events"][2]["dispatch_accepted"] =
            json!(false);
        assert!(validate_raw_startup_trace(&accepted_led_aggregate_mismatch).is_err());

        let mut false_dispatch = valid.clone();
        false_dispatch["profile"]["raw_startup_trace"]["events"][2]["dispatch_accepted"] =
            json!(false);
        false_dispatch["profile"]["raw_startup_trace"]["events"][3]["canonical_cac"] = json!(false);
        for index in 2..6 {
            false_dispatch["profile"]["raw_startup_trace"]["events"][index]["raw_leader_dispatches"] =
                json!(1);
            false_dispatch["profile"]["raw_startup_trace"]["events"][index]["logical_retrieve_dispatches"] =
                json!(1);
        }
        validate_raw_startup_trace(&false_dispatch)
            .expect("a failed logical enqueue still has one Led registration and completion");

        let mut matching_joined = valid.clone();
        let mut matching_joined_row =
            matching_joined["profile"]["raw_startup_trace"]["events"][0].clone();
        matching_joined_row["at_ms"] = json!(125.0);
        matching_joined_row["registration"] = json!("Joined");
        matching_joined_row["dispatch_accepted"] = Value::Null;
        matching_joined["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .insert(1, matching_joined_row.clone());
        validate_raw_startup_trace(&matching_joined)
            .expect("a Joined row may correlate to a traced Led flight");

        let mut unresolved_joined = valid.clone();
        matching_joined_row["raw_flight_id"] = json!("99");
        unresolved_joined["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .insert(1, matching_joined_row);
        validate_raw_startup_trace(&unresolved_joined)
            .expect("a Joined row may belong to an unprofiled foreground leader");

        let mut joined_after_completion = matching_joined;
        let rows = joined_after_completion["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events");
        let mut late_joined = rows.remove(1);
        late_joined["at_ms"] = json!(150.5);
        rows.insert(2, late_joined);
        assert!(validate_raw_startup_trace(&joined_after_completion).is_err());

        let mut cached_without_flight = valid.clone();
        let mut cached_row =
            cached_without_flight["profile"]["raw_startup_trace"]["events"][0].clone();
        cached_row["at_ms"] = json!(125.0);
        cached_row["registration"] = json!("Cached");
        cached_row["raw_flight_id"] = Value::Null;
        cached_row["dispatch_accepted"] = Value::Null;
        cached_without_flight["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .insert(1, cached_row);
        validate_raw_startup_trace(&cached_without_flight)
            .expect("a Cached row has no physical raw flight");

        let mut led_without_flight = valid.clone();
        led_without_flight["profile"]["raw_startup_trace"]["events"][0]["raw_flight_id"] =
            Value::Null;
        assert!(validate_raw_startup_trace(&led_without_flight).is_err());

        let mut control_with_flight = valid.clone();
        control_with_flight["profile"]["raw_startup_trace"]["events"][4]["raw_flight_id"] =
            json!("1");
        assert!(validate_raw_startup_trace(&control_with_flight).is_err());

        let mut mismatched_completion_flight = valid.clone();
        mismatched_completion_flight["profile"]["raw_startup_trace"]["events"][1]["raw_flight_id"] =
            json!("3");
        assert!(validate_raw_startup_trace(&mismatched_completion_flight).is_err());

        let set_counters = |row: &mut Value,
                            led: u64,
                            dispatched: u64,
                            completed: u64,
                            active: u64,
                            minted: u64,
                            available: u64,
                            held: u64,
                            discarded: u64,
                            scout_active: u64| {
            row["raw_leaders_led"] = json!(led);
            row["raw_leader_dispatches"] = json!(dispatched);
            row["raw_leader_completions"] = json!(completed);
            row["raw_leaders_active"] = json!(active);
            row["logical_retrieve_dispatches"] = json!(dispatched);
            row["credits_minted"] = json!(minted);
            row["credits_available"] = json!(available);
            row["credits_held"] = json!(held);
            row["credits_discarded"] = json!(discarded);
            row["scout_active"] = json!(scout_active);
        };
        let base_rows = valid["profile"]["raw_startup_trace"]["events"]
            .as_array()
            .expect("raw events");
        let mut w0_registration_1 = base_rows[0].clone();
        set_counters(&mut w0_registration_1, 1, 1, 0, 1, 0, 0, 0, 0, 0);
        let mut w0_registration_2 = w0_registration_1.clone();
        w0_registration_2["at_ms"] = json!(110.0);
        w0_registration_2["raw_flight_id"] = json!("3");
        set_counters(&mut w0_registration_2, 2, 2, 0, 2, 0, 0, 0, 0, 0);
        let mut w0_completion_1 = base_rows[1].clone();
        w0_completion_1["at_ms"] = json!(140.0);
        set_counters(&mut w0_completion_1, 2, 2, 1, 1, 1, 1, 0, 0, 0);
        let mut w0_completion_2 = base_rows[1].clone();
        w0_completion_2["raw_flight_id"] = json!("3");
        set_counters(&mut w0_completion_2, 2, 2, 2, 0, 2, 2, 0, 0, 0);
        let mut w1_registration_1 = base_rows[2].clone();
        set_counters(&mut w1_registration_1, 3, 3, 2, 1, 2, 1, 1, 0, 1);
        let mut w1_registration_2 = w1_registration_1.clone();
        w1_registration_2["at_ms"] = json!(152.0);
        w1_registration_2["raw_flight_id"] = json!("4");
        w1_registration_2["child_index"] = json!(2);
        w1_registration_2["child_start"] = json!("8192");
        set_counters(&mut w1_registration_2, 4, 4, 2, 2, 2, 0, 2, 0, 2);
        let mut w1_completion_1 = base_rows[3].clone();
        set_counters(&mut w1_completion_1, 4, 4, 3, 1, 2, 1, 1, 0, 1);
        let mut w1_completion_2 = w1_completion_1.clone();
        w1_completion_2["at_ms"] = json!(171.0);
        w1_completion_2["raw_flight_id"] = json!("4");
        w1_completion_2["child_index"] = json!(2);
        w1_completion_2["child_start"] = json!("8192");
        set_counters(&mut w1_completion_2, 4, 4, 4, 0, 2, 2, 0, 0, 0);
        let mut close = base_rows[4].clone();
        set_counters(&mut close, 4, 4, 4, 0, 2, 0, 0, 2, 0);
        let mut terminal = base_rows[5].clone();
        set_counters(&mut terminal, 4, 4, 4, 0, 2, 0, 0, 2, 0);
        let mut two_concurrent_flights = valid.clone();
        *two_concurrent_flights["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events") = vec![
            w0_registration_1,
            w0_registration_2,
            w0_completion_1,
            w0_completion_2,
            w1_registration_1,
            w1_registration_2,
            w1_completion_1,
            w1_completion_2,
            close,
            terminal,
        ];
        validate_raw_startup_trace(&two_concurrent_flights)
            .expect("two concurrent Led scout flights keep distinct child identities");
        let mut cross_swapped_flights = two_concurrent_flights;
        cross_swapped_flights["profile"]["raw_startup_trace"]["events"][6]["raw_flight_id"] =
            json!("4");
        cross_swapped_flights["profile"]["raw_startup_trace"]["events"][7]["raw_flight_id"] =
            json!("2");
        assert!(validate_raw_startup_trace(&cross_swapped_flights).is_err());

        let mut maximum_flight_id = valid.clone();
        for index in [0, 1] {
            maximum_flight_id["profile"]["raw_startup_trace"]["events"][index]["raw_flight_id"] =
                json!("18446744073709551615");
        }
        validate_raw_startup_trace(&maximum_flight_id)
            .expect("raw flight IDs remain lossless through u64::MAX");

        let mut noncanonical_flight_id = valid.clone();
        noncanonical_flight_id["profile"]["raw_startup_trace"]["events"][0]["raw_flight_id"] =
            json!("01");
        assert!(validate_raw_startup_trace(&noncanonical_flight_id).is_err());

        let mut overflowing_flight_id = valid.clone();
        overflowing_flight_id["profile"]["raw_startup_trace"]["events"][0]["raw_flight_id"] =
            json!("18446744073709551616");
        assert!(validate_raw_startup_trace(&overflowing_flight_id).is_err());

        let mut w0_only = valid.clone();
        let rows = w0_only["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events");
        rows.remove(3);
        rows.remove(2);
        for row in &mut rows[2..] {
            row["raw_leaders_led"] = json!(1);
            row["raw_leader_dispatches"] = json!(1);
            row["raw_leader_completions"] = json!(1);
            row["logical_retrieve_dispatches"] = json!(1);
        }
        w0_only["profile"]["service_worker_trace"] = json!({
            "hls_requests": [{ "response": { "header_fields": {
                "X-Weeb3-HLS-Critical-Prefix-Windows": "1"
            }}}]
        });
        validate_raw_startup_trace(&w0_only)
            .expect("a one-window trace may contain only W0 attribution");
        w0_only["profile"]["service_worker_trace"]["hls_requests"][0]["response"]["header_fields"]
            ["X-Weeb3-HLS-Critical-Prefix-Windows"] = json!("5");
        assert!(validate_raw_startup_trace(&w0_only).is_err());

        let mut partial_with_parity = valid.clone();
        for index in [2, 3] {
            let row = &mut partial_with_parity["profile"]["raw_startup_trace"]["events"][index];
            row["requested_first_index"] = json!(1);
            row["requested_last_index"] = json!(2);
            row["requested_count"] = json!(2);
            row["cache_miss_count"] = json!(2);
            row["full_data_group_candidate"] = json!(false);
            row["full_data_group_eligible"] = json!(false);
        }
        validate_raw_startup_trace(&partial_with_parity)
            .expect("a partial group may expose parity while remaining ineligible");

        let mut false_eligibility = partial_with_parity;
        false_eligibility["profile"]["raw_startup_trace"]["events"][2]["full_data_group_eligible"] =
            json!(true);
        false_eligibility["profile"]["raw_startup_trace"]["events"][3]["full_data_group_eligible"] =
            json!(true);
        assert!(validate_raw_startup_trace(&false_eligibility).is_err());

        let mut orphan_completion = valid.clone();
        orphan_completion["profile"]["raw_startup_trace"]["events"][3]["child_index"] = json!(2);
        orphan_completion["profile"]["raw_startup_trace"]["events"][3]["child_start"] =
            json!("8192");
        assert!(validate_raw_startup_trace(&orphan_completion).is_err());

        let mut changed_group = valid.clone();
        changed_group["profile"]["raw_startup_trace"]["events"][3]["group_parent_span"] =
            json!("16384");
        assert!(validate_raw_startup_trace(&changed_group).is_err());

        let mut partial_null_group = valid.clone();
        partial_null_group["profile"]["raw_startup_trace"]["events"][2]["group_depth"] =
            Value::Null;
        assert!(validate_raw_startup_trace(&partial_null_group).is_err());

        let mut noncanonical_geometry = valid.clone();
        noncanonical_geometry["profile"]["raw_startup_trace"]["events"][2]["child_start"] =
            json!("04096");
        assert!(validate_raw_startup_trace(&noncanonical_geometry).is_err());

        let mut overflowing_geometry = valid.clone();
        overflowing_geometry["profile"]["raw_startup_trace"]["events"][2]["group_parent_start"] =
            json!("18446744073709551616");
        assert!(validate_raw_startup_trace(&overflowing_geometry).is_err());

        let mut attributed_seed = valid.clone();
        for field in [
            "group_id",
            "group_horizon_index",
            "group_depth",
            "group_parent_start",
            "group_parent_span",
            "requested_first_index",
            "requested_last_index",
            "requested_count",
            "data_count",
            "parity_count",
            "decoded_raw_count",
            "decoded_only_count",
            "cache_miss_count",
            "child_index",
            "child_start",
            "child_span",
            "full_data_group_candidate",
            "full_data_group_eligible",
        ] {
            attributed_seed["profile"]["raw_startup_trace"]["events"][0][field] =
                valid["profile"]["raw_startup_trace"]["events"][2][field].clone();
        }
        assert!(validate_raw_startup_trace(&attributed_seed).is_err());

        let mut overlapping = valid.clone();
        let mut second_registration =
            overlapping["profile"]["raw_startup_trace"]["events"][2].clone();
        second_registration["at_ms"] = json!(152.0);
        second_registration["child_index"] = json!(2);
        second_registration["child_start"] = json!("4096");
        second_registration["registration"] = json!("Joined");
        second_registration["dispatch_accepted"] = Value::Null;
        second_registration["raw_flight_id"] = json!("99");
        overlapping["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .insert(3, second_registration);
        assert!(validate_raw_startup_trace(&overlapping).is_err());

        let mut duplicate_child = valid.clone();
        let mut duplicate_registration =
            duplicate_child["profile"]["raw_startup_trace"]["events"][2].clone();
        duplicate_registration["at_ms"] = json!(152.0);
        duplicate_registration["registration"] = json!("Joined");
        duplicate_registration["dispatch_accepted"] = Value::Null;
        duplicate_registration["raw_flight_id"] = json!("99");
        duplicate_child["profile"]["raw_startup_trace"]["events"]
            .as_array_mut()
            .expect("raw events")
            .insert(3, duplicate_registration);
        assert!(validate_raw_startup_trace(&duplicate_child).is_err());

        let mut bad_cache_class_sum = valid.clone();
        for index in [2, 3] {
            let row = &mut bad_cache_class_sum["profile"]["raw_startup_trace"]["events"][index];
            row["decoded_raw_count"] = json!(1);
            row["decoded_only_count"] = json!(1);
            row["cache_miss_count"] = json!(2);
            row["full_data_group_eligible"] = json!(false);
        }
        assert!(validate_raw_startup_trace(&bad_cache_class_sum).is_err());

        let mut impossible_miss_count = valid.clone();
        for index in [2, 3] {
            impossible_miss_count["profile"]["raw_startup_trace"]["events"][index]["decoded_raw_count"] =
                json!(3);
            impossible_miss_count["profile"]["raw_startup_trace"]["events"][index]["cache_miss_count"] =
                json!(0);
            impossible_miss_count["profile"]["raw_startup_trace"]["events"][index]["full_data_group_eligible"] =
                json!(false);
        }
        assert!(validate_raw_startup_trace(&impossible_miss_count).is_err());

        let mut capped = valid.clone();
        capped["profile"]["raw_startup_trace"]["emitter_terminal_reason"] = json!("cap-reached");
        capped["profile"]["raw_startup_trace"]["events"][5]["terminal_reason"] =
            json!("cap-reached");
        assert!(validate_raw_startup_trace(&capped).is_err());

        let mut broken = valid;
        broken["profile"]["raw_startup_trace"]["events"][1]["credits_available"] = json!(0);
        assert!(validate_raw_startup_trace(&broken).is_err());
    }

    #[test]
    fn raw_startup_validator_rejects_truncation_and_requires_multi_window_evidence() {
        let empty_trace = || {
            json!({
                "schema_version": 3,
                "cap": 2_048,
                "data_cap": 2_047,
                "events": [],
                "dropped": 0,
                "collector_terminal_reason": null,
                "emitter_terminal_reason": null
            })
        };
        let one_window = json!({
            "profile": {
                "raw_startup_trace": empty_trace(),
                "service_worker_trace": { "hls_requests": [{
                    "response": { "header_fields": {
                        "X-Weeb3-HLS-Critical-Prefix-Windows": "1"
                    }}
                }]}
            }
        });
        validate_raw_startup_trace(&one_window)
            .expect("a one-window or Live profile may have no raw events");

        let mut multi_window = one_window.clone();
        multi_window["profile"]["service_worker_trace"]["hls_requests"][0]["response"]["header_fields"]
            ["X-Weeb3-HLS-Critical-Prefix-Windows"] = json!("5");
        assert!(validate_raw_startup_trace(&multi_window).is_err());

        let mut collector_truncated = one_window.clone();
        collector_truncated["profile"]["raw_startup_trace"]["dropped"] = json!(1);
        collector_truncated["profile"]["raw_startup_trace"]["collector_terminal_reason"] =
            json!("cap-reached");
        assert!(validate_raw_startup_trace(&collector_truncated).is_err());

        let mut dispatch_failed = one_window;
        dispatch_failed["profile"]["raw_startup_trace"]["emitter_terminal_reason"] =
            json!("dispatch-failed");
        assert!(validate_raw_startup_trace(&dispatch_failed).is_err());
    }

    #[test]
    fn contiguous_buffer_uses_only_the_range_containing_playback() {
        let ranges = [(0.0, 4.0), (6.0, 10.0)];
        assert_eq!(contiguous_forward_buffer(2.5, &ranges), 1.5);
        assert_eq!(contiguous_forward_buffer(5.0, &ranges), 0.0);
        assert!((contiguous_forward_buffer(6.02, &ranges) - 3.98).abs() < f64::EPSILON * 4.0);
    }

    #[test]
    fn equal_post_minute_windows_report_the_matched_low_water_trend() {
        let samples = [
            sample(60_500.0, 59.5, 4.0),
            sample(61_000.0, 60.0, 5.0),
            sample(70_000.0, 69.0, 6.0),
            sample(80_000.0, 79.0, 7.0),
            sample(91_000.0, 90.0, 8.0),
        ];
        let summary = summarize_hls_buffer(&samples, Some(1_000.0), 15_000.0);
        assert_eq!(summary.first_minute, Some(samples[1]));
        assert_eq!(summary.early_low_water_s, Some(5.0));
        assert_eq!(summary.late_low_water_s, Some(7.0));
        assert_eq!(summary.low_water_trend_s, Some(2.0));
        assert_eq!(summary.matched_window_ms, Some(15_000.0));
    }

    #[test]
    fn timeline_metric_rejects_only_discontinuous_media_clock_changes() {
        let stable = [sample(1_000.0, 0.0, 5.0), sample(1_500.0, 0.5, 5.0)];
        assert_eq!(
            summarize_hls_buffer(&stable, Some(1_000.0), 15_000.0).timeline_jump_count,
            0
        );
        let jumped = [stable[0], sample(1_500.0, 5.0, 5.0)];
        assert_eq!(
            summarize_hls_buffer(&jumped, Some(1_000.0), 15_000.0).timeline_jump_count,
            1
        );
    }

    #[test]
    fn first_playing_metric_preserves_the_initial_media_clock() {
        let metrics = json!({
            "profile": {
                "events": [
                    { "event": "waiting", "current_time_s": 0.0 },
                    { "event": "playing", "current_time_s": 870.140005 },
                    { "event": "playing", "current_time_s": 871.0 }
                ]
            }
        });
        assert_eq!(
            hls_first_event_value(&metrics, "playing", "current_time_s"),
            Some(870.140005)
        );
    }

    #[test]
    fn playback_checkpoints_preserve_first_frame_timeline_and_nearest_runway_samples() {
        let metrics = json!({
            "profile": {
                "events": [{
                    "event": "playing", "at_ms": 1_000.0,
                    "current_time_s": 870.14, "duration_s": 900.0,
                    "forward_buffer_s": 3.5, "timeline": "absolute"
                }],
                "first_presented_frame": {
                    "event": "presented-frame", "at_ms": 1_120.0,
                    "current_time_s": 870.22, "media_time_s": 870.20,
                    "duration_s": 900.0, "forward_buffer_s": 3.42,
                    "timeline": "absolute"
                },
                "samples": [
                    { "at_ms": 10_800.0, "current_time_s": 880.0,
                      "duration_s": 900.0, "forward_buffer_s": 8.0,
                      "timeline": "absolute" },
                    { "at_ms": 31_200.0, "current_time_s": 900.0,
                      "duration_s": 900.0, "forward_buffer_s": 16.0,
                      "timeline": "absolute" },
                    { "at_ms": 61_100.0, "current_time_s": 930.0,
                      "duration_s": 930.0, "forward_buffer_s": 24.0,
                      "timeline": "absolute" }
                ]
            }
        });
        let summary = summarize_hls_playback_checkpoints(&metrics, Some(1_000.0));
        assert_eq!(
            summary
                .first_playing
                .as_ref()
                .and_then(|checkpoint| checkpoint.forward_buffer_s),
            Some(3.5)
        );
        assert_eq!(
            summary
                .first_presented_frame
                .as_ref()
                .and_then(|checkpoint| checkpoint.media_time_s),
            Some(870.20)
        );
        assert_eq!(
            summary
                .first_presented_frame
                .as_ref()
                .and_then(|checkpoint| checkpoint.timeline.as_deref()),
            Some("absolute")
        );
        assert_eq!(
            summary.after_10_seconds.unwrap().forward_buffer_s,
            Some(8.0)
        );
        assert_eq!(
            summary.after_30_seconds.unwrap().forward_buffer_s,
            Some(16.0)
        );
        assert_eq!(
            summary.after_60_seconds.unwrap().forward_buffer_s,
            Some(24.0)
        );
    }

    #[test]
    fn duration_summary_exposes_large_post_frame_live_window_expansion() {
        let metrics = json!({
            "measured_at_ms": 4_000.0,
            "profile": {
                "first_presented_frame": {
                    "event": "presented-frame", "at_ms": 1_000.0,
                    "duration_s": 48.0
                },
                "events": [
                    { "event": "playing", "at_ms": 1_100.0, "duration_s": 48.0 },
                    { "event": "durationchange", "at_ms": 2_000.0,
                      "duration_s": 2_552.0 },
                    { "event": "durationchange", "at_ms": 3_000.0,
                      "duration_s": 2_560.0 }
                ],
                "samples": [
                    { "at_ms": 1_500.0, "duration_s": 48.0 },
                    { "at_ms": 2_500.0, "duration_s": 2_552.0 }
                ]
            },
            "media": { "duration_s": 2_560.0 }
        });
        let summary = summarize_hls_duration_changes(&metrics, Some(1_100.0));
        assert_eq!(summary.anchor.as_deref(), Some("first_presented_frame"));
        assert_eq!(summary.anchor_at_ms, Some(1_000.0));
        assert_eq!(summary.initial_duration_s, Some(48.0));
        assert_eq!(summary.final_duration_s, Some(2_560.0));
        assert_eq!(summary.max_duration_s, Some(2_560.0));
        assert_eq!(summary.largest_positive_jump_s, Some(2_504.0));
        assert_eq!(summary.largest_negative_jump_s, None);
        assert_eq!(summary.durationchange_event_count, 2);
        validate_hls_duration_jump(&summary, 2_504.0).unwrap();
        assert!(validate_hls_duration_jump(&summary, 2_503.0).is_err());
    }

    #[test]
    fn duration_summary_ignores_non_finite_values_and_reports_rolling_changes() {
        let summary = summarize_hls_duration_observations(
            "first_playing",
            1_000.0,
            0,
            [
                (1_000.0, f64::NAN),
                (1_100.0, f64::INFINITY),
                (1_200.0, 48.0),
                (1_300.0, 56.0),
                (1_400.0, 64.0),
                (1_500.0, 60.0),
                (1_600.0, -1.0),
            ],
        );
        assert_eq!(summary.initial_duration_s, Some(48.0));
        assert_eq!(summary.final_duration_s, Some(60.0));
        assert_eq!(summary.max_duration_s, Some(64.0));
        assert_eq!(summary.largest_positive_jump_s, Some(8.0));
        assert_eq!(summary.largest_negative_jump_s, Some(-4.0));
        assert_eq!(summary.observation_count, 4);
        assert_eq!(summary.observed_change_count, 3);
        validate_hls_duration_jump(&summary, 8.0).unwrap();
        assert!(validate_hls_duration_jump(&summary, 7.999).is_err());
    }

    #[test]
    fn duration_summary_keeps_null_duration_events_counted_but_not_numeric() {
        let metrics = json!({
            "measured_at_ms": 2_000.0,
            "profile": {
                "events": [
                    { "event": "playing", "at_ms": 1_000.0, "duration_s": null },
                    { "event": "durationchange", "at_ms": 1_500.0,
                      "duration_s": null }
                ],
                "samples": [
                    { "at_ms": 1_750.0, "duration_s": "NaN" }
                ]
            },
            "media": { "duration_s": null }
        });
        let summary = summarize_hls_duration_changes(&metrics, Some(1_000.0));
        assert_eq!(summary.anchor.as_deref(), Some("first_playing"));
        assert_eq!(summary.initial_duration_s, None);
        assert_eq!(summary.final_duration_s, None);
        assert_eq!(summary.max_duration_s, None);
        assert_eq!(summary.durationchange_event_count, 1);
        assert_eq!(summary.observation_count, 0);
    }

    #[test]
    fn waiting_summary_pairs_post_start_episodes_and_accounts_for_an_open_wait() {
        let metrics = json!({
            "measured_at_ms": 4_500.0,
            "profile": {
                "events": [
                    { "event": "waiting", "at_ms": 500.0, "timeline": "relative" },
                    { "event": "playing", "at_ms": 1_000.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 2_000.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 2_100.0, "timeline": "relative" },
                    { "event": "playing", "at_ms": 2_600.0, "timeline": "absolute" },
                    { "event": "waiting", "at_ms": 4_000.0, "timeline": "relative" }
                ]
            }
        });
        let summary = summarize_hls_waiting_episodes(&metrics, Some(1_000.0));
        assert_eq!(summary.completed_count, 1);
        assert_eq!(summary.open_count, 1);
        assert_eq!(summary.total_observed_ms, 1_100.0);
        assert_eq!(summary.max_observed_ms, Some(600.0));
        assert_eq!(summary.episodes.len(), 2);
        assert_eq!(summary.episodes[0].waiting_at_ms, 2_000.0);
        assert_eq!(summary.episodes[0].playing_at_ms, Some(2_600.0));
        assert_eq!(
            summary.episodes[0].waiting_timeline.as_deref(),
            Some("absolute")
        );
        assert_eq!(summary.episodes[1].playing_at_ms, None);
        assert_eq!(summary.episodes[1].observed_duration_ms, 500.0);
    }

    #[test]
    fn service_worker_gate_requires_controller_protocol_and_scope() {
        let valid = json!({
            "service_worker": {
                "controlled": true,
                "protocol": {
                    "type": "WEEB3_PONG",
                    "protocol": SERVICE_WORKER_PROTOCOL,
                    "scope": "/weeb-3/"
                }
            }
        });
        validate_service_worker(&valid, "/weeb-3/").unwrap();

        let no_controller = json!({
            "service_worker": {
                "controlled": false,
                "protocol": null
            }
        });
        assert!(validate_service_worker(&no_controller, "/weeb-3/").is_err());

        let old_protocol = json!({
            "service_worker": {
                "controlled": true,
                "protocol": {
                    "type": "WEEB3_PONG",
                    "protocol": 4,
                    "scope": "/weeb-3/"
                }
            }
        });
        assert!(validate_service_worker(&old_protocol, "/weeb-3/").is_err());
    }

    #[test]
    fn cold_start_gate_requires_empty_worker_and_cache_state() {
        let valid = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": false,
                    "registrations": {
                        "status": "resolved", "count": 0, "scripts": []
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 0, "names": []
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        validate_cold_browser_start(&valid).unwrap();

        let controlled = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": true,
                    "registrations": {
                        "status": "resolved", "count": 1, "scripts": ["service.js"]
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 0, "names": []
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        assert!(validate_cold_browser_start(&controlled).is_err());

        let populated_cache = json!({
            "profile": {
                "cold_start": {
                    "controller_present_at_document_start": false,
                    "registrations": {
                        "status": "resolved", "count": 0, "scripts": []
                    },
                    "cache_storage": {
                        "status": "resolved", "count": 1, "names": ["stale"]
                    },
                    "http_cache_cleared_before_navigation": true,
                    "http_cache_disabled_before_navigation": true
                }
            }
        });
        assert!(validate_cold_browser_start(&populated_cache).is_err());
    }
}
