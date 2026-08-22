use std::{cell::Cell, collections::VecDeque, future::Future, pin::Pin, rc::Rc};

use js_sys::{Object, Reflect};
use libp2p::futures::{
    FutureExt, Stream, StreamExt,
    future::{Either, select},
    pin_mut,
    stream::FuturesUnordered,
};
use wasm_bindgen::{JsValue, closure::Closure};
use web_sys::{CustomEvent, CustomEventInit, Window};

use crate::{
    ChunkRetrieveSender, RetrieveCancelToken, RetrieveGenerationMap,
    erasure_coding::{BEE_MAX_UPLOAD_TREE_LEVELS, CHUNK_SIZE, reference_layout, split_references},
    mpsc,
    retrieval::{
        DecodedJoinChunk, RawFetchLifecycle, RawFetchLifecycleFactory, RawFetchRegistration,
        RequestedShardCache, cached_decoded_chunk, queue_decoded_join_child_cancellable,
        requested_shard_cache,
    },
    retrieval_conventions::RetrieveAdmission,
    retrieval_profile, retrieve_cancel_token_current,
};

fn startup_scout_nearest_incomplete_horizon(
    horizon_count: usize,
    current: usize,
    mut incomplete: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (current..horizon_count).find(|&horizon| incomplete(horizon))
}

fn startup_scout_next_admission_horizon(
    horizon_count: usize,
    earliest_incomplete_horizon: usize,
    mut ready: impl FnMut(usize) -> bool,
) -> Option<usize> {
    (earliest_incomplete_horizon..horizon_count).find(|&horizon| ready(horizon))
}

enum ReadyItemOrCredit<T, C> {
    Item(T),
    Credit(C),
    Pending,
}

fn ready_item_before_credit<S, C>(
    active: &mut S,
    try_credit: impl FnOnce() -> Option<C>,
) -> ReadyItemOrCredit<S::Item, C>
where
    S: Stream + Unpin,
{
    match active.next().now_or_never() {
        Some(Some(item)) => ReadyItemOrCredit::Item(item),
        Some(None) | None => try_credit()
            .map(ReadyItemOrCredit::Credit)
            .unwrap_or(ReadyItemOrCredit::Pending),
    }
}

async fn scout_cancel_token_current(
    cancel_generations: &Option<RetrieveGenerationMap>,
    cancel: &Option<RetrieveCancelToken>,
) -> bool {
    if let (Some(generations), Some(_)) = (cancel_generations, cancel) {
        return retrieve_cancel_token_current(generations, cancel).await;
    }
    true
}

const RETRIEVAL_PROFILE_FLAG: &str = "__weeb3HlsRetrieveProfileEnabled";
const RETRIEVAL_PROFILE_GETTER: &str = "__weeb3GetHlsRetrieveProfileSnapshot";
const ROLLING_GROUP_TRACE_FINALIZER: &str = "__weeb3FinalizeHlsRetrieveRollingGroupTrace";
const STARTUP_RAW_PROFILE_FLAG: &str = "__weeb3HlsRawStartupProfileEnabled";
const STARTUP_RAW_PROFILE_EVENT: &str = "weeb3-hls-raw-startup-profile";
const STARTUP_RAW_PROFILE_EVENT_CAP: u64 = 2_048;
const STARTUP_RAW_PROFILE_DATA_EVENT_CAP: u64 = STARTUP_RAW_PROFILE_EVENT_CAP - 1;
const STARTUP_RAW_PROFILE_OBJECT: &str = "__weeb3HlsProfile";

thread_local! {
    static RETRIEVAL_PROFILE_ADAPTER_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn activate_retrieval_profile_if_requested() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let enabled = Reflect::get(window.as_ref(), &JsValue::from_str(RETRIEVAL_PROFILE_FLAG))
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !enabled || RETRIEVAL_PROFILE_ADAPTER_INSTALLED.with(Cell::get) {
        return;
    }

    retrieval_profile::activate();
    RETRIEVAL_PROFILE_ADAPTER_INSTALLED.with(|installed| installed.set(true));

    let snapshot_getter = Closure::<dyn FnMut() -> JsValue>::new(|| {
        retrieval_profile::snapshot_now()
            .and_then(|snapshot| serde_json::to_string(&snapshot).ok())
            .and_then(|json| js_sys::JSON::parse(&json).ok())
            .unwrap_or(JsValue::NULL)
    });
    if Reflect::set(
        window.as_ref(),
        &JsValue::from_str(RETRIEVAL_PROFILE_GETTER),
        snapshot_getter.as_ref(),
    )
    .is_ok()
    {
        snapshot_getter.forget();
    }

    let rolling_finalizer = Closure::<dyn FnMut() -> JsValue>::new(|| {
        retrieval_profile::finalize_rolling_group_trace_now()
            .and_then(|snapshot| serde_json::to_string(&snapshot).ok())
            .and_then(|json| js_sys::JSON::parse(&json).ok())
            .unwrap_or(JsValue::NULL)
    });
    if Reflect::set(
        window.as_ref(),
        &JsValue::from_str(ROLLING_GROUP_TRACE_FINALIZER),
        rolling_finalizer.as_ref(),
    )
    .is_ok()
    {
        rolling_finalizer.forget();
    }
}

struct StartupRawProfileGroup {
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
    full_data_group_candidate: bool,
    decoded_raw_count: Cell<u64>,
    decoded_only_count: Cell<u64>,
    cache_miss_count: Cell<u64>,
    full_data_group_eligible: Cell<bool>,
}

impl StartupRawProfileGroup {
    fn finalize_cache_classification(
        &self,
        decoded_raw_count: usize,
        decoded_only_count: usize,
        cache_miss_count: usize,
    ) {
        self.decoded_raw_count.set(decoded_raw_count as u64);
        self.decoded_only_count.set(decoded_only_count as u64);
        self.cache_miss_count.set(cache_miss_count as u64);
        self.full_data_group_eligible.set(
            self.full_data_group_candidate
                && cache_miss_count > 0
                && decoded_only_count < self.parity_count as usize,
        );
    }
}

#[derive(Clone)]
struct StartupRawProfileChild {
    group: Rc<StartupRawProfileGroup>,
    child_index: u64,
    child_start: u64,
    child_span: u64,
}

struct StartupRawProfileTrace {
    window: Window,
    events_emitted: Cell<u64>,
    next_scout_group_id: Cell<u64>,
    raw_leaders_led: Cell<u64>,
    raw_leader_dispatches: Cell<u64>,
    raw_leader_completions: Cell<u64>,
    raw_leaders_active: Cell<u64>,
    logical_retrieve_dispatches: Cell<u64>,
    credits_minted: Cell<u64>,
    credits_available: Cell<u64>,
    credits_held: Cell<u64>,
    credits_discarded: Cell<u64>,
    scout_active: Cell<u64>,
    admission_closed: Cell<bool>,
    terminated: Cell<bool>,
}

impl StartupRawProfileTrace {
    fn activate() -> Option<Rc<Self>> {
        let window = web_sys::window()?;
        let enabled = Reflect::get(
            window.as_ref(),
            &JsValue::from_str(STARTUP_RAW_PROFILE_FLAG),
        )
        .ok()?
        .as_bool()
        .unwrap_or(false);
        enabled.then(|| {
            Rc::new(Self {
                window,
                events_emitted: Cell::new(0),
                next_scout_group_id: Cell::new(1),
                raw_leaders_led: Cell::new(0),
                raw_leader_dispatches: Cell::new(0),
                raw_leader_completions: Cell::new(0),
                raw_leaders_active: Cell::new(0),
                logical_retrieve_dispatches: Cell::new(0),
                credits_minted: Cell::new(0),
                credits_available: Cell::new(0),
                credits_held: Cell::new(0),
                credits_discarded: Cell::new(0),
                scout_active: Cell::new(0),
                admission_closed: Cell::new(false),
                terminated: Cell::new(false),
            })
        })
    }

    fn set_number(detail: &Object, name: &str, value: u64) {
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str(name),
            &JsValue::from_f64(value as f64),
        );
    }

    fn set_optional_bool(detail: &Object, name: &str, value: Option<bool>) {
        let value = value.map(JsValue::from_bool).unwrap_or(JsValue::NULL);
        let _ = Reflect::set(detail.as_ref(), &JsValue::from_str(name), &value);
    }

    fn set_optional_string(detail: &Object, name: &str, value: Option<&str>) {
        let value = value.map(JsValue::from_str).unwrap_or(JsValue::NULL);
        let _ = Reflect::set(detail.as_ref(), &JsValue::from_str(name), &value);
    }

    fn set_optional_number(detail: &Object, name: &str, value: Option<u64>) {
        let value = value
            .map(|value| JsValue::from_f64(value as f64))
            .unwrap_or(JsValue::NULL);
        let _ = Reflect::set(detail.as_ref(), &JsValue::from_str(name), &value);
    }

    fn set_optional_u64_string(detail: &Object, name: &str, value: Option<u64>) {
        let value = value
            .map(|value| JsValue::from_str(&value.to_string()))
            .unwrap_or(JsValue::NULL);
        let _ = Reflect::set(detail.as_ref(), &JsValue::from_str(name), &value);
    }

    fn new_scout_group(
        &self,
        horizon: usize,
        depth: usize,
        parent_start: u64,
        parent_span: u64,
        requested_first_index: usize,
        requested_last_index: usize,
        data_count: usize,
        parity_count: usize,
    ) -> Rc<StartupRawProfileGroup> {
        let group_id = self.next_scout_group_id.get();
        self.next_scout_group_id.set(group_id.saturating_add(1));
        let requested_count = requested_last_index
            .saturating_sub(requested_first_index)
            .saturating_add(1);
        Rc::new(StartupRawProfileGroup {
            group_id,
            horizon: horizon as u64,
            depth: depth as u64,
            parent_start,
            parent_span,
            requested_first_index: requested_first_index as u64,
            requested_last_index: requested_last_index as u64,
            requested_count: requested_count as u64,
            data_count: data_count as u64,
            parity_count: parity_count as u64,
            full_data_group_candidate: requested_first_index == 0
                && requested_count == data_count
                && parity_count > 0,
            decoded_raw_count: Cell::new(0),
            decoded_only_count: Cell::new(0),
            cache_miss_count: Cell::new(0),
            full_data_group_eligible: Cell::new(false),
        })
    }

    fn dispatch(
        &self,
        event_name: &str,
        horizon: usize,
        registration: Option<&str>,
        dispatch_accepted: Option<bool>,
        canonical_cac: Option<bool>,
        raw_flight_id: Option<u64>,
        admission_open: bool,
        terminal_reason: Option<&str>,
        profile_child: Option<&StartupRawProfileChild>,
    ) -> bool {
        let detail = Object::new();
        Self::set_number(&detail, "schema_version", 3);
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("layer"),
            &JsValue::from_str("raw-singleflight"),
        );
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("event"),
            &JsValue::from_str(event_name),
        );
        Self::set_number(&detail, "horizon_index", horizon as u64);
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("horizon"),
            &JsValue::from_str(&format!("W{horizon}")),
        );
        let registration = registration.map(JsValue::from_str).unwrap_or(JsValue::NULL);
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("registration"),
            &registration,
        );
        Self::set_optional_bool(&detail, "dispatch_accepted", dispatch_accepted);
        Self::set_optional_bool(&detail, "canonical_cac", canonical_cac);
        Self::set_optional_u64_string(&detail, "raw_flight_id", raw_flight_id);
        Self::set_optional_string(&detail, "terminal_reason", terminal_reason);
        let group = profile_child.map(|child| child.group.as_ref());
        Self::set_optional_number(&detail, "group_id", group.map(|group| group.group_id));
        Self::set_optional_number(
            &detail,
            "group_horizon_index",
            group.map(|group| group.horizon),
        );
        Self::set_optional_number(&detail, "group_depth", group.map(|group| group.depth));
        Self::set_optional_u64_string(
            &detail,
            "group_parent_start",
            group.map(|group| group.parent_start),
        );
        Self::set_optional_u64_string(
            &detail,
            "group_parent_span",
            group.map(|group| group.parent_span),
        );
        Self::set_optional_number(
            &detail,
            "requested_first_index",
            group.map(|group| group.requested_first_index),
        );
        Self::set_optional_number(
            &detail,
            "requested_last_index",
            group.map(|group| group.requested_last_index),
        );
        Self::set_optional_number(
            &detail,
            "requested_count",
            group.map(|group| group.requested_count),
        );
        Self::set_optional_number(&detail, "data_count", group.map(|group| group.data_count));
        Self::set_optional_number(
            &detail,
            "parity_count",
            group.map(|group| group.parity_count),
        );
        Self::set_optional_number(
            &detail,
            "decoded_raw_count",
            group.map(|group| group.decoded_raw_count.get()),
        );
        Self::set_optional_number(
            &detail,
            "decoded_only_count",
            group.map(|group| group.decoded_only_count.get()),
        );
        Self::set_optional_number(
            &detail,
            "cache_miss_count",
            group.map(|group| group.cache_miss_count.get()),
        );
        Self::set_optional_number(
            &detail,
            "child_index",
            profile_child.map(|child| child.child_index),
        );
        Self::set_optional_u64_string(
            &detail,
            "child_start",
            profile_child.map(|child| child.child_start),
        );
        Self::set_optional_u64_string(
            &detail,
            "child_span",
            profile_child.map(|child| child.child_span),
        );
        Self::set_optional_bool(
            &detail,
            "full_data_group_candidate",
            group.map(|group| group.full_data_group_candidate),
        );
        Self::set_optional_bool(
            &detail,
            "full_data_group_eligible",
            group.map(|group| group.full_data_group_eligible.get()),
        );
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("admission_open"),
            &JsValue::from_bool(admission_open),
        );
        Self::set_number(&detail, "raw_leaders_led", self.raw_leaders_led.get());
        Self::set_number(
            &detail,
            "raw_leader_dispatches",
            self.raw_leader_dispatches.get(),
        );
        Self::set_number(
            &detail,
            "raw_leader_completions",
            self.raw_leader_completions.get(),
        );
        Self::set_number(&detail, "raw_leaders_active", self.raw_leaders_active.get());
        Self::set_number(
            &detail,
            "logical_retrieve_dispatches",
            self.logical_retrieve_dispatches.get(),
        );
        Self::set_number(&detail, "credits_minted", self.credits_minted.get());
        Self::set_number(&detail, "credits_available", self.credits_available.get());
        Self::set_number(&detail, "credits_held", self.credits_held.get());
        Self::set_number(&detail, "credits_discarded", self.credits_discarded.get());
        Self::set_number(&detail, "scout_active", self.scout_active.get());
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("bee_peer_attempts"),
            &JsValue::NULL,
        );
        let _ = Reflect::set(
            detail.as_ref(),
            &JsValue::from_str("retrieval_permits"),
            &JsValue::NULL,
        );

        let init = CustomEventInit::new();
        init.set_detail(detail.as_ref());
        CustomEvent::new_with_event_init_dict(STARTUP_RAW_PROFILE_EVENT, &init)
            .ok()
            .and_then(|event| self.window.dispatch_event(&event).ok())
            .is_some()
    }

    fn publish_terminal_reason(&self, reason: &str) {
        let Ok(profile) = Reflect::get(
            self.window.as_ref(),
            &JsValue::from_str(STARTUP_RAW_PROFILE_OBJECT),
        ) else {
            return;
        };
        let Ok(trace) = Reflect::get(&profile, &JsValue::from_str("raw_startup_trace")) else {
            return;
        };
        let _ = Reflect::set(
            &trace,
            &JsValue::from_str("emitter_terminal_reason"),
            &JsValue::from_str(reason),
        );
    }

    fn emit_terminal(&self, reason: &'static str, horizon: usize, admission_open: bool) {
        if self.terminated.replace(true) {
            return;
        }
        self.publish_terminal_reason(reason);
        let emitted = self.events_emitted.get();
        if emitted >= STARTUP_RAW_PROFILE_EVENT_CAP {
            return;
        }
        if self.dispatch(
            "trace-terminal",
            horizon,
            None,
            None,
            None,
            None,
            admission_open,
            Some(reason),
            None,
        ) {
            self.events_emitted.set(emitted.saturating_add(1));
        } else {
            self.publish_terminal_reason("dispatch-failed");
        }
    }

    fn emit(
        &self,
        event_name: &str,
        horizon: usize,
        registration: Option<&str>,
        dispatch_accepted: Option<bool>,
        canonical_cac: Option<bool>,
        raw_flight_id: Option<u64>,
        admission_open: bool,
        profile_child: Option<&StartupRawProfileChild>,
    ) {
        if self.terminated.get() {
            return;
        }
        let emitted = self.events_emitted.get();
        if emitted >= STARTUP_RAW_PROFILE_DATA_EVENT_CAP {
            self.emit_terminal("cap-reached", horizon, admission_open);
            return;
        }
        if self.dispatch(
            event_name,
            horizon,
            registration,
            dispatch_accepted,
            canonical_cac,
            raw_flight_id,
            admission_open,
            None,
            profile_child,
        ) {
            self.events_emitted.set(emitted.saturating_add(1));
        } else {
            self.emit_terminal("dispatch-failed", horizon, admission_open);
        }
    }

    fn admission_closed(&self) {
        if !self.admission_closed.replace(true) {
            self.emit("admission-close", 0, None, None, None, None, false, None);
        }
        self.maybe_emit_final();
    }

    fn maybe_emit_final(&self) {
        if self.admission_closed.get()
            && self.raw_leaders_active.get() == 0
            && self.credits_available.get() == 0
            && self.credits_held.get() == 0
        {
            self.emit_terminal("admission-closed", 0, false);
        }
    }

    fn credit_acquired(&self) {
        self.credits_available
            .set(self.credits_available.get().saturating_sub(1));
        self.credits_held
            .set(self.credits_held.get().saturating_add(1));
    }

    fn credit_released(&self, returned: bool) {
        self.credits_held
            .set(self.credits_held.get().saturating_sub(1));
        if returned {
            self.credits_available
                .set(self.credits_available.get().saturating_add(1));
        } else {
            self.credits_discarded
                .set(self.credits_discarded.get().saturating_add(1));
        }
        self.maybe_emit_final();
    }

    fn available_credit_discarded(&self) {
        self.credits_available
            .set(self.credits_available.get().saturating_sub(1));
        self.credits_discarded
            .set(self.credits_discarded.get().saturating_add(1));
    }

    fn credit_minted(&self) {
        self.credits_minted
            .set(self.credits_minted.get().saturating_add(1));
        self.credits_available
            .set(self.credits_available.get().saturating_add(1));
    }

    fn raw_leader_led(&self, scout: bool) {
        self.raw_leaders_led
            .set(self.raw_leaders_led.get().saturating_add(1));
        self.raw_leaders_active
            .set(self.raw_leaders_active.get().saturating_add(1));
        if scout {
            self.scout_active
                .set(self.scout_active.get().saturating_add(1));
        }
    }

    fn raw_leader_dispatched(&self) {
        self.raw_leader_dispatches
            .set(self.raw_leader_dispatches.get().saturating_add(1));
        self.logical_retrieve_dispatches
            .set(self.logical_retrieve_dispatches.get().saturating_add(1));
    }

    fn raw_leader_completed(&self, scout: bool) {
        self.raw_leader_completions
            .set(self.raw_leader_completions.get().saturating_add(1));
        self.raw_leaders_active
            .set(self.raw_leaders_active.get().saturating_sub(1));
        if scout {
            self.scout_active
                .set(self.scout_active.get().saturating_sub(1));
        }
    }
}

#[derive(Clone)]
pub(super) struct StartupRawScout {
    admission: RetrieveAdmission,
    credit_out: mpsc::Sender<()>,
    credit_in: mpsc::Receiver<()>,
    profile_trace: Option<Rc<StartupRawProfileTrace>>,
}

impl StartupRawScout {
    pub(super) fn new() -> Self {
        let (credit_out, credit_in) = mpsc::unbounded();
        Self {
            admission: RetrieveAdmission::new(),
            credit_out,
            credit_in,
            profile_trace: StartupRawProfileTrace::activate(),
        }
    }

    pub(super) fn seed_lifecycle_factory(&self) -> RawFetchLifecycleFactory {
        let scout = self.clone();
        RawFetchLifecycleFactory::new(move || Box::new(HlsRawFetchLifecycle::Seed(scout.clone())))
    }

    pub(super) fn close_new_admissions(&self) {
        self.admission.close();
        while self.credit_in.try_recv().is_ok() {
            if let Some(trace) = self.profile_trace.as_ref() {
                trace.available_credit_discarded();
            }
        }
        if let Some(trace) = self.profile_trace.as_ref() {
            trace.admission_closed();
        }
    }

    pub(super) fn new_admissions_open(&self) -> bool {
        self.admission.is_open()
    }

    fn mint_credit(&self) {
        if self.new_admissions_open() && self.credit_out.try_send(()).is_ok() {
            if let Some(trace) = self.profile_trace.as_ref() {
                trace.credit_minted();
            }
        }
    }

    fn return_credit(&self) {
        let returned = self.new_admissions_open() && self.credit_out.try_send(()).is_ok();
        if let Some(trace) = self.profile_trace.as_ref() {
            trace.credit_released(returned);
        }
    }

    async fn acquire_credit(&self) -> Option<StartupRawScoutCredit> {
        if !self.new_admissions_open() {
            return None;
        }
        let credit = self.credit_in.recv();
        let closed = self.admission.wait_closed();
        pin_mut!(credit, closed);
        match select(credit, closed).await {
            Either::Left((Ok(()), _)) if self.new_admissions_open() => {
                if let Some(trace) = self.profile_trace.as_ref() {
                    trace.credit_acquired();
                }
                Some(StartupRawScoutCredit {
                    scout: self.clone(),
                    horizon: None,
                    profile_child: None,
                    leader_active: false,
                })
            }
            Either::Left((Ok(()), _)) => {
                if let Some(trace) = self.profile_trace.as_ref() {
                    trace.available_credit_discarded();
                }
                None
            }
            Either::Left((Err(_), _)) | Either::Right(_) => None,
        }
    }

    fn try_acquire_credit(&self) -> Option<StartupRawScoutCredit> {
        if !self.new_admissions_open() || self.credit_in.try_recv().is_err() {
            return None;
        }
        if let Some(trace) = self.profile_trace.as_ref() {
            trace.credit_acquired();
        }
        Some(StartupRawScoutCredit {
            scout: self.clone(),
            horizon: None,
            profile_child: None,
            leader_active: false,
        })
    }
}

struct StartupRawScoutCredit {
    scout: StartupRawScout,
    horizon: Option<usize>,
    profile_child: Option<StartupRawProfileChild>,
    leader_active: bool,
}

impl StartupRawScoutCredit {
    fn assign_horizon(&mut self, horizon: usize) {
        self.horizon = Some(horizon);
    }

    fn assign_profile_child(&mut self, profile_child: Option<StartupRawProfileChild>) {
        self.profile_child = profile_child;
    }
}

impl Drop for StartupRawScoutCredit {
    fn drop(&mut self) {
        self.scout.return_credit();
    }
}

enum HlsRawFetchLifecycle {
    Seed(StartupRawScout),
    Scout(StartupRawScoutCredit),
}

impl HlsRawFetchLifecycle {
    fn horizon(&self) -> usize {
        match self {
            Self::Seed(_) => 0,
            Self::Scout(credit) => credit.horizon.unwrap_or(0),
        }
    }

    fn trace(&self) -> Option<Rc<StartupRawProfileTrace>> {
        match self {
            Self::Seed(scout) => scout.profile_trace.clone(),
            Self::Scout(credit) => credit.scout.profile_trace.clone(),
        }
    }

    fn profile_child(&self) -> Option<&StartupRawProfileChild> {
        match self {
            Self::Seed(_) => None,
            Self::Scout(credit) => credit.profile_child.as_ref(),
        }
    }

    fn admission_open(&self) -> bool {
        match self {
            Self::Seed(scout) => scout.new_admissions_open(),
            Self::Scout(credit) => credit.scout.new_admissions_open(),
        }
    }

    fn finish_without_leader(self, registration: &'static str, raw_flight_id: Option<u64>) {
        let horizon = self.horizon();
        let admission_open = self.admission_open();
        let trace = self.trace();
        if let Some(trace) = trace.as_ref() {
            trace.emit(
                "registration",
                horizon,
                Some(registration),
                None,
                None,
                raw_flight_id,
                admission_open,
                self.profile_child(),
            );
        }
        drop(self);
        if let Some(trace) = trace {
            trace.maybe_emit_final();
        }
    }

    fn mark_led(&mut self) {
        let scout = match self {
            Self::Seed(_) => false,
            Self::Scout(credit) => {
                credit.leader_active = true;
                true
            }
        };
        if let Some(trace) = self.trace() {
            trace.raw_leader_led(scout);
        }
    }

    fn record_dispatch(&self, accepted: bool, raw_flight_id: u64) {
        if let Some(trace) = self.trace() {
            if accepted {
                trace.raw_leader_dispatched();
            }
            trace.emit(
                "registration",
                self.horizon(),
                Some("Led"),
                Some(accepted),
                None,
                Some(raw_flight_id),
                self.admission_open(),
                self.profile_child(),
            );
        }
    }

    fn complete(self, canonical_cac: bool, raw_flight_id: u64) {
        let horizon = self.horizon();
        let admission_open = self.admission_open();
        let trace = self.trace();
        let profile_child = self.profile_child().cloned();
        let scout_leader = matches!(&self, Self::Scout(credit) if credit.leader_active);
        match self {
            Self::Seed(scout) if canonical_cac => {
                scout.mint_credit();
            }
            Self::Seed(_) => {}
            Self::Scout(mut credit) => {
                credit.leader_active = false;
                drop(credit);
            }
        }
        if let Some(trace) = trace.as_ref() {
            trace.raw_leader_completed(scout_leader);
            trace.emit(
                "completion",
                horizon,
                Some("Led"),
                None,
                Some(canonical_cac),
                Some(raw_flight_id),
                admission_open,
                profile_child.as_ref(),
            );
            trace.maybe_emit_final();
        }
        // A Scout credit is returned by Drop only after its physical shared
        // flight has completed. Closed scouts intentionally discard it.
    }
}

impl RawFetchLifecycle for HlsRawFetchLifecycle {
    fn finish_registration(
        self: Box<Self>,
        registration: RawFetchRegistration,
        raw_flight_id: Option<u64>,
    ) {
        let registration = match registration {
            RawFetchRegistration::Cached => "Cached",
            RawFetchRegistration::Joined => "Joined",
            RawFetchRegistration::Led => "Led",
        };
        (*self).finish_without_leader(registration, raw_flight_id);
    }

    fn leader_selected(&mut self) {
        self.mark_led();
    }

    fn leader_registered(&self, raw_flight_id: u64, dispatch_accepted: bool) {
        self.record_dispatch(dispatch_accepted, raw_flight_id);
    }

    fn complete(self: Box<Self>, raw_flight_id: u64, canonical_cac: bool) {
        HlsRawFetchLifecycle::complete(*self, canonical_cac, raw_flight_id);
    }
}

#[derive(Clone)]
struct ScoutTraversalNode {
    start: u64,
    depth: usize,
    chunk: DecodedJoinChunk,
}

struct StartupScoutChild {
    start: u64,
    depth: usize,
    limit: u64,
    reference: Vec<u8>,
    profile_child: Option<StartupRawProfileChild>,
}

type StartupScoutJoiner =
    FuturesUnordered<Pin<Box<dyn Future<Output = (usize, Option<ScoutTraversalNode>)>>>>;

fn pop_startup_scout_child(
    requested: &mut [VecDeque<StartupScoutChild>],
    earliest_incomplete_horizon: usize,
) -> Option<(usize, StartupScoutChild)> {
    let horizon = startup_scout_next_admission_horizon(
        requested.len(),
        earliest_incomplete_horizon,
        |horizon| !requested[horizon].is_empty(),
    )?;
    requested[horizon].pop_front().map(|child| (horizon, child))
}

fn startup_scout_child_admissible(
    requested: &[VecDeque<StartupScoutChild>],
    earliest_incomplete_horizon: usize,
) -> bool {
    startup_scout_next_admission_horizon(requested.len(), earliest_incomplete_horizon, |horizon| {
        !requested[horizon].is_empty()
    })
    .is_some()
}

fn queue_startup_raw_scout_child(
    horizon: usize,
    child: StartupScoutChild,
    mut credit: StartupRawScoutCredit,
    chunk_retrieve_chan: ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
) -> Pin<Box<dyn Future<Output = (usize, Option<ScoutTraversalNode>)>>> {
    // A credit is acquired before cache/singleflight registration. A cache hit
    // or an ordinary foreground leader therefore returns the unused credit;
    // only a newly-led scout RawFetch holds it through physical completion.
    credit.assign_horizon(horizon.saturating_add(1));
    credit.assign_profile_child(child.profile_child.clone());
    let decoded = queue_decoded_join_child_cancellable(
        child.reference,
        chunk_retrieve_chan,
        cancel,
        Box::new(HlsRawFetchLifecycle::Scout(credit)),
    );

    Box::pin(async move {
        let node = decoded.await.and_then(|chunk| {
            (chunk.span == child.limit).then_some(ScoutTraversalNode {
                start: child.start,
                depth: child.depth,
                chunk,
            })
        });
        (horizon, node)
    })
}

fn admit_startup_raw_scout_child(
    active: &mut StartupScoutJoiner,
    active_counts: &mut [usize],
    horizon: usize,
    child: StartupScoutChild,
    credit: StartupRawScoutCredit,
    chunk_retrieve_chan: ChunkRetrieveSender,
    cancel: Option<RetrieveCancelToken>,
) -> bool {
    let Some(active_count) = active_counts.get_mut(horizon) else {
        return false;
    };
    let Some(next_active_count) = active_count.checked_add(1) else {
        return false;
    };
    *active_count = next_active_count;
    active.push(queue_startup_raw_scout_child(
        horizon,
        child,
        credit,
        chunk_retrieve_chan,
        cancel,
    ));
    true
}

fn finish_startup_raw_scout_child(
    pending: &mut [VecDeque<ScoutTraversalNode>],
    active_counts: &mut [usize],
    scout: &StartupRawScout,
    result: (usize, Option<ScoutTraversalNode>),
) -> bool {
    let (horizon, node) = result;
    let Some(active_count) = active_counts.get_mut(horizon) else {
        scout.close_new_admissions();
        return false;
    };
    let Some(next_active_count) = active_count.checked_sub(1) else {
        scout.close_new_admissions();
        return false;
    };
    *active_count = next_active_count;

    let (Some(node), Some(queue)) = (node, pending.get_mut(horizon)) else {
        scout.close_new_admissions();
        return false;
    };
    queue.push_back(node);
    true
}

/// Best-effort cache-only traversal of several future HLS storage horizons.
/// Credits fill every ready child in the earliest incomplete horizon. Farther
/// work is admitted only while earlier horizons wait on active dependencies.
/// Admissions use ordinary shared keys and transports and never create a range
/// Pending, request parity, reconstruct shards, or assemble an output buffer.
pub(super) async fn scout_data_ranges_cache_only_cancellable(
    root: DecodedJoinChunk,
    payload_ranges: Vec<(u64, u64)>,
    encrypted: bool,
    chunk_retrieve_chan: &ChunkRetrieveSender,
    cancel_generations: Option<RetrieveGenerationMap>,
    cancel: Option<RetrieveCancelToken>,
    scout: StartupRawScout,
) {
    if payload_ranges.is_empty()
        || payload_ranges
            .iter()
            .any(|(start, end)| start > end || *start >= root.span || *end >= root.span)
        || payload_ranges.windows(2).any(|pair| pair[0].1 >= pair[1].0)
    {
        scout.close_new_admissions();
        return;
    }
    let mut pending = payload_ranges
        .iter()
        .map(|_| {
            VecDeque::from([ScoutTraversalNode {
                start: 0,
                depth: 0,
                chunk: root.clone(),
            }])
        })
        .collect::<Vec<_>>();
    let mut requested = payload_ranges
        .iter()
        .map(|_| VecDeque::<StartupScoutChild>::new())
        .collect::<Vec<_>>();
    let mut active = StartupScoutJoiner::new();
    let mut active_counts = vec![0_usize; payload_ranges.len()];
    let mut earliest_incomplete_horizon = 0_usize;

    loop {
        for (horizon, &(payload_start, payload_end_inclusive)) in payload_ranges.iter().enumerate()
        {
            while let Some(node) = pending[horizon].pop_front() {
                if !scout.new_admissions_open()
                    || !scout_cancel_token_current(&cancel_generations, &cancel).await
                    || node.depth > BEE_MAX_UPLOAD_TREE_LEVELS
                {
                    scout.close_new_admissions();
                    break;
                }
                if node.chunk.span <= CHUNK_SIZE as u64 {
                    continue;
                }
                let Some(node_end) = node
                    .start
                    .checked_add(node.chunk.span)
                    .and_then(|end| end.checked_sub(1))
                else {
                    scout.close_new_admissions();
                    break;
                };
                if node.start > payload_end_inclusive || node_end < payload_start {
                    continue;
                }
                let Some(layout) = reference_layout(node.chunk.span, node.chunk.level, encrypted)
                else {
                    scout.close_new_admissions();
                    break;
                };
                let Some((data_references, parity_references)) = split_references(
                    node.chunk.payload.as_ref(),
                    node.chunk.span,
                    node.chunk.level,
                    encrypted,
                ) else {
                    scout.close_new_admissions();
                    break;
                };
                if data_references.len() != layout.data_shards
                    || parity_references.len() != layout.parity_shards
                {
                    scout.close_new_admissions();
                    break;
                }

                let relative_start = payload_start.saturating_sub(node.start);
                let relative_end = payload_end_inclusive
                    .min(node_end)
                    .saturating_sub(node.start);
                let Some(last_data_index) = layout.data_shards.checked_sub(1) else {
                    scout.close_new_admissions();
                    break;
                };
                let first_index = usize::try_from(relative_start / layout.child_capacity)
                    .unwrap_or(usize::MAX)
                    .min(last_data_index);
                let last_index = usize::try_from(relative_end / layout.child_capacity)
                    .unwrap_or(usize::MAX)
                    .min(last_data_index);
                if first_index > last_index {
                    scout.close_new_admissions();
                    break;
                }

                let mut profile_group = None::<Rc<StartupRawProfileGroup>>;
                let mut profile_decoded_raw_count = 0usize;
                let mut profile_decoded_only_count = 0usize;
                let mut profile_cache_miss_count = 0usize;
                for index in first_index..=last_index {
                    let Some(child_offset) = layout.child_capacity.checked_mul(index as u64) else {
                        scout.close_new_admissions();
                        break;
                    };
                    let Some(child_start) = node.start.checked_add(child_offset) else {
                        scout.close_new_admissions();
                        break;
                    };
                    let child_limit = if index + 1 == layout.data_shards {
                        node.chunk.span.saturating_sub(child_offset)
                    } else {
                        layout.child_capacity
                    };
                    let reference = data_references[index].clone();
                    let cached = if scout.profile_trace.is_some() {
                        match requested_shard_cache(&reference) {
                            RequestedShardCache::DecodedAndRaw { decoded, .. } => {
                                profile_decoded_raw_count += 1;
                                Some(decoded)
                            }
                            RequestedShardCache::DecodedOnly(decoded) => {
                                profile_decoded_only_count += 1;
                                Some(decoded)
                            }
                            RequestedShardCache::Miss => {
                                profile_cache_miss_count += 1;
                                None
                            }
                        }
                    } else {
                        cached_decoded_chunk(&reference)
                    };
                    if let Some(chunk) = cached {
                        if chunk.span != child_limit {
                            scout.close_new_admissions();
                            break;
                        }
                        pending[horizon].push_back(ScoutTraversalNode {
                            start: child_start,
                            depth: node.depth + 1,
                            chunk,
                        });
                    } else {
                        let profile_child = scout.profile_trace.as_ref().map(|trace| {
                            let group = profile_group.get_or_insert_with(|| {
                                trace.new_scout_group(
                                    horizon.saturating_add(1),
                                    node.depth,
                                    node.start,
                                    node.chunk.span,
                                    first_index,
                                    last_index,
                                    layout.data_shards,
                                    layout.parity_shards,
                                )
                            });
                            StartupRawProfileChild {
                                group: Rc::clone(group),
                                child_index: index as u64,
                                child_start,
                                child_span: child_limit,
                            }
                        });
                        requested[horizon].push_back(StartupScoutChild {
                            start: child_start,
                            depth: node.depth + 1,
                            limit: child_limit,
                            reference,
                            profile_child,
                        });
                    }
                }
                if let Some(group) = profile_group.as_ref() {
                    group.finalize_cache_classification(
                        profile_decoded_raw_count,
                        profile_decoded_only_count,
                        profile_cache_miss_count,
                    );
                }
            }
        }

        if !scout.new_admissions_open() {
            for queue in &mut requested {
                queue.clear();
            }
            while active.next().await.is_some() {}
            return;
        }

        earliest_incomplete_horizon = startup_scout_nearest_incomplete_horizon(
            pending.len(),
            earliest_incomplete_horizon,
            |horizon| {
                !pending[horizon].is_empty()
                    || !requested[horizon].is_empty()
                    || active_counts[horizon] != 0
            },
        )
        .unwrap_or(pending.len());

        while startup_scout_child_admissible(&requested, earliest_incomplete_horizon) {
            match ready_item_before_credit(&mut active, || {
                if pending.iter().all(|queue| queue.is_empty()) {
                    scout.try_acquire_credit()
                } else {
                    None
                }
            }) {
                ReadyItemOrCredit::Item(result) => {
                    if !finish_startup_raw_scout_child(
                        &mut pending,
                        &mut active_counts,
                        &scout,
                        result,
                    ) {
                        break;
                    }
                }
                ReadyItemOrCredit::Credit(credit) if scout.new_admissions_open() => {
                    let Some((horizon, child)) =
                        pop_startup_scout_child(&mut requested, earliest_incomplete_horizon)
                    else {
                        drop(credit);
                        break;
                    };
                    if !admit_startup_raw_scout_child(
                        &mut active,
                        &mut active_counts,
                        horizon,
                        child,
                        credit,
                        chunk_retrieve_chan.clone(),
                        cancel.clone(),
                    ) {
                        scout.close_new_admissions();
                        break;
                    }
                }
                ReadyItemOrCredit::Credit(_) | ReadyItemOrCredit::Pending => break,
            }
        }
        if !scout.new_admissions_open() {
            continue;
        }

        let pending_empty = pending.iter().all(|queue| queue.is_empty());
        let requested_empty = requested.iter().all(|queue| queue.is_empty());
        if pending_empty && requested_empty && active.is_empty() {
            return;
        }
        if !pending_empty {
            continue;
        }

        let child_admissible =
            startup_scout_child_admissible(&requested, earliest_incomplete_horizon);
        if !child_admissible {
            let Some(result) = active.next().await else {
                scout.close_new_admissions();
                continue;
            };
            finish_startup_raw_scout_child(&mut pending, &mut active_counts, &scout, result);
            continue;
        }

        if active.is_empty() {
            let Some(credit) = scout.acquire_credit().await else {
                continue;
            };
            if !scout.new_admissions_open() {
                drop(credit);
                continue;
            }
            let Some((horizon, child)) =
                pop_startup_scout_child(&mut requested, earliest_incomplete_horizon)
            else {
                drop(credit);
                scout.close_new_admissions();
                continue;
            };
            if !admit_startup_raw_scout_child(
                &mut active,
                &mut active_counts,
                horizon,
                child,
                credit,
                chunk_retrieve_chan.clone(),
                cancel.clone(),
            ) {
                scout.close_new_admissions();
            }
            continue;
        }

        let next_child = active.next();
        let next_credit = scout.acquire_credit();
        pin_mut!(next_child, next_credit);
        // RawFetch completion publishes the child result before returning its
        // credit. Polling the child first makes that fail-fast order explicit
        // when both futures become ready in the same turn.
        match select(next_child, next_credit).await {
            Either::Left((Some(result), _)) => {
                finish_startup_raw_scout_child(&mut pending, &mut active_counts, &scout, result);
            }
            Either::Left(_) => scout.close_new_admissions(),
            Either::Right((Some(credit), _)) if scout.new_admissions_open() => {
                let Some((horizon, child)) =
                    pop_startup_scout_child(&mut requested, earliest_incomplete_horizon)
                else {
                    drop(credit);
                    scout.close_new_admissions();
                    continue;
                };
                if !admit_startup_raw_scout_child(
                    &mut active,
                    &mut active_counts,
                    horizon,
                    child,
                    credit,
                    chunk_retrieve_chan.clone(),
                    cancel.clone(),
                ) {
                    scout.close_new_admissions();
                }
            }
            Either::Right((_credit, _)) => {}
        }
    }
}
