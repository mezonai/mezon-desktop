#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("mezon_rtc/candidate.h");

        type Candidate; // webrtc::Candidate

        fn _shared_candidate() -> SharedPtr<Candidate>;
    }
}
