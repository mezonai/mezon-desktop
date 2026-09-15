#pragma once

#include "api/video/video_frame_buffer.h"
#include "api/video/i420_buffer.h"

namespace mezon_rtc {

// Pixel format of the DMA buffer surface.
enum class DmaBufPixelFormat {
  kNV12 = 0,
  kYUV420M = 1,
};

// A VideoFrameBuffer backed by a Jetson NvBufSurface DMA file descriptor.
// Reports Type::kNative so it flows through the standard WebRTC pipeline.
// The encoder can detect this type and pass the fd directly to the hardware
// encoder via V4L2_MEMORY_DMABUF for zero-copy encoding.
class DmaBufVideoFrameBuffer : public webrtc::VideoFrameBuffer {
 public:
  DmaBufVideoFrameBuffer(int dmabuf_fd,
                         int width,
                         int height,
                         DmaBufPixelFormat pixel_format);
  ~DmaBufVideoFrameBuffer() override = default;

  // webrtc::VideoFrameBuffer
  Type type() const override;
  int width() const override;
  int height() const override;
  webrtc::scoped_refptr<webrtc::I420BufferInterface> ToI420() override;
  webrtc::scoped_refptr<webrtc::VideoFrameBuffer> CropAndScale(
      int offset_x,
      int offset_y,
      int crop_width,
      int crop_height,
      int scaled_width,
      int scaled_height) override;

  // DMA buffer accessors
  int dmabuf_fd() const { return dmabuf_fd_; }
  DmaBufPixelFormat pixel_format() const { return pixel_format_; }

  // Helper to check if a VideoFrameBuffer is a DmaBufVideoFrameBuffer.
  static DmaBufVideoFrameBuffer* FromNative(webrtc::VideoFrameBuffer* buffer);

 private:
  int dmabuf_fd_;
  int width_;
  int height_;
  DmaBufPixelFormat pixel_format_;
};

}
