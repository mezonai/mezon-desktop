#pragma once

#include <memory>

#include "api/peer_connection_interface.h"
#include "api/rtp_parameters.h"
#include "api/rtp_transceiver_direction.h"
#include "api/rtp_transceiver_interface.h"
#include "api/scoped_refptr.h"
#include "mezon_rtc/rtc_error.h"
#include "mezon_rtc/rtp_parameters.h"
#include "mezon_rtc/rtp_receiver.h"
#include "mezon_rtc/rtp_sender.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class RtpTransceiver;
}
#include "webrtc-sys/src/rtp_transceiver.rs.h"

namespace mezon_ffi {

webrtc::RtpTransceiverInit to_native_rtp_transceiver_init(
    RtpTransceiverInit init);

class RtpTransceiver {
 public:
  RtpTransceiver(
      std::shared_ptr<RtcRuntime> rtc_runtime,
      webrtc::scoped_refptr<webrtc::RtpTransceiverInterface> transceiver,
      webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection);

  MediaType media_type() const;

  rust::String mid() const;

  std::shared_ptr<RtpSender> sender() const;

  std::shared_ptr<RtpReceiver> receiver() const;

  bool stopped() const;

  bool stopping() const;

  RtpTransceiverDirection direction() const;

  void set_direction(RtpTransceiverDirection direction) const;

  RtpTransceiverDirection current_direction() const;

  RtpTransceiverDirection fired_direction() const;

  void stop_standard() const;

  void set_codec_preferences(rust::Vec<RtpCodecCapability> codecs) const;

  rust::Vec<RtpCodecCapability> codec_preferences() const;

  rust::Vec<RtpHeaderExtensionCapability> header_extensions_to_negotiate()
      const;

  rust::Vec<RtpHeaderExtensionCapability> negotiated_header_extensions() const;

  void set_header_extensions_to_negotiate(
      rust::Vec<RtpHeaderExtensionCapability> header_extensions_to_offer) const;

 private:
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::RtpTransceiverInterface> transceiver_;
  webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection_;
};

static std::shared_ptr<RtpTransceiver> _shared_rtp_transceiver() {
  return nullptr;
}

}
