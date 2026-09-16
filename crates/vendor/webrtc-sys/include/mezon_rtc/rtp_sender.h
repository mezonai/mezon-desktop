#pragma once

#include <memory>

#include "api/peer_connection_interface.h"
#include "api/rtp_sender_interface.h"
#include "api/scoped_refptr.h"
#include "mezon_rtc/media_stream.h"
#include "mezon_rtc/rtc_error.h"
#include "mezon_rtc/rtp_parameters.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class RtpSender;
}
#include "webrtc-sys/src/rtp_sender.rs.h"

namespace mezon_ffi {

class RtpSender {
 public:
  RtpSender(
      std::shared_ptr<RtcRuntime> rtc_runtime,
      webrtc::scoped_refptr<webrtc::RtpSenderInterface> sender,
      webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection);

  bool set_track(std::shared_ptr<MediaStreamTrack> track) const;

  std::shared_ptr<MediaStreamTrack> track() const;

  uint32_t ssrc() const;

  void get_stats(
      rust::Box<SenderContext> ctx,
      rust::Fn<void(rust::Box<SenderContext>, rust::String)> on_stats) const;

  MediaType media_type() const;

  rust::String id() const;

  rust::Vec<rust::String> stream_ids() const;

  void set_streams(const rust::Vec<rust::String>& stream_ids) const;

  rust::Vec<RtpEncodingParameters> init_send_encodings() const;

  RtpParameters get_parameters() const;

  void set_parameters(RtpParameters params) const;

  void set_video_encoder_backend(VideoEncoderBackend backend) const;

  webrtc::scoped_refptr<webrtc::RtpSenderInterface> rtc_sender() const {
    return sender_;
  }

 private:
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::RtpSenderInterface> sender_;
  webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection_;
};

static std::shared_ptr<RtpSender> _shared_rtp_sender() {
  return nullptr;  // Ignore
}
}
