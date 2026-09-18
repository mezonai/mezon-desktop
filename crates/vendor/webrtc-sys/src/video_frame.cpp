#include "mezon_rtc/video_frame.h"

#include <memory>

#include "api/video/video_frame.h"

namespace mezon_ffi {
VideoFrame::VideoFrame(const webrtc::VideoFrame& frame)
    : frame_(std::move(frame)) {}

unsigned int VideoFrame::width() const {
  return frame_.width();
}
unsigned int VideoFrame::height() const {
  return frame_.height();
}
uint32_t VideoFrame::size() const {
  return frame_.size();
}
uint16_t VideoFrame::id() const {
  return frame_.id();
}
int64_t VideoFrame::timestamp_us() const {
  return frame_.timestamp_us();
}
int64_t VideoFrame::ntp_time_ms() const {
  return frame_.ntp_time_ms();
}
uint32_t VideoFrame::timestamp() const {
  return frame_.rtp_timestamp();
}

VideoRotation VideoFrame::rotation() const {
  return static_cast<VideoRotation>(frame_.rotation());
}

std::unique_ptr<VideoFrameBuffer> VideoFrame::video_frame_buffer() const {
  return std::make_unique<VideoFrameBuffer>(frame_.video_frame_buffer());
}

webrtc::VideoFrame VideoFrame::get() const {
  return frame_;
}

void VideoFrameBuilder::set_video_frame_buffer(const VideoFrameBuffer& buffer) {
  builder_.set_video_frame_buffer(buffer.get());  // const & ref_counted
}

void VideoFrameBuilder::set_timestamp_us(int64_t timestamp_us) {
  builder_.set_timestamp_us(timestamp_us);
}

void VideoFrameBuilder::set_rotation(VideoRotation rotation) {
  builder_.set_rotation(static_cast<webrtc::VideoRotation>(rotation));
}

void VideoFrameBuilder::set_id(uint16_t id) {
  builder_.set_id(id);
}

std::unique_ptr<VideoFrame> VideoFrameBuilder::build() {
  return std::make_unique<VideoFrame>(builder_.build());
}

std::unique_ptr<VideoFrameBuilder> new_video_frame_builder() {
  return std::make_unique<VideoFrameBuilder>();
}

}
