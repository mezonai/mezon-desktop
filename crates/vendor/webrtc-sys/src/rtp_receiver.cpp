#include "mezon_rtc/rtp_receiver.h"
#include "mezon_rtc/jsep.h"

#include <memory>

#include "absl/types/optional.h"
#include "api/peer_connection_interface.h"
#include "api/scoped_refptr.h"

namespace mezon_ffi {

RtpReceiver::RtpReceiver(
    std::shared_ptr<RtcRuntime> rtc_runtime,
    webrtc::scoped_refptr<webrtc::RtpReceiverInterface> receiver,
    webrtc::scoped_refptr<webrtc::PeerConnectionInterface> peer_connection)
    : rtc_runtime_(rtc_runtime),
      receiver_(std::move(receiver)),
      peer_connection_(std::move(peer_connection)) {}

std::shared_ptr<MediaStreamTrack> RtpReceiver::track() const {
  return rtc_runtime_->get_or_create_media_stream_track(receiver_->track());
}

rust::Vec<rust::String> RtpReceiver::stream_ids() const {
  rust::Vec<rust::String> rust;
  for (auto id : receiver_->stream_ids())
    rust.push_back(id);
  return rust;
}

void RtpReceiver::get_stats(
    rust::Box<ReceiverContext> ctx,
    rust::Fn<void(rust::Box<ReceiverContext>, rust::String)> on_stats) const {
	auto observer = 
      webrtc::make_ref_counted<NativeRtcStatsCollector<ReceiverContext>>(std::move(ctx), on_stats);
  peer_connection_->GetStats(receiver_, observer);
}

rust::Vec<MediaStreamPtr> RtpReceiver::streams() const {
  rust::Vec<MediaStreamPtr> rust;
  for (auto stream : receiver_->streams())
    rust.push_back(
        MediaStreamPtr{std::make_shared<MediaStream>(rtc_runtime_, stream)});
  return rust;
}

MediaType RtpReceiver::media_type() const {
  return static_cast<MediaType>(receiver_->media_type());
}

rust::String RtpReceiver::id() const {
  return receiver_->id();
}

RtpParameters RtpReceiver::get_parameters() const {
  return to_rust_rtp_parameters(receiver_->GetParameters());
}

void RtpReceiver::set_jitter_buffer_minimum_delay(bool is_some,
                                                  double delay_seconds) const {
  receiver_->SetJitterBufferMinimumDelay(
      is_some ? absl::make_optional(delay_seconds) : absl::nullopt);
}

}
