const COMPLETED_PROGRESS_LIMIT: usize = 24;

#[derive(Clone, Debug)]
pub(crate) struct ProgressRow {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub phase: String,
    pub percent: Option<u8>,
    pub detail: String,
    pub done: bool,
    pub ok: bool,
}

#[derive(Debug)]
pub(crate) struct ProgressStore {
    next_id: u64,
    revision: u64,
    rows: Vec<ProgressRow>,
}

impl ProgressStore {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 0,
            revision: 0,
            rows: Vec::new(),
        }
    }

    pub(crate) fn start(
        &mut self,
        kind: impl Into<String>,
        subject: impl Into<String>,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) -> String {
        self.next_id = self.next_id.saturating_add(1);
        let id = format!("op-{}", self.next_id);
        self.rows.insert(
            0,
            ProgressRow {
                id: id.clone(),
                kind: kind.into(),
                subject: subject.into(),
                phase: phase.into(),
                percent,
                detail: detail.into(),
                done: false,
                ok: true,
            },
        );
        self.trim_completed();
        self.bump_revision();
        id
    }

    pub(crate) fn update(
        &mut self,
        id: &str,
        phase: impl Into<String>,
        percent: Option<u8>,
        detail: impl Into<String>,
    ) {
        let phase = phase.into();
        let detail = detail.into();
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return;
        };

        if row.phase == phase && row.percent == percent && row.detail == detail {
            return;
        }

        row.phase = phase;
        row.percent = percent;
        row.detail = detail;
        self.bump_revision();
    }

    pub(crate) fn finish(
        &mut self,
        id: &str,
        phase: impl Into<String>,
        detail: impl Into<String>,
        ok: bool,
    ) {
        let phase = phase.into();
        let detail = detail.into();
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return;
        };

        let percent = if ok { Some(100) } else { row.percent };
        if row.phase == phase
            && row.percent == percent
            && row.detail == detail
            && row.done
            && row.ok == ok
        {
            return;
        }

        row.phase = phase;
        row.percent = percent;
        row.detail = detail;
        row.done = true;
        row.ok = ok;
        self.trim_completed();
        self.bump_revision();
    }

    pub(crate) fn snapshot_if_changed(
        &self,
        seen_revision: u64,
    ) -> Option<(u64, Vec<ProgressRow>)> {
        if self.revision == seen_revision {
            return None;
        }

        let (mut active, completed): (Vec<_>, Vec<_>) =
            self.rows.iter().cloned().partition(|row| !row.done);
        active.extend(completed);

        Some((self.revision, active))
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn trim_completed(&mut self) {
        let mut completed_seen = 0usize;
        self.rows.retain(|row| {
            if !row.done {
                return true;
            }

            completed_seen += 1;
            completed_seen <= COMPLETED_PROGRESS_LIMIT
        });
    }
}
