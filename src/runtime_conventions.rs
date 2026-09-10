use crate::*;

impl Weeb3 {
    pub(crate) fn runtime_is_started(&self) -> bool {
        self.runtime_started.load(Ordering::Acquire)
    }

    pub(super) async fn retrieve_raw(&self, address: String, chunk: bool) -> Vec<u8> {
        let progress_id = self
            .start_progress(
                if chunk { "chunk" } else { "bytes" },
                address.clone(),
                "retrieve",
                None,
                "starting",
            )
            .await;
        let valaddr = match hex::decode(&address) {
            Ok(hex) => hex,
            Err(_) => {
                self.finish_progress(&progress_id, "failed", "invalid reference", false)
                    .await;
                return vec![];
            }
        };

        let bytes = if chunk {
            let (chan_out, chan_in) = mpsc::bounded::<Vec<u8>>(1);
            let _ = self
                .chunk_port
                .0
                .try_send(chunk_retrieve_request(valaddr, chan_out));
            chan_in.recv().await.unwrap_or_default()
        } else {
            retrieve_data(&valaddr, &self.chunk_port.0).await
        };
        let ok = !bytes.is_empty();
        self.finish_progress(
            &progress_id,
            if ok { "complete" } else { "failed" },
            format!("{} bytes", bytes.len()),
            ok,
        )
        .await;
        bytes
    }

    pub(crate) async fn start_progress(
        &self,
        kind: impl Into<String>,
        subject: impl Into<String>,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) -> String {
        self.progress
            .lock()
            .await
            .start(kind, subject, phase, percent, detail)
    }

    pub(crate) async fn update_progress(
        &self,
        id: &str,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) {
        self.progress
            .lock()
            .await
            .update(id, phase, percent, detail);
    }

    pub(crate) async fn finish_progress(
        &self,
        id: &str,
        phase: impl Into<String>,
        detail: impl Into<String>,
        ok: bool,
    ) {
        self.progress.lock().await.finish(id, phase, detail, ok);
    }

    pub(crate) async fn get_progress_snapshot(
        &self,
        seen_revision: u64,
    ) -> Option<(u64, Vec<ProgressRow>)> {
        self.progress
            .lock()
            .await
            .snapshot_if_changed(seen_revision)
    }
}

pub(crate) fn js_error_message(error: &JsValue) -> String {
    crate::worker_protocol::string_property(error, "message")
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "unknown browser error".to_string())
}

pub(crate) fn spawn_upload_progress_listener(
    progress_store: Arc<Mutex<ProgressStore>>,
    progress_id: String,
    progress_in: mpsc::Receiver<UploadProgressDelta>,
) {
    spawn_local(async move {
        let mut chunks_total = 0u64;
        let mut chunks_done = 0u64;
        let mut last_render = 0.0;

        while let Ok(delta) = progress_in.recv().await {
            chunks_total = chunks_total.saturating_add(delta.chunks_total_delta);
            chunks_done = chunks_done.saturating_add(delta.chunks_done_delta);

            if chunks_total > 0 {
                chunks_done = chunks_done.min(chunks_total);
            }

            let complete = chunks_total > 0 && chunks_done >= chunks_total;
            let now = Date::now();
            if !complete && now - last_render < 250.0 && !chunks_done.is_multiple_of(64) {
                continue;
            }

            let percent = if chunks_total > 0 {
                Some(((chunks_done.saturating_mul(100)) / chunks_total).min(100) as u8)
            } else {
                None
            };
            let detail = if chunks_total > 0 {
                format!("{} of {} chunks pushed", chunks_done, chunks_total)
            } else {
                "waiting for chunk plan".to_string()
            };

            progress_store
                .lock()
                .await
                .update(&progress_id, "push", percent, detail);
            last_render = now;
        }
    });
}

pub(crate) fn interface_log_to(log_port: &mpsc::Sender<String>, log_start_ms: f64, log0: String) {
    if log_port.is_full() {
        return;
    }
    let elapsed_ms = (Date::now() - log_start_ms).max(0.0).round() as u64;
    let log = format!("[+{}ms] {}", elapsed_ms, log0);
    let _ = log_port.try_send(log);
}

pub(crate) async fn cheques_active_in_window() -> bool {
    if get_chequebook_signer_key().await.is_empty() {
        return false;
    }

    let chequebook = get_chequebook_address().await;
    if chequebook.len() != 20 {
        return false;
    }

    let w3 = match web3() {
        Ok(w3) => w3,
        Err(_) => return false,
    };

    chequebook_balance(&w3, web3::types::Address::from_slice(&chequebook))
        .await
        .is_ok_and(|balance| !balance.is_zero())
}

pub(crate) type AsyncPort<T> = (mpsc::Sender<T>, mpsc::Receiver<T>);

pub(crate) fn drain_ready<T>(
    first: Option<T>,
    receiver: &mpsc::Receiver<T>,
) -> impl Iterator<Item = T> {
    first
        .into_iter()
        .chain(std::iter::from_fn(|| receiver.try_recv().ok()))
}

pub(crate) type UploadRequest = (
    Vec<Resource>,
    bool,
    erasure_coding::RedundancyLevel,
    String,
    bool,
    String,
    Option<String>,
    Option<UploadProgressSender>,
    mpsc::Sender<Vec<u8>>,
);
pub(crate) type BootnodeChange = (String, bool, u64);

#[derive(Clone)]
pub(crate) struct ChunkRetrieveSender {
    runtime_scope: usize,
    sender: mpsc::Sender<ChunkRetrieveRequest>,
}

impl ChunkRetrieveSender {
    pub(crate) fn runtime_scope(&self) -> usize {
        self.runtime_scope
    }

    pub(crate) fn try_send(
        &self,
        request: ChunkRetrieveRequest,
    ) -> Result<(), mpsc::TrySendError<ChunkRetrieveRequest>> {
        self.sender.try_send(request)
    }
}

pub(crate) type ChunkRetrieveReceiver = mpsc::Receiver<ChunkRetrieveRequest>;

static NEXT_CHUNK_RETRIEVE_RUNTIME_SCOPE: AtomicUsize = AtomicUsize::new(1);

pub(crate) fn chunk_retrieve_channel() -> (ChunkRetrieveSender, ChunkRetrieveReceiver) {
    let (sender, receiver) = mpsc::unbounded::<ChunkRetrieveRequest>();
    let runtime_scope = NEXT_CHUNK_RETRIEVE_RUNTIME_SCOPE.fetch_add(1, Ordering::Relaxed);
    (
        ChunkRetrieveSender {
            runtime_scope,
            sender,
        },
        receiver,
    )
}

pub(crate) struct ChunkRetrieveRequest {
    pub address: Vec<u8>,
    pub chan: mpsc::Sender<Vec<u8>>,
    pub cancel: Option<RetrieveCancelToken>,
    pub admission: Option<retrieval_conventions::RetrieveAdmission>,
    pub hedge_demand: Option<retrieval_conventions::SharedRetrieveHedgeDemand>,
}

pub(crate) fn chunk_retrieve_request(
    address: Vec<u8>,
    chan: mpsc::Sender<Vec<u8>>,
) -> ChunkRetrieveRequest {
    ChunkRetrieveRequest {
        address,
        chan,
        cancel: None,
        admission: None,
        hedge_demand: None,
    }
}

pub(crate) struct BzzRangeRequest {
    pub(crate) metadata: BzzMetadata,
    pub(crate) start: u64,
    pub(crate) end_inclusive: u64,
    pub(crate) cancel: Option<RetrieveCancelToken>,
    pub(crate) chan: mpsc::Sender<Option<(Vec<u8>, BzzMetadata)>>,
}
