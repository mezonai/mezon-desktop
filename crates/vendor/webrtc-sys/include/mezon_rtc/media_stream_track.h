#pragma once

#include <memory>

#include "api/media_stream_interface.h"
#include "mezon_rtc/helper.h"
#include "mezon_rtc/webrtc.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class MediaStreamTrack;
}
#include "webrtc-sys/src/media_stream_track.rs.h"

namespace mezon_ffi {

class MediaStreamTrack {
 protected:
  MediaStreamTrack(std::shared_ptr<RtcRuntime>,
                   webrtc::scoped_refptr<webrtc::MediaStreamTrackInterface> track);

 public:
  rust::String kind() const;
  rust::String id() const;

  bool enabled() const;
  bool set_enabled(bool enable) const;

  TrackState state() const;

  webrtc::scoped_refptr<webrtc::MediaStreamTrackInterface> rtc_track() const {
    return track_;
  }

 protected:
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::MediaStreamTrackInterface> track_;
};

static std::shared_ptr<MediaStreamTrack> _shared_media_stream_track() {
  return nullptr;  // Ignore
}

}
