#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("mezon_rtc/prohibit_libsrtp_initialization.h");

        fn ProhibitLibsrtpInitialization();
    }
}
