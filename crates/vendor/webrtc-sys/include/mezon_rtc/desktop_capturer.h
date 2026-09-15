#pragma once
#include <memory>

#include "modules/desktop_capture/desktop_capturer.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class DesktopFrame;
class DesktopCapturer;
class DesktopCapturerOptions;
class Source;
}

#include "webrtc-sys/src/desktop_capturer.rs.h"

namespace mezon_ffi {

class DesktopCapturer : public webrtc::DesktopCapturer::Callback {
 public:
  explicit DesktopCapturer(std::unique_ptr<webrtc::DesktopCapturer> capturer)
      : capturer(std::move(capturer)), callback(std::nullopt) {}

  void OnCaptureResult(webrtc::DesktopCapturer::Result result,
                       std::unique_ptr<webrtc::DesktopFrame> frame) final;

  rust::Vec<Source> get_source_list() const;
  bool select_source(uint64_t id) const { return capturer->SelectSource(id); }
  void start(rust::Box<DesktopCapturerCallbackWrapper> callback);
  void capture_frame() const { capturer->CaptureFrame(); }

 private:
  std::unique_ptr<webrtc::DesktopCapturer> capturer;
  std::optional<rust::Box<DesktopCapturerCallbackWrapper>> callback;
};

class DesktopFrame {
 public:
  DesktopFrame(std::unique_ptr<webrtc::DesktopFrame> frame) : frame(std::move(frame)) {}
  int32_t width() const { return frame->size().width(); }

  int32_t height() const { return frame->size().height(); }

  int32_t left() const { return frame->rect().left(); }

  int32_t top() const { return frame->rect().top(); }

  int32_t stride() const { return frame->stride(); }

  const uint8_t* data() const { return frame->data(); }

 private:
  std::unique_ptr<webrtc::DesktopFrame> frame;
};

std::unique_ptr<DesktopCapturer> new_desktop_capturer(DesktopCapturerOptions options);
}