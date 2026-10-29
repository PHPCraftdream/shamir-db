use std::sync::atomic::{AtomicUsize, Ordering};

struct WriterDrainBarrier {
    active: AtomicUsize,
}

impl WriterDrainBarrier {
    fn drain(&self) {
        self.active.load(Ordering::SeqCst);
        while AtomicUsize::load(&self.active, Ordering::SeqCst) != 0 {}
        self.active.load(Ordering::SeqCst);
    }

    fn unpinned(&self) {
        AtomicUsize::load(&self.active, Ordering::Relaxed);
    }
}
