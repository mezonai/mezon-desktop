#include "mezon_rtc/rtc_error.h"

#include <iomanip>
#include <sstream>
#include <string>

namespace mezon_ffi {

RtcError to_error(const webrtc::RTCError& error) {
  RtcError rtc_err;
  rtc_err.error_detail = static_cast<RtcErrorDetailType>(error.error_detail());
  rtc_err.error_type = static_cast<RtcErrorType>(error.type());
  rtc_err.has_sctp_cause_code = error.sctp_cause_code().has_value();
  rtc_err.sctp_cause_code = error.sctp_cause_code().value_or(0);
  rtc_err.message = error.message();
  return rtc_err;
}

std::string serialize_error(const RtcError& error) {
  std::stringstream ss;
  ss << std::hex << std::setfill('0');
  ss << std::setw(8) << (uint32_t)error.error_type;
  ss << std::setw(8) << (uint32_t)error.error_detail;
  ss << std::setw(2) << (uint16_t)error.has_sctp_cause_code;
  ss << std::setw(4) << (uint16_t)error.sctp_cause_code;
  ss << std::dec << std::setw(1) << std::string(error.message);
  return ss.str();
}

#ifdef MEZON_RTC_TEST
rust::String serialize_deserialize() {
  RtcError rtc_err;
  rtc_err.error_type = RtcErrorType::InternalError;
  rtc_err.error_detail = RtcErrorDetailType::DataChannelFailure;
  rtc_err.has_sctp_cause_code = true;
  rtc_err.sctp_cause_code = 24;
  rtc_err.message = "this is not a test, I repeat, this is not a test";
  return serialize_error(rtc_err);
}
#endif

}
