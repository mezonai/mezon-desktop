use std::sync::OnceLock;
use tokio::runtime::Runtime;

static VOICE_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub(crate) fn runtime() -> &'static Runtime {
    VOICE_RUNTIME.get_or_init(|| {
        let worker_threads = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 4))
            .unwrap_or(2);
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            .thread_name("mezon-voice")
            .build()
            .expect("failed to build mezon-voice runtime")
    })
}

pub(crate) fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}
