pub mod scan;

use tokio::task::JoinHandle;

pub struct ScanHandle {
    pub handle: JoinHandle<()>,
}

impl ScanHandle {
    pub fn abort(&self) {
        self.handle.abort();
    }
}
