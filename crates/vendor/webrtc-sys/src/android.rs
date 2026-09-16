#[cfg(target_os = "android")]
#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("mezon_rtc/android.h");

        type JavaVM;

        /// Initialize Android WebRTC with the JVM.
        /// Called automatically by init_android_context(), so only call directly
        /// in JNI_OnLoad or when you don't have an Android Context.
        /// Idempotent - safe to call multiple times.
        unsafe fn init_android(vm: *mut JavaVM);

        /// Initialize Android WebRTC with the application context.
        /// This is the main init function - calls init_android() internally,
        /// then initializes ContextUtils for PlatformAudio.
        /// Idempotent - safe to call multiple times.
        ///
        /// # Arguments
        /// * `jvm` - The JavaVM pointer
        /// * `context` - The Android application context (jobject as usize)
        ///
        /// # Returns
        /// true if context init succeeded, false otherwise.
        /// Note: JVM init always happens regardless of return value.
        unsafe fn init_android_context(jvm: *mut JavaVM, context: usize) -> bool;
    }
}
