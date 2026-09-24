#pragma once

#include "api/rtc_error.h"
#include "rust/cxx.h"
#include "webrtc-sys/src/rtc_error.rs.h"

namespace mezon_ffi {

RtcError to_error(const webrtc::RTCError& error);
std::string serialize_error(
    const RtcError& error);  // to be used inside cxx::Exception msg

#ifdef MEZON_RTC_TEST
rust::String serialize_deserialize();
#endif

}
