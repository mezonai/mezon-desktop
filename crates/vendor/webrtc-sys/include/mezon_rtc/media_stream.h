#pragma once

#include <memory>

#include "api/media_stream_interface.h"
#include "mezon_rtc/helper.h"
#include "mezon_rtc/webrtc.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class MediaStream;
}
#include "webrtc-sys/src/media_stream.rs.h"

namespace mezon_ffi {

class MediaStream {
 public:
  MediaStream(std::shared_ptr<RtcRuntime> rtc_runtime,
              webrtc::scoped_refptr<webrtc::MediaStreamInterface> stream);

  rust::String id() const;
  rust::Vec<VideoTrackPtr> get_video_tracks() const;
  rust::Vec<AudioTrackPtr> get_audio_tracks() const;

  std::shared_ptr<AudioTrack> find_audio_track(rust::String track_id) const;
  std::shared_ptr<VideoTrack> find_video_track(rust::String track_id) const;

  bool add_track(std::shared_ptr<MediaStreamTrack> track) const;
  bool remove_track(std::shared_ptr<MediaStreamTrack> track) const;

 private:
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::MediaStreamInterface> media_stream_;
};

static std::shared_ptr<MediaStream> _shared_media_stream() {
  return nullptr;  // Ignore
}

}
