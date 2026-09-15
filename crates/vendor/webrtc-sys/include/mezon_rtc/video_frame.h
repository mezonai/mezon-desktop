#pragma once

#include "api/video/video_frame.h"
#include "mezon_rtc/video_frame_buffer.h"
#include "rtc_base/checks.h"

namespace mezon_ffi {
class VideoFrame;
class VideoFrameBuilder;
}
#include "webrtc-sys/src/video_frame.rs.h"

namespace mezon_ffi {

class VideoFrame {
 public:
  explicit VideoFrame(const webrtc::VideoFrame& frame);

  unsigned int width() const;
  unsigned int height() const;
  uint32_t size() const;
  uint16_t id() const;
  int64_t timestamp_us() const;
  int64_t ntp_time_ms() const;
  uint32_t timestamp() const;

  VideoRotation rotation() const;
  std::unique_ptr<VideoFrameBuffer> video_frame_buffer() const;

  webrtc::VideoFrame get() const;

 private:
  webrtc::VideoFrame frame_;
};

// Allow to create VideoFrames from Rust,
// the builder pattern will be redone in Rust
class VideoFrameBuilder {
 public:
  VideoFrameBuilder() = default;

  void set_video_frame_buffer(const VideoFrameBuffer& buffer);
  void set_timestamp_us(int64_t timestamp_us);
  void set_rotation(VideoRotation rotation);
  void set_id(uint16_t id);
  std::unique_ptr<VideoFrame> build();

 private:
  webrtc::VideoFrame::Builder builder_;
};

std::unique_ptr<VideoFrameBuilder> new_video_frame_builder();

}
