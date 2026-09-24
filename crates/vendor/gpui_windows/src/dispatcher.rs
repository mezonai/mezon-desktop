use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{ThreadId, current},
    time::Duration,
};

use anyhow::Context;
use parking_lot::Mutex;
use util::ResultExt;
use windows::{
    System::Threading::{
        ThreadPool, ThreadPoolTimer, TimerElapsedHandler, WorkItemHandler, WorkItemPriority,
    },
    Win32::{
        Foundation::{LPARAM, WPARAM},
        Media::{timeBeginPeriod, timeEndPeriod},
        System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL},
        UI::WindowsAndMessaging::PostMessageW,
    },
};

use crate::{HWND, SafeHwnd, WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD};
use gpui::{
    PlatformDispatcher, Priority, PriorityQueueSender, RunnableVariant, TimerResolutionGuard,
};

// mezon vendor edit: `timeBeginPeriod` sets the timer resolution process-wide, so raising and
// dropping it per holder toggled the global scheduler tick many times a second — once per short
// `dispatch_after` timer (30Hz media pumps, GIF wakeups, scrollbar autohide) and once per
// fallback vsync sleep (`vsync.rs`, up to 120Hz). That is real power and scheduler cost on
// laptops. This guard refcounts the resolution across every holder, so the syscall only fires on
// the 0 -> 1 and 1 -> 0 transitions and 1ms stays held while ANY of them is outstanding. Each
// holder still releases on every path, since the release is a `Drop`.
static HIGH_RES_TIMER_HOLDERS: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct HighResTimerGuard;

impl HighResTimerGuard {
    pub(crate) fn acquire() -> Self {
        if HIGH_RES_TIMER_HOLDERS.fetch_add(1, Ordering::AcqRel) == 0 {
            unsafe { timeBeginPeriod(1) };
        }
        Self
    }
}

impl Drop for HighResTimerGuard {
    fn drop(&mut self) {
        if HIGH_RES_TIMER_HOLDERS.fetch_sub(1, Ordering::AcqRel) == 1 {
            unsafe { timeEndPeriod(1) };
        }
    }
}

pub(crate) struct WindowsDispatcher {
    pub(crate) wake_posted: AtomicBool,
    main_sender: PriorityQueueSender<RunnableVariant>,
    main_thread_id: ThreadId,
    pub(crate) platform_window_handle: SafeHwnd,
    validation_number: usize,
}

impl WindowsDispatcher {
    pub(crate) fn new(
        main_sender: PriorityQueueSender<RunnableVariant>,
        platform_window_handle: HWND,
        validation_number: usize,
    ) -> Self {
        let main_thread_id = current().id();
        let platform_window_handle = platform_window_handle.into();

        WindowsDispatcher {
            main_sender,
            main_thread_id,
            platform_window_handle,
            validation_number,
            wake_posted: AtomicBool::new(false),
        }
    }

    fn dispatch_on_threadpool(&self, priority: WorkItemPriority, runnable: RunnableVariant) {
        let handler = {
            let mut task_wrapper = Some(runnable);
            WorkItemHandler::new(move |_| {
                let runnable = task_wrapper.take().unwrap();
                Self::execute_runnable(runnable);
                Ok(())
            })
        };

        if let Err(error) = ThreadPool::RunWithPriorityAsync(&handler, priority) {
            log::error!("failed to dispatch work to the Windows thread pool: {error}");
            // Zed #60693 adapted to this older WinRT dispatcher: leaking the handler keeps
            // the captured runnable pending instead of cancelling a task during shutdown.
            std::mem::forget(handler);
        }
    }

    fn dispatch_on_threadpool_after(&self, runnable: RunnableVariant, duration: Duration) {
        // mezon vendor edit: hold 1ms system timer resolution while a short timer is
        // pending, else 30Hz media pumps quantize to the ~15.6ms tick (fire at 31-47ms).
        // Scoped to short timers so long TTL timers don't pin 1ms resolution forever.
        // `HighResTimerGuard` refcounts it process-wide, so N pending timers cost one
        // `timeBeginPeriod`, not N. The guard restores resolution on handler completion and is
        // explicitly released if CreateTimer fails before its handler is intentionally leaked.
        const HIGH_RES_THRESHOLD: Duration = Duration::from_millis(100);
        let resolution_guard = Arc::new(Mutex::new(
            (duration < HIGH_RES_THRESHOLD).then(HighResTimerGuard::acquire),
        ));
        let handler = {
            let mut task_wrapper = Some(runnable);
            let resolution_guard = resolution_guard.clone();
            TimerElapsedHandler::new(move |_| {
                let runnable = task_wrapper.take().unwrap();
                Self::execute_runnable(runnable);
                resolution_guard.lock().take();
                Ok(())
            })
        };
        if let Err(error) = ThreadPoolTimer::CreateTimer(&handler, duration.into()) {
            log::error!("failed to create a Windows thread-pool timer: {error}");
            // Preserve Zed's shutdown behavior by leaking the pending runnable, but release
            // Mezon's process-wide timer-resolution holder before leaking its handler.
            resolution_guard.lock().take();
            std::mem::forget(handler);
        }
    }

    #[inline(always)]
    pub(crate) fn execute_runnable(runnable: RunnableVariant) {
        let location = runnable.metadata().location;
        let spawned = runnable.metadata().spawned;
        gpui::profiler::update_running_task(spawned, location);
        runnable.run();
        gpui::profiler::save_task_timing();
    }
}

impl PlatformDispatcher for WindowsDispatcher {
    fn is_main_thread(&self) -> bool {
        current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        let priority = match priority {
            Priority::RealtimeAudio => {
                panic!("RealtimeAudio priority should use spawn_realtime, not dispatch")
            }
            Priority::High => WorkItemPriority::High,
            Priority::Medium => WorkItemPriority::Normal,
            Priority::Low => WorkItemPriority::Low,
        };
        self.dispatch_on_threadpool(priority, runnable);
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, priority: Priority) {
        match self.main_sender.send(priority, runnable) {
            Ok(_) => {
                if !self.wake_posted.swap(true, Ordering::AcqRel) {
                    unsafe {
                        PostMessageW(
                            Some(self.platform_window_handle.as_raw()),
                            WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
                            WPARAM(self.validation_number),
                            LPARAM(0),
                        )
                        .log_err();
                    }
                }
            }
            Err(runnable) => {
                // NOTE: Runnable may wrap a Future that is !Send.
                //
                // This is usually safe because we only poll it on the main thread.
                // However if the send fails, we know that:
                // 1. main_receiver has been dropped (which implies the app is shutting down)
                // 2. we are on a background thread.
                // It is not safe to drop something !Send on the wrong thread, and
                // the app will exit soon anyway, so we must forget the runnable.
                std::mem::forget(runnable);
            }
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        self.dispatch_on_threadpool_after(runnable, duration);
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(move || {
            // SAFETY: always safe to call
            let thread_handle = unsafe { GetCurrentThread() };

            // SAFETY: thread_handle is a valid handle to the current thread
            unsafe { SetThreadPriority(thread_handle, THREAD_PRIORITY_TIME_CRITICAL) }
                .context("thread priority")
                .log_err();

            f();
        });
    }

    fn increase_timer_resolution(&self) -> TimerResolutionGuard {
        // mezon vendor edit: share the process-wide refcount with the short timers above and
        // the fallback vsync sleep, so overlapping holders don't each toggle the global tick.
        let guard = HighResTimerGuard::acquire();
        util::defer(Box::new(move || drop(guard)))
    }
}
