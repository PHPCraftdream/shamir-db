use bytes::Bytes;
use shamir_collections::{TFxMap, TFxSet};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{ProbeKey, RecordRef, RecordView};
use shamir_types::types::value::InnerValue;

#[derive(Default)]
pub(crate) struct StagedFieldIndex {
    walked: usize,
    overwrites_seen: usize,
    postings: TFxMap<ProbeKey, TFxSet<usize>>,
    undecodable: TFxSet<usize>,
}

impl StagedFieldIndex {
    pub(crate) fn catch_up<'a>(
        &mut self,
        field: &InternerKey,
        len: usize,
        overwritten: &[usize],
        bytes_at: impl Fn(usize) -> Option<&'a Bytes>,
    ) {
        while self.walked < len {
            self.fold(self.walked, field, &bytes_at);
            self.walked += 1;
        }
        while self.overwrites_seen < overwritten.len() {
            let pos = overwritten[self.overwrites_seen];
            self.overwrites_seen += 1;
            self.fold(pos, field, &bytes_at);
        }
    }

    fn fold<'a>(
        &mut self,
        pos: usize,
        field: &InternerKey,
        bytes_at: &impl Fn(usize) -> Option<&'a Bytes>,
    ) {
        #[cfg(test)]
        FOLD_DECODES.with(|count| count.set(count.get().saturating_add(1)));
        let Some(bytes) = bytes_at(pos) else {
            return;
        };
        let decoded = match RecordView::new(bytes) {
            Ok(view) => Ok(view
                .scalar_at(std::slice::from_ref(field))
                .map(ProbeKey::for_scalar)),
            Err(_) => InnerValue::from_bytes(bytes).map(|value| {
                value
                    .scalar_at(std::slice::from_ref(field))
                    .map(ProbeKey::for_scalar)
            }),
        };
        match decoded {
            Ok(Some(keys)) => {
                for key in keys {
                    self.postings.entry(key).or_default().insert(pos);
                }
            }
            Ok(None) => {}
            Err(_) => {
                self.undecodable.insert(pos);
            }
        }
    }

    pub(crate) fn candidates(&self, keys: &[ProbeKey]) -> TFxSet<usize> {
        let mut out = self.undecodable.clone();
        for key in keys {
            if let Some(positions) = self.postings.get(key) {
                out.extend(positions.iter().copied());
            }
        }
        out
    }
}

#[cfg(test)]
thread_local! { static FOLD_DECODES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn reset_fold_decodes() {
    FOLD_DECODES.with(|count| count.set(0));
}
#[cfg(test)]
pub(crate) fn fold_decodes() -> u64 {
    FOLD_DECODES.with(std::cell::Cell::get)
}
