#pragma once

#include <memory>

#include "api/peer_connection_interface.h"
#include "api/rtp_receiver_interface.h"
#include "api/scoped_refptr.h"
#include "mezon_rtc/helper.h"
#include "mezon_rtc/media_stream.h"
#include "mezon_rtc/rtp_parameters.h"
#include "mezon_rtc/webrtc.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class RtpReceiver;
}
#include "webrtc-sys/src/rtp_receiver.rs.h"
namespace mezon_ffi {

class RtpReceiver {
 public:
  RtpReceiver(
      std::shared_ptr<RtcRuntime> rtc_runtime,
      webrtc::scoped_refptr<webrtc::RtpReceiverInterface> receiver,
      webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection);

  std::shared_ptr<MediaStreamTrack> track() const;

  void get_stats(
      rust::Box<ReceiverContext> ctx,
      rust::Fn<void(rust::Box<ReceiverContext>, rust::String)> on_stats) const;

  rust::Vec<rust::String> stream_ids() const;
  rust::Vec<MediaStreamPtr> streams() const;

  MediaType media_type() const;
  rust::String id() const;

  RtpParameters get_parameters() const;

  // bool set_parameters(RtpParameters parameters) const; // Seems unsupported

  void set_jitter_buffer_minimum_delay(bool is_some,
                                       double delay_seconds) const;

  webrtc::scoped_refptr<webrtc::RtpReceiverInterface> rtc_receiver() const {
    return receiver_;
  }

 private:
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::RtpReceiverInterface> receiver_;
  webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection_;
};

static std::shared_ptr<RtpReceiver> _shared_rtp_receiver() {
  return nullptr;
}

}
