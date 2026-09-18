#include <algorithm>
#include <iostream>
#include <memory>

#include "api/media_stream_interface.h"
#include "api/video/video_frame.h"
#include "api/video/video_rotation.h"
#include "audio/remix_resample.h"
#include "common_audio/include/audio_util.h"
#include "mezon_rtc/media_stream.h"
#include "rtc_base/logging.h"
#include "rtc_base/ref_counted_object.h"
#include "rtc_base/time_utils.h"

namespace mezon_ffi {

MediaStreamTrack::MediaStreamTrack(
    std::shared_ptr<RtcRuntime> rtc_runtime,
    webrtc::scoped_refptr<webrtc::MediaStreamTrackInterface> track)
    : rtc_runtime_(rtc_runtime), track_(std::move(track)) {}

rust::String MediaStreamTrack::kind() const {
  return track_->kind();
}

rust::String MediaStreamTrack::id() const {
  return track_->id();
}

bool MediaStreamTrack::enabled() const {
  return track_->enabled();
}

bool MediaStreamTrack::set_enabled(bool enable) const {
  return track_->set_enabled(enable);
}

TrackState MediaStreamTrack::state() const {
  return static_cast<TrackState>(track_->state());
}

}
